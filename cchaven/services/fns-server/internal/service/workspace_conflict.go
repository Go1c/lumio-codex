package service

import (
	"context"
	"crypto/rand"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/zeebo/blake3"
)

const workspaceConflictResolveWaitingTTL = 24 * time.Hour

var errWorkspaceConflictRefreshUnrepresentable = errors.New("workspace conflict drift has no representable refreshed generation")

func (s *workspaceSyncService) ResolveConflict(
	ctx context.Context,
	uid int64,
	req dto.WorkspaceConflictResolvedRequest,
) (*WorkspaceResolveOutcome, error) {
	if s == nil {
		return nil, errors.New("workspace sync service is nil")
	}
	if s.initErr != nil {
		return nil, s.initErr
	}
	requestJSON, err := json.Marshal(req)
	if err != nil {
		return nil, fmt.Errorf("marshal workspace conflict resolution digest: %w", err)
	}
	digestSum := blake3.Sum256(requestJSON)
	digest := hex.EncodeToString(digestSum[:])
	if err := s.repo.Migrate(ctx, uid); err != nil {
		return nil, err
	}

	var outcome *WorkspaceResolveOutcome
	var committedErr error
	err = s.repo.Write(ctx, uid, func(tx domain.WorkspaceWriteTx) error {
		now := s.now()
		operation, readErr := tx.Operation(string(req.ClientID), string(req.OperationID))
		if readErr != nil && !errors.Is(readErr, domain.ErrWorkspaceRecordNotFound) {
			return readErr
		}
		if readErr == nil {
			if operation.RequestKind != string(dto.WorkspaceActionConflictResolved) ||
				operation.RequestDigest != digest {
				committedErr = &WorkspaceServiceError{Code: dto.WorkspaceErrorOperationReused}
				return nil
			}
			switch operation.State {
			case "terminal":
				outcome, readErr = workspaceConflictReplayResolution(operation)
				return readErr
			case "expired_guard":
				committedErr = &WorkspaceServiceError{Code: dto.WorkspaceErrorOperationReused}
				return nil
			case "waiting_blob":
				if operation.ExpiresAt == nil || !now.Before(*operation.ExpiresAt) {
					if err := workspaceConflictExpireWaiting(tx, operation, now); err != nil {
						return err
					}
					committedErr = &WorkspaceServiceError{Code: dto.WorkspaceErrorOperationReused}
					return nil
				}
			default:
				return fmt.Errorf("workspace resolve operation has invalid state %q", operation.State)
			}
		} else {
			operation = nil
		}

		workspace, readErr := tx.Workspace(string(req.WorkspaceID))
		if errors.Is(readErr, domain.ErrWorkspaceRecordNotFound) {
			committedErr = &WorkspaceServiceError{Code: dto.WorkspaceErrorWorkspaceNotFound}
			return nil
		}
		if readErr != nil {
			return readErr
		}
		if operation == nil {
			reconciled, operationErr := tx.Operation(string(req.ClientID), string(req.OperationID))
			if operationErr == nil {
				if reconciled.RequestKind != string(dto.WorkspaceActionConflictResolved) ||
					reconciled.RequestDigest != digest {
					committedErr = &WorkspaceServiceError{Code: dto.WorkspaceErrorOperationReused}
					return nil
				}
				switch reconciled.State {
				case "terminal":
					outcome, operationErr = workspaceConflictReplayResolution(reconciled)
					return operationErr
				case "expired_guard":
					committedErr = &WorkspaceServiceError{Code: dto.WorkspaceErrorOperationReused}
					return nil
				case "waiting_blob":
					if reconciled.ExpiresAt == nil || !now.Before(*reconciled.ExpiresAt) {
						if err := workspaceConflictExpireWaiting(tx, reconciled, now); err != nil {
							return err
						}
						committedErr = &WorkspaceServiceError{Code: dto.WorkspaceErrorOperationReused}
						return nil
					}
					operation = reconciled
				default:
					return fmt.Errorf("workspace resolve operation has invalid state %q", reconciled.State)
				}
			} else if !errors.Is(operationErr, domain.ErrWorkspaceRecordNotFound) {
				return operationErr
			}
		}
		conflict, readErr := tx.Conflict(string(req.WorkspaceID), string(req.ConflictID))
		if errors.Is(readErr, domain.ErrWorkspaceRecordNotFound) {
			if operation != nil {
				if err := workspaceConflictExpireWaiting(tx, operation, now); err != nil {
					return err
				}
			}
			committedErr = &WorkspaceServiceError{Code: dto.WorkspaceErrorConflictNotFound}
			return nil
		}
		if readErr != nil {
			return readErr
		}
		if req.ConflictRevision != conflict.ConflictRevision || conflict.Status != "pending" {
			if operation != nil {
				if err := workspaceConflictExpireWaiting(tx, operation, now); err != nil {
					return err
				}
			}
			committedErr = &WorkspaceServiceError{Code: dto.WorkspaceErrorConflictRevisionStale}
			return nil
		}
		unchanged, readErr := workspaceConflictSourceAndTargetMatch(tx, conflict)
		if readErr != nil {
			return readErr
		}
		if !unchanged {
			if operation != nil {
				if err := workspaceConflictExpireWaiting(tx, operation, now); err != nil {
					return err
				}
			}
			if req.Choice == dto.WorkspaceConflictKeepCurrent && conflict.Kind == dto.WorkspaceConflictRename {
				authoritative, terminal, authoritativeErr := workspaceConflictAuthoritativeDeletedSource(tx, conflict)
				if authoritativeErr != nil {
					return authoritativeErr
				}
				if terminal {
					created, decodeErr := workspaceConflictCreatedFromRecord(conflict)
					if decodeErr != nil {
						return decodeErr
					}
					if validateErr := req.ValidateAgainst(*created); validateErr != nil {
						return validateErr
					}
					outcome, readErr = workspaceConflictCommitResolution(
						tx, workspace, conflict, operation, req, digest, now, authoritative,
					)
					return readErr
				}
			}
			refreshed, refreshErr := workspaceConflictRefreshGeneration(tx, conflict, now)
			if refreshErr != nil && !errors.Is(refreshErr, errWorkspaceConflictRefreshUnrepresentable) {
				return refreshErr
			}
			committedErr = &WorkspaceServiceError{
				Code:              dto.WorkspaceErrorConflictRevisionStale,
				RefreshedConflict: refreshed,
			}
			return nil
		}
		created, readErr := workspaceConflictCreatedFromRecord(conflict)
		if readErr != nil {
			return readErr
		}
		if validateErr := req.ValidateAgainst(*created); validateErr != nil {
			return validateErr
		}

		if req.Choice == dto.WorkspaceConflictUseMerged {
			hash := *req.ContentHash.Value
			blob, blobErr := tx.Blob(hash)
			if errors.Is(blobErr, domain.ErrWorkspaceRecordNotFound) ||
				(blobErr == nil && blob.Size != req.Metadata.Size) {
				if operation == nil {
					expiresAt := now.Add(workspaceConflictResolveWaitingTTL)
					if err := tx.SaveOperation(domain.WorkspaceOperationRecord{
						WorkspaceID:      string(req.WorkspaceID),
						ClientID:         string(req.ClientID),
						OperationID:      string(req.OperationID),
						RequestKind:      string(dto.WorkspaceActionConflictResolved),
						RequestDigest:    digest,
						State:            "waiting_blob",
						RequiredHash:     &hash,
						ConflictRevision: &req.ConflictRevision,
						ExpiresAt:        &expiresAt,
						CreatedAt:        now,
						UpdatedAt:        now,
					}); err != nil {
						return err
					}
				}
				committedErr = workspaceConflictBlobRequiredError(req, hash)
				return nil
			}
			if blobErr != nil {
				return blobErr
			}
		}

		outcome, readErr = workspaceConflictCommitResolution(
			tx, workspace, conflict, operation, req, digest, now, nil,
		)
		return readErr
	})
	if err != nil {
		if workspaceSyncIsOperationWriteRace(err) {
			recovered, recoverErr := s.recoverConflictResolution(ctx, uid, req, digest)
			if recoverErr == nil {
				return recovered, nil
			}
			var serviceErr *WorkspaceServiceError
			if errors.As(recoverErr, &serviceErr) {
				return nil, recoverErr
			}
		}
		return nil, fmt.Errorf("resolve workspace conflict: %w", err)
	}
	if committedErr != nil {
		return nil, committedErr
	}
	return outcome, nil
}

func workspaceConflictCommitResolution(
	tx domain.WorkspaceWriteTx,
	workspace *domain.WorkspaceRecord,
	conflict *domain.WorkspaceConflictRecord,
	operation *domain.WorkspaceOperationRecord,
	req dto.WorkspaceConflictResolvedRequest,
	digest string,
	now time.Time,
	authoritativeState *dto.WorkspacePathState,
) (*WorkspaceResolveOutcome, error) {
	if workspace.GlobalRevision == dto.WorkspaceRevision(math.MaxUint64) {
		return nil, &WorkspaceServiceError{Code: dto.WorkspaceErrorInvalidRevision}
	}
	nextRevision := workspace.GlobalRevision + 1
	var state dto.WorkspacePathState
	var err error
	if authoritativeState == nil {
		state, err = workspaceConflictResolvedState(req, nextRevision)
		if err != nil {
			return nil, err
		}
	} else {
		state = *authoritativeState
		state.PathRevision = nextRevision
		if err := state.Validate(); err != nil {
			return nil, err
		}
	}
	oldState, err := workspaceConflictApplyResolvedPaths(tx, workspace, conflict, req, state, now)
	if err != nil {
		return nil, err
	}
	stateJSON, err := json.Marshal(state)
	if err != nil {
		return nil, err
	}
	resolved := &dto.WorkspaceConflictResolvedMessage{
		WorkspaceID:        req.WorkspaceID,
		ConflictID:         req.ConflictID,
		ConflictRevision:   req.ConflictRevision,
		OperationID:        req.OperationID,
		Revision:           nextRevision,
		Choice:             req.Choice,
		PathState:          state,
		ResolvedByClientID: req.ClientID,
	}
	if err := resolved.Validate(); err != nil {
		return nil, err
	}
	resultJSON, err := json.Marshal(resolved)
	if err != nil {
		return nil, err
	}
	item := domain.WorkspaceEventRecord{
		WorkspaceID:    string(req.WorkspaceID),
		Revision:       nextRevision,
		Kind:           "conflict_resolved",
		OperationID:    string(req.OperationID),
		OriginClientID: string(req.ClientID),
		PathStateJSON:  stateJSON,
		ResolvedJSON:   resultJSON,
		CreatedAt:      now,
	}
	if oldState != nil {
		item.OldPathStateJSON, err = json.Marshal(oldState)
		if err != nil {
			return nil, err
		}
		item.NewPathStateJSON = stateJSON
	}
	if err := tx.SaveEvent(item); err != nil {
		return nil, err
	}
	if state.ContentHash.Value != nil {
		if err := tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
			ContentHash: *state.ContentHash.Value,
			OwnerType:   "event",
			OwnerKey:    workspaceSyncEventOwnerKey(req.WorkspaceID, nextRevision),
		}, now); err != nil {
			return nil, err
		}
	}
	if err := tx.RemoveBlobRefs("conflict", conflict.ConflictID, now); err != nil {
		return nil, err
	}
	action := string(dto.WorkspaceActionConflictResolved)
	createdAt := now
	if operation != nil && !operation.CreatedAt.IsZero() {
		createdAt = operation.CreatedAt
	}
	if err := tx.SaveOperation(domain.WorkspaceOperationRecord{
		WorkspaceID:   string(req.WorkspaceID),
		ClientID:      string(req.ClientID),
		OperationID:   string(req.OperationID),
		RequestKind:   string(dto.WorkspaceActionConflictResolved),
		RequestDigest: digest,
		State:         "terminal",
		ResultAction:  &action,
		ResultJSON:    resultJSON,
		CreatedAt:     createdAt,
		UpdatedAt:     now,
	}); err != nil {
		return nil, err
	}
	conflict.Status = "resolved"
	conflict.ResolutionOperationID = workspaceConflictStringPointer(string(req.OperationID))
	conflict.ResolutionRevision = workspaceConflictRevisionPointer(nextRevision)
	conflict.ResolutionChoice = workspaceConflictChoicePointer(req.Choice)
	conflict.ResolutionPathStateJSON = stateJSON
	conflict.ResolvedByClientID = workspaceConflictStringPointer(string(req.ClientID))
	conflict.ResolvedAt = workspaceConflictTimePointer(now)
	conflict.UpdatedAt = now
	if err := tx.SaveConflict(*conflict); err != nil {
		return nil, err
	}
	workspace.GlobalRevision = nextRevision
	workspace.UpdatedAt = now
	if err := tx.SaveWorkspace(*workspace); err != nil {
		return nil, err
	}
	return &WorkspaceResolveOutcome{Resolved: resolved}, nil
}

func workspaceConflictResolvedState(
	req dto.WorkspaceConflictResolvedRequest,
	revision dto.WorkspaceRevision,
) (dto.WorkspacePathState, error) {
	state := dto.WorkspacePathState{
		Path:         req.Path,
		PathRevision: revision,
		ContentHash:  req.ContentHash,
		Metadata:     req.Metadata,
	}
	if req.Choice == dto.WorkspaceConflictDelete {
		state.Kind = dto.WorkspaceEntryTombstone
		state.Tombstone = true
	} else if req.ContentHash.Value == nil {
		state.Kind = dto.WorkspaceEntryDirectory
	} else {
		state.Kind = dto.WorkspaceEntryFile
	}
	if err := state.Validate(); err != nil {
		return dto.WorkspacePathState{}, err
	}
	return state, nil
}

func workspaceConflictApplyResolvedPaths(
	tx domain.WorkspaceWriteTx,
	workspace *domain.WorkspaceRecord,
	conflict *domain.WorkspaceConflictRecord,
	req dto.WorkspaceConflictResolvedRequest,
	state dto.WorkspacePathState,
	now time.Time,
) (*dto.WorkspacePathState, error) {
	current, err := tx.Path(conflict.WorkspaceID, conflict.Path)
	if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
		current = nil
	} else if err != nil {
		return nil, err
	}
	if state.Path == conflict.Path {
		if err := workspaceConflictWritePath(tx, workspace, current, state, req.WorkspaceID, now); err != nil {
			return nil, err
		}
		return nil, nil
	}
	target, err := tx.Path(conflict.WorkspaceID, state.Path)
	if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
		target = nil
	} else if err != nil {
		return nil, err
	}
	if current != nil && current.Kind == dto.WorkspaceEntryDirectory && state.Kind != dto.WorkspaceEntryDirectory {
		return nil, errors.New("workspace directory rename resolution requires a directory target")
	}

	type renameMove struct {
		source      domain.WorkspacePathRecord
		destination dto.WorkspacePath
		target      *domain.WorkspacePathRecord
	}
	moves := make([]renameMove, 0, 1)
	if current != nil {
		moves = append(moves, renameMove{source: *current, destination: state.Path, target: target})
	}
	if current != nil && current.Kind == dto.WorkspaceEntryDirectory && !current.Tombstone {
		paths, pathsErr := tx.Paths(conflict.WorkspaceID)
		if pathsErr != nil {
			return nil, pathsErr
		}
		prefix := string(conflict.Path) + "/"
		for i := range paths {
			if paths[i].ID == current.ID || !strings.HasPrefix(string(paths[i].Path), prefix) {
				continue
			}
			suffix := strings.TrimPrefix(string(paths[i].Path), string(conflict.Path))
			destination, parseErr := dto.ParseWorkspacePath(string(state.Path) + suffix)
			if parseErr != nil {
				return nil, parseErr
			}
			if _, collisionErr := tx.Path(conflict.WorkspaceID, destination); collisionErr == nil {
				return nil, fmt.Errorf("workspace directory rename destination collision at %s", destination)
			} else if !errors.Is(collisionErr, domain.ErrWorkspaceRecordNotFound) {
				return nil, collisionErr
			}
			moves = append(moves, renameMove{source: paths[i], destination: destination})
		}
	}

	tombstone := dto.WorkspacePathState{
		Path:         conflict.Path,
		PathRevision: state.PathRevision,
		Kind:         dto.WorkspaceEntryTombstone,
		ContentHash:  dto.WorkspaceNullableHash{Present: true},
		Metadata:     dto.WorkspaceFileMetadata{},
		Tombstone:    true,
	}
	if err := workspaceConflictWritePath(tx, workspace, current, tombstone, req.WorkspaceID, now); err != nil {
		return nil, err
	}
	if err := workspaceConflictWritePath(tx, workspace, target, state, req.WorkspaceID, now); err != nil {
		return nil, err
	}
	for i := 1; i < len(moves); i++ {
		sourceState := dto.WorkspacePathState{
			Path:         moves[i].source.Path,
			PathRevision: state.PathRevision,
			Kind:         dto.WorkspaceEntryTombstone,
			ContentHash:  dto.WorkspaceNullableHash{Present: true},
			Metadata:     dto.WorkspaceFileMetadata{},
			Tombstone:    true,
		}
		destinationState := workspaceSyncStateFromRecord(moves[i].source)
		destinationState.PathRevision = state.PathRevision
		destinationState.Path = moves[i].destination
		if err := workspaceConflictWritePath(tx, workspace, &moves[i].source, sourceState, req.WorkspaceID, now); err != nil {
			return nil, err
		}
		if err := workspaceConflictWritePath(tx, workspace, moves[i].target, destinationState, req.WorkspaceID, now); err != nil {
			return nil, err
		}
	}
	return &tombstone, nil
}

func workspaceConflictWritePath(
	tx domain.WorkspaceWriteTx,
	workspace *domain.WorkspaceRecord,
	existing *domain.WorkspacePathRecord,
	state dto.WorkspacePathState,
	workspaceID dto.WorkspaceUUID,
	now time.Time,
) error {
	oldSize := uint64(0)
	wasLive := existing != nil && !existing.Tombstone
	if wasLive {
		oldSize = existing.Size
	}
	willLive := !state.Tombstone
	switch {
	case !wasLive && willLive:
		workspace.LivePathCount++
	case wasLive && !willLive:
		workspace.LivePathCount--
	}
	if workspace.LivePathCount < 0 {
		return &domain.WorkspaceCounterUnderflowError{Counter: "live_path_count", Value: workspace.LivePathCount}
	}
	if workspace.LivePathCount > workspaceSyncMaxLivePaths {
		return &WorkspaceServiceError{Code: dto.WorkspaceErrorWorkspaceLimitExceeded}
	}
	if oldSize > workspace.LiveBytes {
		return &domain.WorkspaceCounterUnderflowError{Counter: "live_bytes", Value: -1}
	}
	remaining := workspace.LiveBytes - oldSize
	if state.Metadata.Size > math.MaxUint64-remaining {
		return &domain.WorkspaceCounterOverflowError{Counter: "live_bytes"}
	}
	workspace.LiveBytes = remaining + state.Metadata.Size
	if workspace.LiveBytes > workspaceSyncMaxLiveBytes {
		return &WorkspaceServiceError{Code: dto.WorkspaceErrorWorkspaceLimitExceeded}
	}
	ownerKey := workspaceSyncPathOwnerKey(workspaceID, state.Path)
	if err := tx.RemoveBlobRefs("path", ownerKey, now); err != nil {
		return err
	}
	record := workspaceSyncPathRecord(string(workspaceID), state)
	if existing != nil {
		record.ID = existing.ID
	}
	if err := tx.SavePath(record); err != nil {
		return err
	}
	if state.ContentHash.Value != nil {
		return tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
			ContentHash: *state.ContentHash.Value,
			OwnerType:   "path",
			OwnerKey:    ownerKey,
		}, now)
	}
	return nil
}

func workspaceConflictExpireWaiting(
	tx domain.WorkspaceWriteTx,
	operation *domain.WorkspaceOperationRecord,
	now time.Time,
) error {
	return tx.SaveOperation(domain.WorkspaceOperationRecord{
		WorkspaceID:   operation.WorkspaceID,
		ClientID:      operation.ClientID,
		OperationID:   operation.OperationID,
		RequestKind:   operation.RequestKind,
		RequestDigest: operation.RequestDigest,
		State:         "expired_guard",
		CreatedAt:     operation.CreatedAt,
		UpdatedAt:     now,
	})
}

func workspaceConflictBlobRequiredError(
	req dto.WorkspaceConflictResolvedRequest,
	hash dto.WorkspaceContentHash,
) *WorkspaceServiceError {
	return &WorkspaceServiceError{
		Code: dto.WorkspaceErrorBlobRequired,
		RequiredUpload: &dto.WorkspaceBlobNeedUploadPush{
			WorkspaceID: req.WorkspaceID,
			Direction:   dto.WorkspaceBlobUpload,
			OperationID: req.OperationID,
			ContentHash: hash,
			Size:        req.Metadata.Size,
		},
	}
}

func workspaceConflictReplayResolution(
	operation *domain.WorkspaceOperationRecord,
) (*WorkspaceResolveOutcome, error) {
	if operation.ResultAction == nil || *operation.ResultAction != string(dto.WorkspaceActionConflictResolved) {
		return nil, errors.New("terminal workspace resolution has invalid result action")
	}
	decoded, err := dto.DecodeWorkspaceV2Data(
		dto.WorkspaceActionConflictResolved,
		dto.WorkspaceFlowServerResponse,
		operation.ResultJSON,
	)
	if err != nil {
		return nil, err
	}
	resolved, ok := decoded.(*dto.WorkspaceConflictResolvedMessage)
	if !ok {
		return nil, errors.New("terminal workspace resolution has invalid result type")
	}
	if err := resolved.Validate(); err != nil {
		return nil, err
	}
	return &WorkspaceResolveOutcome{Resolved: resolved}, nil
}

func (s *workspaceSyncService) recoverConflictResolution(
	ctx context.Context,
	uid int64,
	req dto.WorkspaceConflictResolvedRequest,
	digest string,
) (*WorkspaceResolveOutcome, error) {
	var operation *domain.WorkspaceOperationRecord
	err := s.repo.Read(ctx, uid, func(tx domain.WorkspaceReadTx) error {
		var readErr error
		operation, readErr = tx.Operation(string(req.ClientID), string(req.OperationID))
		return readErr
	})
	if err != nil {
		return nil, err
	}
	if operation.RequestKind != string(dto.WorkspaceActionConflictResolved) || operation.RequestDigest != digest {
		return nil, &WorkspaceServiceError{Code: dto.WorkspaceErrorOperationReused}
	}
	switch operation.State {
	case "terminal":
		return workspaceConflictReplayResolution(operation)
	case "waiting_blob":
		if operation.RequiredHash == nil {
			return nil, errors.New("waiting workspace resolution has no required hash")
		}
		return nil, workspaceConflictBlobRequiredError(req, *operation.RequiredHash)
	case "expired_guard":
		return nil, &WorkspaceServiceError{Code: dto.WorkspaceErrorOperationReused}
	default:
		return nil, fmt.Errorf("workspace resolution race ended in state %s", operation.State)
	}
}

func workspaceConflictStringPointer(value string) *string { return &value }

func workspaceConflictRevisionPointer(value dto.WorkspaceRevision) *dto.WorkspaceRevision {
	return &value
}

func workspaceConflictChoicePointer(value dto.WorkspaceConflictChoice) *dto.WorkspaceConflictChoice {
	return &value
}

func workspaceConflictTimePointer(value time.Time) *time.Time { return &value }

func (s *workspaceSyncService) materializeConflict(
	tx domain.WorkspaceWriteTx,
	workspace *domain.WorkspaceRecord,
	current *domain.WorkspacePathRecord,
	operation *domain.WorkspaceOperationRecord,
	mutation dto.WorkspaceMutation,
	digest string,
) (*WorkspaceMutationOutcome, bool, error) {
	if current == nil ||
		mutation.Kind == dto.WorkspaceMutationMkdir ||
		mutation.Kind == dto.WorkspaceMutationUpsertSymlink ||
		(mutation.Kind == dto.WorkspaceMutationRename && current.Tombstone) {
		return nil, false, nil
	}
	ancestor, ok, err := workspaceConflictAncestor(tx, workspace, mutation)
	if err != nil || !ok {
		return nil, false, err
	}
	currentSide := workspaceConflictSideFromPathRecord(*current)
	incoming, ok := workspaceConflictIncomingSide(mutation)
	if !ok || workspaceConflictSidesEqual(currentSide, incoming) {
		return nil, false, nil
	}
	kind, err := workspaceConflictKind(tx, mutation, currentSide, incoming)
	if err != nil {
		return nil, false, err
	}
	conflictRevision, err := workspaceNewConflictRevision()
	if err != nil {
		return nil, false, err
	}
	conflictID := dto.WorkspaceUUID(uuid.NewString())
	var existing *domain.WorkspaceConflictRecord
	if pending, pendingErr := tx.PendingConflict(string(mutation.WorkspaceID), mutation.Path); pendingErr == nil {
		existing = pending
		conflictID = dto.WorkspaceUUID(pending.ConflictID)
		if conflictRevision == pending.ConflictRevision {
			conflictRevision, err = workspaceNewConflictRevisionDifferentFrom(pending.ConflictRevision)
			if err != nil {
				return nil, false, err
			}
		}
	} else if !errors.Is(pendingErr, domain.ErrWorkspaceRecordNotFound) {
		return nil, false, pendingErr
	}
	var renameTarget dto.WorkspaceConflictSide
	if mutation.Kind == dto.WorkspaceMutationRename {
		if mutation.NewPath == nil {
			return nil, false, errors.New("workspace rename conflict has no target path")
		}
		target, targetErr := tx.Path(string(mutation.WorkspaceID), *mutation.NewPath)
		if errors.Is(targetErr, domain.ErrWorkspaceRecordNotFound) {
			renameTarget = workspaceConflictMissingSide()
		} else if targetErr != nil {
			return nil, false, targetErr
		} else {
			renameTarget = workspaceConflictSideFromPathRecord(*target)
		}
	}
	created := &dto.WorkspaceConflictCreatedMessage{
		WorkspaceID:          mutation.WorkspaceID,
		ConflictID:           conflictID,
		ConflictRevision:     conflictRevision,
		Path:                 mutation.Path,
		Kind:                 kind,
		Ancestor:             ancestor,
		Current:              currentSide,
		Incoming:             incoming,
		CreatedByOperationID: mutation.OperationID,
	}
	if err := created.Validate(); err != nil {
		return nil, false, fmt.Errorf("validate workspace conflict: %w", err)
	}
	ancestorJSON, err := json.Marshal(ancestor)
	if err != nil {
		return nil, false, err
	}
	currentJSON, err := json.Marshal(currentSide)
	if err != nil {
		return nil, false, err
	}
	incomingJSON, err := json.Marshal(incoming)
	if err != nil {
		return nil, false, err
	}
	var renameTargetJSON []byte
	if mutation.Kind == dto.WorkspaceMutationRename {
		var descendants []dto.WorkspacePathState
		if current.Kind == dto.WorkspaceEntryDirectory && !current.Tombstone {
			paths, pathsErr := tx.Paths(string(mutation.WorkspaceID))
			if pathsErr != nil {
				return nil, false, pathsErr
			}
			prefix := string(mutation.Path) + "/"
			for i := range paths {
				if paths[i].ID == current.ID || !strings.HasPrefix(string(paths[i].Path), prefix) {
					continue
				}
				descendants = append(descendants, workspaceSyncStateFromRecord(paths[i]))
			}
		}
		renameTargetJSON, err = json.Marshal(workspaceConflictRenameSnapshot{
			Target:      renameTarget,
			Descendants: descendants,
		})
		if err != nil {
			return nil, false, err
		}
	}
	now := s.now()
	if existing != nil {
		if err := tx.RemoveBlobRefs("conflict", existing.ConflictID, now); err != nil {
			return nil, false, err
		}
	}
	if err := tx.SaveConflict(domain.WorkspaceConflictRecord{
		WorkspaceID:          string(mutation.WorkspaceID),
		ConflictID:           string(conflictID),
		ConflictRevision:     conflictRevision,
		Path:                 mutation.Path,
		Kind:                 kind,
		Status:               "pending",
		AncestorJSON:         ancestorJSON,
		CurrentJSON:          currentJSON,
		IncomingJSON:         incomingJSON,
		RenameTargetJSON:     renameTargetJSON,
		CreatedByOperationID: string(mutation.OperationID),
		CreatedAt:            now,
		UpdatedAt:            now,
	}); err != nil {
		return nil, false, err
	}
	for _, hash := range workspaceConflictDistinctHashes(ancestor, currentSide, incoming) {
		if err := tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
			ContentHash: hash,
			OwnerType:   "conflict",
			OwnerKey:    string(conflictID),
		}, now); err != nil {
			return nil, false, err
		}
	}

	currentState := workspaceSyncStateFromRecord(*current)

	rejected := &dto.WorkspaceMutationRejectedMessage{
		WorkspaceID:      mutation.WorkspaceID,
		ClientID:         mutation.ClientID,
		OperationID:      mutation.OperationID,
		Reason:           dto.WorkspaceMutationRejectConflictCreated,
		CurrentPathState: &currentState,
		ConflictID:       &conflictID,
	}
	resultJSON, err := json.Marshal(rejected)
	if err != nil {
		return nil, false, err
	}
	conflictJSON, err := json.Marshal(created)
	if err != nil {
		return nil, false, err
	}
	action := string(dto.WorkspaceActionMutationRejected)
	createdAt := now
	if operation != nil && !operation.CreatedAt.IsZero() {
		createdAt = operation.CreatedAt
	}
	if err := tx.SaveOperation(domain.WorkspaceOperationRecord{
		WorkspaceID:   string(mutation.WorkspaceID),
		ClientID:      string(mutation.ClientID),
		OperationID:   string(mutation.OperationID),
		RequestKind:   string(dto.WorkspaceActionMutation),
		RequestDigest: digest,
		State:         "terminal",
		ResultAction:  &action,
		ResultJSON:    resultJSON,
		ConflictJSON:  conflictJSON,
		CreatedAt:     createdAt,
		UpdatedAt:     now,
	}); err != nil {
		return nil, false, err
	}
	return &WorkspaceMutationOutcome{Rejected: rejected, Conflict: created}, true, nil
}

func workspaceConflictAncestor(
	tx domain.WorkspaceReadTx,
	workspace *domain.WorkspaceRecord,
	mutation dto.WorkspaceMutation,
) (dto.WorkspaceConflictSide, bool, error) {
	if mutation.BasePathRevision == 0 {
		return dto.WorkspaceConflictSide{
			ContentHash: dto.WorkspaceNullableHash{Present: true},
			Tombstone:   true,
		}, true, nil
	}
	if mutation.BasePathRevision < workspace.ReplayFloorRevision {
		return dto.WorkspaceConflictSide{}, false, nil
	}
	events, err := tx.EventsAfter(
		string(mutation.WorkspaceID),
		mutation.BasePathRevision-1,
		mutation.BasePathRevision,
	)
	if err != nil {
		return dto.WorkspaceConflictSide{}, false, err
	}
	if len(events) != 1 || events[0].Revision != mutation.BasePathRevision {
		return dto.WorkspaceConflictSide{}, false, nil
	}
	var state dto.WorkspacePathState
	item := events[0]
	if err := json.Unmarshal(item.PathStateJSON, &state); err != nil {
		return dto.WorkspaceConflictSide{}, false, fmt.Errorf("decode workspace conflict ancestor: %w", err)
	}
	if state.Path != mutation.Path && len(item.OldPathStateJSON) != 0 {
		var oldState dto.WorkspacePathState
		if err := json.Unmarshal(item.OldPathStateJSON, &oldState); err != nil {
			return dto.WorkspaceConflictSide{}, false, fmt.Errorf("decode workspace conflict ancestor old state: %w", err)
		}
		if oldState.Path == mutation.Path {
			state = oldState
		}
	}
	if state.Path != mutation.Path && len(item.NewPathStateJSON) != 0 {
		var newState dto.WorkspacePathState
		if err := json.Unmarshal(item.NewPathStateJSON, &newState); err != nil {
			return dto.WorkspaceConflictSide{}, false, fmt.Errorf("decode workspace conflict ancestor new state: %w", err)
		}
		if newState.Path == mutation.Path {
			state = newState
		}
	}
	if state.Path != mutation.Path {
		return dto.WorkspaceConflictSide{}, false, nil
	}
	return workspaceConflictSideFromStateValue(state), true, nil
}

func workspaceConflictSideFromPathRecord(record domain.WorkspacePathRecord) dto.WorkspaceConflictSide {
	return workspaceConflictSideFromStateValue(workspaceSyncStateFromRecord(record))
}

func workspaceConflictSideFromStateValue(state dto.WorkspacePathState) dto.WorkspaceConflictSide {
	var path *dto.WorkspacePath
	if !state.Tombstone {
		copy := state.Path
		path = &copy
	}
	return dto.WorkspaceConflictSide{
		Path:         path,
		PathRevision: state.PathRevision,
		ContentHash:  state.ContentHash,
		Metadata:     state.Metadata,
		Tombstone:    state.Tombstone,
	}
}

func workspaceConflictMissingSide() dto.WorkspaceConflictSide {
	return dto.WorkspaceConflictSide{
		ContentHash: dto.WorkspaceNullableHash{Present: true},
		Tombstone:   true,
	}
}

type workspaceConflictRenameSnapshot struct {
	Target      dto.WorkspaceConflictSide `json:"target"`
	Descendants []dto.WorkspacePathState  `json:"descendants"`
}

func workspaceConflictDecodeRenameSnapshot(
	encodedJSON []byte,
) (dto.WorkspaceConflictSide, []dto.WorkspacePathState, bool, error) {
	var encoded struct {
		Target      *dto.WorkspaceConflictSide `json:"target"`
		Descendants []dto.WorkspacePathState   `json:"descendants"`
	}
	if err := json.Unmarshal(encodedJSON, &encoded); err != nil {
		return dto.WorkspaceConflictSide{}, nil, false, err
	}
	if encoded.Target != nil {
		return *encoded.Target, encoded.Descendants, true, nil
	}
	var target dto.WorkspaceConflictSide
	if err := json.Unmarshal(encodedJSON, &target); err != nil {
		return dto.WorkspaceConflictSide{}, nil, false, err
	}
	return target, nil, false, nil
}

func workspaceConflictRenameDescendantsMatch(
	tx domain.WorkspaceReadTx,
	workspaceID string,
	path dto.WorkspacePath,
	expected []dto.WorkspacePathState,
) (bool, error) {
	paths, err := tx.Paths(workspaceID)
	if err != nil {
		return false, err
	}
	prefix := string(path) + "/"
	expectedByPath := make(map[dto.WorkspacePath]dto.WorkspacePathState, len(expected))
	for _, state := range expected {
		expectedByPath[state.Path] = state
	}
	actualByPath := make(map[dto.WorkspacePath]dto.WorkspacePathState)
	for i := range paths {
		if !strings.HasPrefix(string(paths[i].Path), prefix) {
			continue
		}
		actualByPath[paths[i].Path] = workspaceSyncStateFromRecord(paths[i])
	}
	if len(actualByPath) != len(expectedByPath) {
		return false, nil
	}
	for descendantPath, expectedState := range expectedByPath {
		actualState, ok := actualByPath[descendantPath]
		if !ok || !workspaceConflictPathStatesEqual(expectedState, actualState) {
			return false, nil
		}
	}
	return true, nil
}

func workspaceConflictPathStatesEqual(left, right dto.WorkspacePathState) bool {
	leftJSON, leftErr := json.Marshal(left)
	rightJSON, rightErr := json.Marshal(right)
	return leftErr == nil && rightErr == nil && string(leftJSON) == string(rightJSON)
}

func workspaceConflictSourceAndTargetMatch(
	tx domain.WorkspaceReadTx,
	conflict *domain.WorkspaceConflictRecord,
) (bool, error) {
	var expectedSource dto.WorkspaceConflictSide
	if err := json.Unmarshal(conflict.CurrentJSON, &expectedSource); err != nil {
		return false, fmt.Errorf("decode workspace conflict source snapshot: %w", err)
	}
	source, err := tx.Path(conflict.WorkspaceID, conflict.Path)
	if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	if !workspaceConflictSidesEqual(expectedSource, workspaceConflictSideFromPathRecord(*source)) {
		return false, nil
	}
	if conflict.Kind != dto.WorkspaceConflictRename {
		return true, nil
	}
	if len(conflict.RenameTargetJSON) == 0 {
		return false, fmt.Errorf("workspace rename conflict has no target snapshot")
	}
	expectedTarget, expectedDescendants, hasDescendantSnapshot, err := workspaceConflictDecodeRenameSnapshot(conflict.RenameTargetJSON)
	if err != nil {
		return false, fmt.Errorf("decode workspace conflict target snapshot: %w", err)
	}
	if source.Kind == dto.WorkspaceEntryDirectory && !source.Tombstone {
		if !hasDescendantSnapshot {
			return false, errors.New("workspace rename conflict has no descendant snapshot")
		}
		unchanged, descendantsErr := workspaceConflictRenameDescendantsMatch(
			tx, conflict.WorkspaceID, conflict.Path, expectedDescendants,
		)
		if descendantsErr != nil {
			return false, descendantsErr
		}
		if !unchanged {
			return false, nil
		}
	}
	var incoming dto.WorkspaceConflictSide
	if err := json.Unmarshal(conflict.IncomingJSON, &incoming); err != nil {
		return false, fmt.Errorf("decode workspace conflict incoming snapshot: %w", err)
	}
	if incoming.Path == nil {
		return false, errors.New("workspace rename conflict has no target path")
	}
	target, err := tx.Path(conflict.WorkspaceID, *incoming.Path)
	if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
		return workspaceConflictSidesEqual(expectedTarget, workspaceConflictMissingSide()), nil
	}
	if err != nil {
		return false, err
	}
	return workspaceConflictSidesEqual(expectedTarget, workspaceConflictSideFromPathRecord(*target)), nil
}

func workspaceConflictRefreshGeneration(
	tx domain.WorkspaceWriteTx,
	conflict *domain.WorkspaceConflictRecord,
	now time.Time,
) (*dto.WorkspaceConflictCreatedMessage, error) {
	var ancestor, incoming dto.WorkspaceConflictSide
	if err := json.Unmarshal(conflict.AncestorJSON, &ancestor); err != nil {
		return nil, fmt.Errorf("decode workspace conflict ancestor snapshot: %w", err)
	}
	if err := json.Unmarshal(conflict.IncomingJSON, &incoming); err != nil {
		return nil, fmt.Errorf("decode workspace conflict incoming snapshot: %w", err)
	}

	currentRecord, err := tx.Path(conflict.WorkspaceID, conflict.Path)
	if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
		currentRecord = nil
	} else if err != nil {
		return nil, err
	}
	current := workspaceConflictMissingSide()
	if currentRecord != nil {
		current = workspaceConflictSideFromPathRecord(*currentRecord)
	}

	kind := conflict.Kind
	var renameTargetJSON []byte
	if kind == dto.WorkspaceConflictRename {
		if currentRecord == nil || currentRecord.Tombstone || incoming.Path == nil {
			return nil, errWorkspaceConflictRefreshUnrepresentable
		}
		target, targetErr := tx.Path(conflict.WorkspaceID, *incoming.Path)
		targetSide := workspaceConflictMissingSide()
		if targetErr == nil {
			targetSide = workspaceConflictSideFromPathRecord(*target)
		} else if !errors.Is(targetErr, domain.ErrWorkspaceRecordNotFound) {
			return nil, targetErr
		}
		descendants := make([]dto.WorkspacePathState, 0)
		if currentRecord.Kind == dto.WorkspaceEntryDirectory {
			paths, pathsErr := tx.Paths(conflict.WorkspaceID)
			if pathsErr != nil {
				return nil, pathsErr
			}
			prefix := string(conflict.Path) + "/"
			for i := range paths {
				if paths[i].ID == currentRecord.ID || !strings.HasPrefix(string(paths[i].Path), prefix) {
					continue
				}
				descendants = append(descendants, workspaceSyncStateFromRecord(paths[i]))
			}
		}
		renameTargetJSON, err = json.Marshal(workspaceConflictRenameSnapshot{
			Target: targetSide, Descendants: descendants,
		})
		if err != nil {
			return nil, err
		}
	} else {
		kind, err = workspaceConflictKindFromSides(tx, current, incoming)
		if err != nil {
			return nil, err
		}
	}

	currentJSON, err := json.Marshal(current)
	if err != nil {
		return nil, err
	}
	refreshed := *conflict
	refreshed.Kind = kind
	refreshed.CurrentJSON = currentJSON
	refreshed.RenameTargetJSON = renameTargetJSON
	refreshed.UpdatedAt = now
	refreshed.ConflictRevision, err = workspaceNewConflictRevisionDifferentFrom(conflict.ConflictRevision)
	if err != nil {
		return nil, err
	}
	created, err := workspaceConflictCreatedFromRecord(&refreshed)
	if err != nil {
		return nil, fmt.Errorf("validate refreshed workspace conflict: %w", err)
	}
	if err := tx.RemoveBlobRefs("conflict", conflict.ConflictID, now); err != nil {
		return nil, err
	}
	if err := tx.SaveConflict(refreshed); err != nil {
		return nil, err
	}
	for _, hash := range workspaceConflictDistinctHashes(ancestor, current, incoming) {
		if err := tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
			ContentHash: hash,
			OwnerType:   "conflict",
			OwnerKey:    conflict.ConflictID,
		}, now); err != nil {
			return nil, err
		}
	}
	*conflict = refreshed
	return created, nil
}

func workspaceConflictKindFromSides(
	tx domain.WorkspaceReadTx,
	current dto.WorkspaceConflictSide,
	incoming dto.WorkspaceConflictSide,
) (dto.WorkspaceConflictKind, error) {
	if current.Tombstone != incoming.Tombstone {
		return dto.WorkspaceConflictDeleteModify, nil
	}
	if current.Tombstone || current.Path == nil || incoming.Path == nil ||
		current.ContentHash.Value == nil || incoming.ContentHash.Value == nil {
		return "", errWorkspaceConflictRefreshUnrepresentable
	}
	for _, side := range []dto.WorkspaceConflictSide{current, incoming} {
		blob, err := tx.Blob(*side.ContentHash.Value)
		if err != nil {
			return "", err
		}
		if !blob.UTF8Valid {
			return dto.WorkspaceConflictBinary, nil
		}
	}
	return dto.WorkspaceConflictContent, nil
}

func workspaceConflictAuthoritativeDeletedSource(
	tx domain.WorkspaceReadTx,
	conflict *domain.WorkspaceConflictRecord,
) (*dto.WorkspacePathState, bool, error) {
	current, err := tx.Path(conflict.WorkspaceID, conflict.Path)
	if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
		return &dto.WorkspacePathState{
			Path:        conflict.Path,
			Kind:        dto.WorkspaceEntryTombstone,
			ContentHash: dto.WorkspaceNullableHash{Present: true},
			Tombstone:   true,
		}, true, nil
	}
	if err != nil {
		return nil, false, err
	}
	if !current.Tombstone {
		return nil, false, nil
	}
	state := workspaceSyncStateFromRecord(*current)
	return &state, true, nil
}

func workspaceConflictIncomingSide(mutation dto.WorkspaceMutation) (dto.WorkspaceConflictSide, bool) {
	side := dto.WorkspaceConflictSide{
		PathRevision: mutation.BasePathRevision,
		ContentHash:  mutation.ContentHash,
		Metadata:     mutation.Metadata,
	}
	switch mutation.Kind {
	case dto.WorkspaceMutationUpsertFile:
		path := mutation.Path
		side.Path = &path
	case dto.WorkspaceMutationDelete:
		side.Tombstone = true
	case dto.WorkspaceMutationRename:
		if mutation.NewPath == nil {
			return dto.WorkspaceConflictSide{}, false
		}
		path := *mutation.NewPath
		side.Path = &path
		if mutation.TargetBasePathRevision != nil {
			side.PathRevision = *mutation.TargetBasePathRevision
		}
	default:
		return dto.WorkspaceConflictSide{}, false
	}
	return side, true
}

func workspaceConflictKind(
	tx domain.WorkspaceReadTx,
	mutation dto.WorkspaceMutation,
	current dto.WorkspaceConflictSide,
	incoming dto.WorkspaceConflictSide,
) (dto.WorkspaceConflictKind, error) {
	if mutation.Kind == dto.WorkspaceMutationRename {
		return dto.WorkspaceConflictRename, nil
	}
	if current.Tombstone != incoming.Tombstone {
		return dto.WorkspaceConflictDeleteModify, nil
	}
	for _, side := range []dto.WorkspaceConflictSide{current, incoming} {
		if side.Tombstone || side.ContentHash.Value == nil {
			continue
		}
		blob, err := tx.Blob(*side.ContentHash.Value)
		if err != nil {
			return "", err
		}
		if !blob.UTF8Valid {
			return dto.WorkspaceConflictBinary, nil
		}
	}
	return dto.WorkspaceConflictContent, nil
}

func workspaceConflictDistinctHashes(sides ...dto.WorkspaceConflictSide) []dto.WorkspaceContentHash {
	seen := make(map[dto.WorkspaceContentHash]struct{})
	result := make([]dto.WorkspaceContentHash, 0, len(sides))
	for _, side := range sides {
		if side.ContentHash.Value == nil {
			continue
		}
		hash := *side.ContentHash.Value
		if _, ok := seen[hash]; ok {
			continue
		}
		seen[hash] = struct{}{}
		result = append(result, hash)
	}
	return result
}

func workspaceConflictSidesEqual(left, right dto.WorkspaceConflictSide) bool {
	leftJSON, leftErr := json.Marshal(left)
	rightJSON, rightErr := json.Marshal(right)
	return leftErr == nil && rightErr == nil && string(leftJSON) == string(rightJSON)
}

func workspaceConflictCreatedFromRecord(record *domain.WorkspaceConflictRecord) (*dto.WorkspaceConflictCreatedMessage, error) {
	var ancestor, current, incoming dto.WorkspaceConflictSide
	if err := json.Unmarshal(record.AncestorJSON, &ancestor); err != nil {
		return nil, err
	}
	if err := json.Unmarshal(record.CurrentJSON, &current); err != nil {
		return nil, err
	}
	if err := json.Unmarshal(record.IncomingJSON, &incoming); err != nil {
		return nil, err
	}
	created := &dto.WorkspaceConflictCreatedMessage{
		WorkspaceID:          dto.WorkspaceUUID(record.WorkspaceID),
		ConflictID:           dto.WorkspaceUUID(record.ConflictID),
		ConflictRevision:     record.ConflictRevision,
		Path:                 record.Path,
		Kind:                 record.Kind,
		Ancestor:             ancestor,
		Current:              current,
		Incoming:             incoming,
		CreatedByOperationID: dto.WorkspaceUUID(record.CreatedByOperationID),
	}
	if err := created.Validate(); err != nil {
		return nil, err
	}
	return created, nil
}

func workspaceConflictReplayMutation(
	tx domain.WorkspaceReadTx,
	operation *domain.WorkspaceOperationRecord,
	outcome *WorkspaceMutationOutcome,
) (*WorkspaceMutationOutcome, error) {
	if outcome.Rejected == nil || outcome.Rejected.Reason != dto.WorkspaceMutationRejectConflictCreated {
		return outcome, nil
	}
	if outcome.Rejected.ConflictID == nil {
		return nil, errors.New("terminal conflict rejection has no conflict id")
	}
	if len(operation.ConflictJSON) != 0 {
		var created dto.WorkspaceConflictCreatedMessage
		if err := json.Unmarshal(operation.ConflictJSON, &created); err != nil {
			return nil, fmt.Errorf("decode terminal workspace conflict: %w", err)
		}
		if err := created.Validate(); err != nil {
			return nil, fmt.Errorf("validate terminal workspace conflict: %w", err)
		}
		record, err := tx.Conflict(operation.WorkspaceID, string(created.ConflictID))
		if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
			return outcome, nil
		}
		if err != nil {
			return nil, err
		}
		if record.Status != "pending" || record.ConflictRevision != created.ConflictRevision {
			return outcome, nil
		}
		outcome.Conflict = &created
		return outcome, nil
	}
	record, err := tx.Conflict(operation.WorkspaceID, string(*outcome.Rejected.ConflictID))
	if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
		return outcome, nil
	}
	if err != nil {
		return nil, err
	}
	if record.Status != "pending" {
		return outcome, nil
	}
	created, err := workspaceConflictCreatedFromRecord(record)
	if err != nil {
		return nil, err
	}
	outcome.Conflict = created
	return outcome, nil
}

func workspaceConflictDefaultNow() time.Time { return time.Now().UTC() }

func workspaceNewConflictRevision() (dto.WorkspaceConflictRevision, error) {
	var raw [8]byte
	if _, err := rand.Read(raw[:]); err != nil {
		return dto.WorkspaceConflictRevision{}, fmt.Errorf("generate workspace conflict revision: %w", err)
	}
	value := binary.BigEndian.Uint64(raw[:])
	if value == 0 {
		value = 1
	}
	return dto.ParseWorkspaceConflictRevision(fmt.Sprintf("%d", value))
}

func workspaceNewConflictRevisionDifferentFrom(
	previous dto.WorkspaceConflictRevision,
) (dto.WorkspaceConflictRevision, error) {
	candidate, err := workspaceNewConflictRevision()
	if err != nil {
		return dto.WorkspaceConflictRevision{}, err
	}
	if candidate != previous {
		return candidate, nil
	}
	for _, fallback := range []string{"1", "2"} {
		candidate, err = dto.ParseWorkspaceConflictRevision(fallback)
		if err != nil {
			return dto.WorkspaceConflictRevision{}, err
		}
		if candidate != previous {
			return candidate, nil
		}
	}
	return dto.WorkspaceConflictRevision{}, errors.New("unable to generate a distinct workspace conflict revision")
}
