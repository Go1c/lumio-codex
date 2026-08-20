package service

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"strings"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/config"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/pkg/util"
	"github.com/zeebo/blake3"
)

const (
	workspaceSyncMaxLivePaths int64  = 50_000
	workspaceSyncMaxLiveBytes uint64 = dto.WorkspaceMaxBlobBytes
)

type WorkspaceSyncService interface {
	Subscribe(ctx context.Context, uid int64, req dto.WorkspaceSubscribeRequest) (*domain.WorkspaceChangeSet, error)
	ApplyMutation(ctx context.Context, uid int64, mutation dto.WorkspaceMutation) (*WorkspaceMutationOutcome, error)
	Acknowledge(ctx context.Context, uid int64, req dto.WorkspaceAckRequest, lastDelivered dto.WorkspaceRevision) error
	ResolveConflict(ctx context.Context, uid int64, req dto.WorkspaceConflictResolvedRequest) (*WorkspaceResolveOutcome, error)
	PruneUser(ctx context.Context, uid int64, now time.Time) error
}

type WorkspaceMutationOutcome struct {
	Accepted       *dto.WorkspaceMutationAcceptedMessage
	Rejected       *dto.WorkspaceMutationRejectedMessage
	Conflict       *dto.WorkspaceConflictCreatedMessage
	RequiredUpload *dto.WorkspaceBlobNeedUploadPush
}

type WorkspaceResolveOutcome struct {
	Resolved *dto.WorkspaceConflictResolvedMessage
}

type WorkspaceServiceError struct {
	Code              dto.WorkspaceV2ErrorCode
	RequiredUpload    *dto.WorkspaceBlobNeedUploadPush
	RefreshedConflict *dto.WorkspaceConflictCreatedMessage
}

func (e *WorkspaceServiceError) Error() string {
	return "workspace sync service error: " + string(e.Code)
}

type workspaceSyncService struct {
	repo                 domain.WorkspaceRepository
	blobStore            WorkspaceBlobStore
	eventRetention       time.Duration
	eventMaxPerWorkspace int
	pruneBatchSize       int
	now                  func() time.Time
	initErr              error
}

// InitError exposes constructor validation to the application wiring layer.
func (s *workspaceSyncService) InitError() error {
	if s == nil {
		return errors.New("workspace sync service is nil")
	}
	return s.initErr
}

const (
	workspaceSyncDefaultEventRetention       = 30 * 24 * time.Hour
	workspaceSyncDefaultEventMaxPerWorkspace = 100_000
	workspaceSyncDefaultPruneBatchSize       = 500
)

func NewWorkspaceSyncService(
	repo domain.WorkspaceRepository,
	blobStore WorkspaceBlobStore,
	configs ...*config.WorkspaceConfig,
) *workspaceSyncService {
	service := &workspaceSyncService{
		repo:                 repo,
		blobStore:            blobStore,
		eventRetention:       workspaceSyncDefaultEventRetention,
		eventMaxPerWorkspace: workspaceSyncDefaultEventMaxPerWorkspace,
		pruneBatchSize:       workspaceSyncDefaultPruneBatchSize,
		now:                  workspaceConflictDefaultNow,
	}
	if repo == nil {
		service.initErr = errors.New("workspace repository is nil")
	} else if blobStore == nil {
		service.initErr = errors.New("workspace blob store is nil")
	} else if len(configs) > 1 {
		service.initErr = errors.New("workspace sync config must be provided at most once")
	} else if len(configs) == 1 {
		if configs[0] == nil {
			service.initErr = errors.New("workspace sync config is nil")
		} else if err := service.configure(configs[0]); err != nil {
			service.initErr = err
		}
	}
	return service
}

func (s *workspaceSyncService) configure(cfg *config.WorkspaceConfig) error {
	if err := cfg.Validate(); err != nil {
		return fmt.Errorf("validate workspace sync config: %w", err)
	}
	eventRetention, err := util.ParseDuration(cfg.EventRetention)
	if err != nil {
		return fmt.Errorf("parse workspace event retention: %w", err)
	}
	s.eventRetention = eventRetention
	s.eventMaxPerWorkspace = cfg.EventMaxPerWorkspace
	s.pruneBatchSize = cfg.PruneBatchSize
	return nil
}

func (s *workspaceSyncService) Subscribe(
	ctx context.Context,
	uid int64,
	req dto.WorkspaceSubscribeRequest,
) (*domain.WorkspaceChangeSet, error) {
	return s.subscribeReplay(ctx, uid, req)
}

// CurrentPendingConflict returns the authoritative pending generation for a
// live notification. Conflict revisions are opaque equality guards, so the
// transport must consult the row rather than trying to order their values.
func (s *workspaceSyncService) CurrentPendingConflict(
	ctx context.Context,
	uid int64,
	workspaceID dto.WorkspaceUUID,
	conflictID dto.WorkspaceUUID,
) (*dto.WorkspaceConflictCreatedMessage, error) {
	if s == nil {
		return nil, errors.New("workspace sync service is nil")
	}
	if s.initErr != nil {
		return nil, s.initErr
	}
	if err := s.repo.Migrate(ctx, uid); err != nil {
		return nil, err
	}
	var record *domain.WorkspaceConflictRecord
	err := s.repo.Read(ctx, uid, func(tx domain.WorkspaceReadTx) error {
		var readErr error
		record, readErr = tx.Conflict(string(workspaceID), string(conflictID))
		return readErr
	})
	if err != nil {
		if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
			return nil, nil
		}
		return nil, err
	}
	if record == nil || record.Status != "pending" {
		return nil, nil
	}
	return workspaceConflictCreatedFromRecord(record)
}

func (s *workspaceSyncService) ApplyMutation(
	ctx context.Context,
	uid int64,
	mutation dto.WorkspaceMutation,
) (*WorkspaceMutationOutcome, error) {
	if s == nil {
		return nil, errors.New("workspace sync service is nil")
	}
	if s.initErr != nil {
		return nil, s.initErr
	}
	if err := mutation.Validate(); err != nil {
		return nil, err
	}
	requestJSON, err := json.Marshal(mutation)
	if err != nil {
		return nil, fmt.Errorf("marshal workspace mutation digest: %w", err)
	}
	digestSum := blake3.Sum256(requestJSON)
	digest := hex.EncodeToString(digestSum[:])
	if err := s.repo.Migrate(ctx, uid); err != nil {
		return nil, err
	}

	var outcome *WorkspaceMutationOutcome
	err = s.repo.Write(ctx, uid, func(tx domain.WorkspaceWriteTx) error {
		operation, readErr := tx.Operation(string(mutation.ClientID), string(mutation.OperationID))
		if readErr == nil {
			var handled bool
			var operationErr error
			outcome, handled, operationErr = workspaceSyncExistingMutationOutcome(tx, operation, mutation, digest)
			if handled {
				return operationErr
			}
		}
		if readErr != nil && !errors.Is(readErr, domain.ErrWorkspaceRecordNotFound) {
			return readErr
		}

		workspace, readErr := tx.Workspace(string(mutation.WorkspaceID))
		if errors.Is(readErr, domain.ErrWorkspaceRecordNotFound) {
			return &WorkspaceServiceError{Code: dto.WorkspaceErrorWorkspaceNotFound}
		}
		if readErr != nil {
			return readErr
		}
		reconciledOperation, operationErr := tx.Operation(string(mutation.ClientID), string(mutation.OperationID))
		if operationErr == nil {
			var handled bool
			outcome, handled, operationErr = workspaceSyncExistingMutationOutcome(
				tx,
				reconciledOperation,
				mutation,
				digest,
			)
			if handled {
				return operationErr
			}
			operation = reconciledOperation
		} else if errors.Is(operationErr, domain.ErrWorkspaceRecordNotFound) {
			operation = nil
		} else {
			return operationErr
		}
		if _, readErr = tx.Client(string(mutation.WorkspaceID), string(mutation.ClientID)); errors.Is(readErr, domain.ErrWorkspaceRecordNotFound) {
			return &WorkspaceServiceError{Code: dto.WorkspaceErrorClientNotRegistered}
		} else if readErr != nil {
			return readErr
		}

		current, readErr := tx.Path(string(mutation.WorkspaceID), mutation.Path)
		if errors.Is(readErr, domain.ErrWorkspaceRecordNotFound) {
			current = nil
		} else if readErr != nil {
			return readErr
		}
		currentRevision := dto.WorkspaceRevision(0)
		if current != nil {
			currentRevision = current.PathRevision
		}
		var finalHash *dto.WorkspaceContentHash
		if mutation.Kind != dto.WorkspaceMutationRename && mutation.ContentHash.Value != nil {
			hash := *mutation.ContentHash.Value
			finalHash = &hash
			blob, blobErr := tx.Blob(*finalHash)
			if errors.Is(blobErr, domain.ErrWorkspaceRecordNotFound) ||
				(blobErr == nil && blob.Size != mutation.Metadata.Size) {
				now := time.Now().UTC()
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
					State:         "waiting_blob",
					RequiredHash:  finalHash,
					CreatedAt:     createdAt,
					UpdatedAt:     now,
				}); err != nil {
					return err
				}
				rejected := &dto.WorkspaceMutationRejectedMessage{
					WorkspaceID:  mutation.WorkspaceID,
					ClientID:     mutation.ClientID,
					OperationID:  mutation.OperationID,
					Reason:       dto.WorkspaceMutationRejectBlobRequired,
					RequiredHash: finalHash,
				}
				if current != nil {
					state := workspaceSyncStateFromRecord(*current)
					rejected.CurrentPathState = &state
				}
				outcome = &WorkspaceMutationOutcome{
					Rejected: rejected,
					RequiredUpload: &dto.WorkspaceBlobNeedUploadPush{
						WorkspaceID: mutation.WorkspaceID,
						Direction:   dto.WorkspaceBlobUpload,
						OperationID: mutation.OperationID,
						ContentHash: *finalHash,
						Size:        mutation.Metadata.Size,
					},
				}
				return nil
			}
			if blobErr != nil {
				return blobErr
			}
		}
		if mutation.BasePathRevision != currentRevision {
			var created bool
			outcome, created, readErr = s.materializeConflict(
				tx, workspace, current, operation, mutation, digest,
			)
			if readErr != nil || created {
				return readErr
			}
			var rejectErr error
			outcome, rejectErr = workspaceSyncStoreMutationRejection(
				tx, operation, mutation, digest, current, dto.WorkspaceMutationRejectStaleBase,
			)
			return rejectErr
		}
		if mutation.Kind == dto.WorkspaceMutationRename {
			var renameErr error
			outcome, renameErr = s.workspaceSyncApplyRename(
				tx, workspace, current, operation, mutation, requestJSON, digest,
			)
			return renameErr
		}
		if workspace.GlobalRevision == dto.WorkspaceRevision(math.MaxUint64) {
			return &WorkspaceServiceError{Code: dto.WorkspaceErrorInvalidRevision}
		}
		nextRevision := workspace.GlobalRevision + 1
		state := dto.WorkspacePathState{
			Path:         mutation.Path,
			PathRevision: nextRevision,
			ContentHash:  dto.WorkspaceNullableHash{Present: true},
			Metadata:     mutation.Metadata,
		}
		switch mutation.Kind {
		case dto.WorkspaceMutationUpsertFile:
			state.Kind = dto.WorkspaceEntryFile
			state.ContentHash = workspaceSyncNullableHash(*finalHash)
		case dto.WorkspaceMutationUpsertSymlink:
			state.Kind = dto.WorkspaceEntrySymlink
			state.ContentHash = workspaceSyncNullableHash(*finalHash)
		case dto.WorkspaceMutationMkdir:
			state.Kind = dto.WorkspaceEntryDirectory
		case dto.WorkspaceMutationDelete:
			state.Kind = dto.WorkspaceEntryTombstone
			state.Tombstone = true
		default:
			return fmt.Errorf("workspace mutation kind %q is not implemented", mutation.Kind)
		}

		oldSize := uint64(0)
		wasLive := current != nil && !current.Tombstone
		if wasLive {
			oldSize = current.Size
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
		remainingBytes := workspace.LiveBytes - oldSize
		if state.Metadata.Size > math.MaxUint64-remainingBytes {
			return &domain.WorkspaceCounterOverflowError{Counter: "live_bytes"}
		}
		workspace.LiveBytes = remainingBytes + state.Metadata.Size
		if workspace.LiveBytes > workspaceSyncMaxLiveBytes {
			return &WorkspaceServiceError{Code: dto.WorkspaceErrorWorkspaceLimitExceeded}
		}

		pathRecord := workspaceSyncPathRecord(string(mutation.WorkspaceID), state)
		if current != nil {
			pathRecord.ID = current.ID
		}
		pathOwner := workspaceSyncPathOwnerKey(mutation.WorkspaceID, mutation.Path)
		if err := tx.RemoveBlobRefs("path", pathOwner, time.Now().UTC()); err != nil {
			return err
		}
		if err := tx.SavePath(pathRecord); err != nil {
			return err
		}
		if finalHash != nil {
			if err := tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
				ContentHash: *finalHash,
				OwnerType:   "path",
				OwnerKey:    pathOwner,
			}, time.Now().UTC()); err != nil {
				return err
			}
		}

		accepted := &dto.WorkspaceMutationAcceptedMessage{
			WorkspaceID: mutation.WorkspaceID,
			ClientID:    mutation.ClientID,
			OperationID: mutation.OperationID,
			Revision:    nextRevision,
			PathState:   state,
		}
		acceptedJSON, err := json.Marshal(accepted)
		if err != nil {
			return err
		}
		stateJSON, err := json.Marshal(state)
		if err != nil {
			return err
		}
		if err := tx.SaveEvent(domain.WorkspaceEventRecord{
			WorkspaceID:    string(mutation.WorkspaceID),
			Revision:       nextRevision,
			OperationID:    string(mutation.OperationID),
			OriginClientID: string(mutation.ClientID),
			MutationJSON:   requestJSON,
			PathStateJSON:  stateJSON,
			CreatedAt:      time.Now().UTC(),
		}); err != nil {
			return err
		}
		if finalHash != nil {
			if err := tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
				ContentHash: *finalHash,
				OwnerType:   "event",
				OwnerKey:    workspaceSyncEventOwnerKey(mutation.WorkspaceID, nextRevision),
			}, time.Now().UTC()); err != nil {
				return err
			}
		}
		action := string(dto.WorkspaceActionMutationAccepted)
		createdAt := time.Now().UTC()
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
			ResultJSON:    acceptedJSON,
			CreatedAt:     createdAt,
			UpdatedAt:     time.Now().UTC(),
		}); err != nil {
			return err
		}
		workspace.GlobalRevision = nextRevision
		workspace.UpdatedAt = time.Now().UTC()
		if err := tx.SaveWorkspace(*workspace); err != nil {
			return err
		}
		outcome = &WorkspaceMutationOutcome{Accepted: accepted}
		return nil
	})
	if err != nil {
		if workspaceSyncIsOperationWriteRace(err) {
			recovered, recoverErr := s.recoverMutationOperation(ctx, uid, mutation, digest)
			if recoverErr == nil {
				return recovered, nil
			}
		}
		return nil, fmt.Errorf("apply workspace mutation: %w", err)
	}
	return outcome, nil
}

func workspaceSyncExistingMutationOutcome(
	tx domain.WorkspaceReadTx,
	operation *domain.WorkspaceOperationRecord,
	mutation dto.WorkspaceMutation,
	digest string,
) (*WorkspaceMutationOutcome, bool, error) {
	if operation.RequestKind != string(dto.WorkspaceActionMutation) || operation.RequestDigest != digest {
		return workspaceSyncOperationReusedOutcome(mutation), true, nil
	}
	switch operation.State {
	case "terminal":
		outcome, err := workspaceSyncReplayMutation(tx, operation)
		return outcome, true, err
	case "waiting_blob":
		return nil, false, nil
	default:
		return nil, true, fmt.Errorf("workspace operation already exists in state %s", operation.State)
	}
}

func workspaceSyncOperationReusedOutcome(mutation dto.WorkspaceMutation) *WorkspaceMutationOutcome {
	return &WorkspaceMutationOutcome{Rejected: &dto.WorkspaceMutationRejectedMessage{
		WorkspaceID: mutation.WorkspaceID,
		ClientID:    mutation.ClientID,
		OperationID: mutation.OperationID,
		Reason:      dto.WorkspaceMutationRejectOperationReused,
	}}
}

func workspaceSyncIsOperationWriteRace(err error) bool {
	var immutable *domain.WorkspaceOperationImmutableError
	if errors.As(err, &immutable) {
		return true
	}
	var unique *domain.WorkspaceUniqueConstraintError
	return errors.As(err, &unique) && unique.Entity == "operation"
}

func (s *workspaceSyncService) recoverMutationOperation(
	ctx context.Context,
	uid int64,
	mutation dto.WorkspaceMutation,
	digest string,
) (*WorkspaceMutationOutcome, error) {
	var operation *domain.WorkspaceOperationRecord
	err := s.repo.Read(ctx, uid, func(tx domain.WorkspaceReadTx) error {
		var readErr error
		operation, readErr = tx.Operation(string(mutation.ClientID), string(mutation.OperationID))
		return readErr
	})
	if err != nil {
		return nil, err
	}
	if operation.RequestKind != string(dto.WorkspaceActionMutation) || operation.RequestDigest != digest {
		return workspaceSyncOperationReusedOutcome(mutation), nil
	}
	switch operation.State {
	case "terminal":
		var outcome *WorkspaceMutationOutcome
		err = s.repo.Read(ctx, uid, func(tx domain.WorkspaceReadTx) error {
			var replayErr error
			outcome, replayErr = workspaceSyncReplayMutation(tx, operation)
			return replayErr
		})
		return outcome, err
	case "waiting_blob":
		if operation.RequiredHash == nil {
			return nil, errors.New("waiting workspace mutation has no required hash")
		}
		hash := *operation.RequiredHash
		return &WorkspaceMutationOutcome{
			Rejected: &dto.WorkspaceMutationRejectedMessage{
				WorkspaceID:  mutation.WorkspaceID,
				ClientID:     mutation.ClientID,
				OperationID:  mutation.OperationID,
				Reason:       dto.WorkspaceMutationRejectBlobRequired,
				RequiredHash: &hash,
			},
			RequiredUpload: &dto.WorkspaceBlobNeedUploadPush{
				WorkspaceID: mutation.WorkspaceID,
				Direction:   dto.WorkspaceBlobUpload,
				OperationID: mutation.OperationID,
				ContentHash: hash,
				Size:        mutation.Metadata.Size,
			},
		}, nil
	default:
		return nil, fmt.Errorf("workspace operation race ended in state %s", operation.State)
	}
}

func (s *workspaceSyncService) workspaceSyncApplyRename(
	tx domain.WorkspaceWriteTx,
	workspace *domain.WorkspaceRecord,
	source *domain.WorkspacePathRecord,
	operation *domain.WorkspaceOperationRecord,
	mutation dto.WorkspaceMutation,
	requestJSON []byte,
	digest string,
) (*WorkspaceMutationOutcome, error) {
	if source == nil || source.Tombstone {
		return nil, errors.New("workspace rename source is not live")
	}
	if !workspaceSyncMutationMatchesRecord(mutation, *source) {
		return nil, errors.New("workspace rename state does not match source")
	}
	target, err := tx.Path(string(mutation.WorkspaceID), *mutation.NewPath)
	if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
		target = nil
	} else if err != nil {
		return nil, err
	}
	targetRevision := dto.WorkspaceRevision(0)
	if target != nil {
		targetRevision = target.PathRevision
	}
	if *mutation.TargetBasePathRevision != targetRevision {
		outcome, created, conflictErr := s.materializeConflict(
			tx, workspace, source, operation, mutation, digest,
		)
		if conflictErr != nil || created {
			return outcome, conflictErr
		}
		return workspaceSyncStoreMutationRejection(
			tx, operation, mutation, digest, source, dto.WorkspaceMutationRejectStaleBase,
		)
	}

	type renameMove struct {
		source      domain.WorkspacePathRecord
		destination dto.WorkspacePath
		target      *domain.WorkspacePathRecord
	}
	moves := []renameMove{{source: *source, destination: *mutation.NewPath, target: target}}
	if source.Kind == dto.WorkspaceEntryDirectory {
		paths, pathsErr := tx.Paths(string(mutation.WorkspaceID))
		if pathsErr != nil {
			return nil, pathsErr
		}
		prefix := string(mutation.Path) + "/"
		for i := range paths {
			if paths[i].ID == source.ID || !strings.HasPrefix(string(paths[i].Path), prefix) {
				continue
			}
			suffix := strings.TrimPrefix(string(paths[i].Path), string(mutation.Path))
			destination, parseErr := dto.ParseWorkspacePath(string(*mutation.NewPath) + suffix)
			if parseErr != nil {
				return nil, parseErr
			}
			var destinationTarget *domain.WorkspacePathRecord
			existing, collisionErr := tx.Path(string(mutation.WorkspaceID), destination)
			switch {
			case collisionErr == nil && !existing.Tombstone:
				return nil, fmt.Errorf("workspace directory rename destination collision at %s", destination)
			case collisionErr == nil:
				destinationTarget = existing
			case !errors.Is(collisionErr, domain.ErrWorkspaceRecordNotFound):
				return nil, collisionErr
			}
			moves = append(moves, renameMove{
				source: paths[i], destination: destination, target: destinationTarget,
			})
		}
	}
	for i := range moves {
		if moves[i].source.ContentHash == nil || moves[i].source.Tombstone {
			continue
		}
		blob, blobErr := tx.Blob(*moves[i].source.ContentHash)
		if blobErr != nil {
			return nil, blobErr
		}
		if blob.Size != moves[i].source.Size {
			return nil, &WorkspaceServiceError{Code: dto.WorkspaceErrorBlobSizeMismatch}
		}
	}
	if workspace.GlobalRevision == dto.WorkspaceRevision(math.MaxUint64) {
		return nil, &WorkspaceServiceError{Code: dto.WorkspaceErrorInvalidRevision}
	}
	nextRevision := workspace.GlobalRevision + 1
	oldState := dto.WorkspacePathState{
		Path:         mutation.Path,
		PathRevision: nextRevision,
		Kind:         dto.WorkspaceEntryTombstone,
		ContentHash:  dto.WorkspaceNullableHash{Present: true},
		Metadata:     dto.WorkspaceFileMetadata{},
		Tombstone:    true,
	}
	newState := workspaceSyncStateFromRecord(*source)
	newState.Path = *mutation.NewPath
	newState.PathRevision = nextRevision
	if err := oldState.Validate(); err != nil {
		return nil, err
	}
	if err := newState.Validate(); err != nil {
		return nil, err
	}

	if target != nil && !target.Tombstone {
		if workspace.LivePathCount == 0 {
			return nil, &domain.WorkspaceCounterUnderflowError{Counter: "live_path_count", Value: -1}
		}
		workspace.LivePathCount--
		if target.Size > workspace.LiveBytes {
			return nil, &domain.WorkspaceCounterUnderflowError{Counter: "live_bytes", Value: -1}
		}
		workspace.LiveBytes -= target.Size
	}
	if workspace.LivePathCount > workspaceSyncMaxLivePaths || workspace.LiveBytes > workspaceSyncMaxLiveBytes {
		return nil, &WorkspaceServiceError{Code: dto.WorkspaceErrorWorkspaceLimitExceeded}
	}

	now := time.Now().UTC()
	for i := range moves {
		sourceState := dto.WorkspacePathState{
			Path:         moves[i].source.Path,
			PathRevision: nextRevision,
			Kind:         dto.WorkspaceEntryTombstone,
			ContentHash:  dto.WorkspaceNullableHash{Present: true},
			Metadata:     dto.WorkspaceFileMetadata{},
			Tombstone:    true,
		}
		destinationState := workspaceSyncStateFromRecord(moves[i].source)
		destinationState.Path = moves[i].destination
		destinationState.PathRevision = nextRevision
		sourceOwner := workspaceSyncPathOwnerKey(mutation.WorkspaceID, moves[i].source.Path)
		destinationOwner := workspaceSyncPathOwnerKey(mutation.WorkspaceID, moves[i].destination)
		if err := tx.RemoveBlobRefs("path", sourceOwner, now); err != nil {
			return nil, err
		}
		if err := tx.RemoveBlobRefs("path", destinationOwner, now); err != nil {
			return nil, err
		}
		sourceRecord := workspaceSyncPathRecord(string(mutation.WorkspaceID), sourceState)
		sourceRecord.ID = moves[i].source.ID
		if err := tx.SavePath(sourceRecord); err != nil {
			return nil, err
		}
		destinationRecord := workspaceSyncPathRecord(string(mutation.WorkspaceID), destinationState)
		if moves[i].target != nil {
			destinationRecord.ID = moves[i].target.ID
		}
		if err := tx.SavePath(destinationRecord); err != nil {
			return nil, err
		}
		if moves[i].source.ContentHash != nil && !moves[i].source.Tombstone {
			if err := tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
				ContentHash: *moves[i].source.ContentHash,
				OwnerType:   "path",
				OwnerKey:    destinationOwner,
			}, now); err != nil {
				return nil, err
			}
		}
		if i == 0 {
			oldState = sourceState
			newState = destinationState
		}
	}
	accepted := &dto.WorkspaceMutationAcceptedMessage{
		WorkspaceID:  mutation.WorkspaceID,
		ClientID:     mutation.ClientID,
		OperationID:  mutation.OperationID,
		Revision:     nextRevision,
		PathState:    newState,
		OldPathState: &oldState,
		NewPathState: &newState,
	}
	acceptedJSON, err := json.Marshal(accepted)
	if err != nil {
		return nil, err
	}
	pathStateJSON, err := json.Marshal(newState)
	if err != nil {
		return nil, err
	}
	oldStateJSON, err := json.Marshal(oldState)
	if err != nil {
		return nil, err
	}
	newStateJSON, err := json.Marshal(newState)
	if err != nil {
		return nil, err
	}
	if err := tx.SaveEvent(domain.WorkspaceEventRecord{
		WorkspaceID:      string(mutation.WorkspaceID),
		Revision:         nextRevision,
		OperationID:      string(mutation.OperationID),
		OriginClientID:   string(mutation.ClientID),
		MutationJSON:     requestJSON,
		PathStateJSON:    pathStateJSON,
		OldPathStateJSON: oldStateJSON,
		NewPathStateJSON: newStateJSON,
		CreatedAt:        now,
	}); err != nil {
		return nil, err
	}
	if source.ContentHash != nil {
		if err := tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
			ContentHash: *source.ContentHash,
			OwnerType:   "event",
			OwnerKey:    workspaceSyncEventOwnerKey(mutation.WorkspaceID, nextRevision),
		}, now); err != nil {
			return nil, err
		}
	}
	action := string(dto.WorkspaceActionMutationAccepted)
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
		ResultJSON:    acceptedJSON,
		CreatedAt:     createdAt,
		UpdatedAt:     now,
	}); err != nil {
		return nil, err
	}
	workspace.GlobalRevision = nextRevision
	workspace.UpdatedAt = now
	if err := tx.SaveWorkspace(*workspace); err != nil {
		return nil, err
	}
	return &WorkspaceMutationOutcome{Accepted: accepted}, nil
}

func workspaceSyncMutationMatchesRecord(
	mutation dto.WorkspaceMutation,
	record domain.WorkspacePathRecord,
) bool {
	if mutation.Metadata != (dto.WorkspaceFileMetadata{
		Size: record.Size, ModifiedAtMS: record.ModifiedAtMS, Executable: record.Executable,
	}) {
		return false
	}
	if (mutation.ContentHash.Value == nil) != (record.ContentHash == nil) {
		return false
	}
	return mutation.ContentHash.Value == nil || *mutation.ContentHash.Value == *record.ContentHash
}

func workspaceSyncStoreMutationRejection(
	tx domain.WorkspaceWriteTx,
	operation *domain.WorkspaceOperationRecord,
	mutation dto.WorkspaceMutation,
	digest string,
	current *domain.WorkspacePathRecord,
	reason string,
) (*WorkspaceMutationOutcome, error) {
	rejected := &dto.WorkspaceMutationRejectedMessage{
		WorkspaceID: mutation.WorkspaceID,
		ClientID:    mutation.ClientID,
		OperationID: mutation.OperationID,
		Reason:      reason,
	}
	if current != nil {
		state := workspaceSyncStateFromRecord(*current)
		rejected.CurrentPathState = &state
	}
	resultJSON, err := json.Marshal(rejected)
	if err != nil {
		return nil, err
	}
	now := time.Now().UTC()
	createdAt := now
	if operation != nil && !operation.CreatedAt.IsZero() {
		createdAt = operation.CreatedAt
	}
	action := string(dto.WorkspaceActionMutationRejected)
	if err := tx.SaveOperation(domain.WorkspaceOperationRecord{
		WorkspaceID:   string(mutation.WorkspaceID),
		ClientID:      string(mutation.ClientID),
		OperationID:   string(mutation.OperationID),
		RequestKind:   string(dto.WorkspaceActionMutation),
		RequestDigest: digest,
		State:         "terminal",
		ResultAction:  &action,
		ResultJSON:    resultJSON,
		CreatedAt:     createdAt,
		UpdatedAt:     now,
	}); err != nil {
		return nil, err
	}
	return &WorkspaceMutationOutcome{Rejected: rejected}, nil
}

func workspaceSyncReplayMutation(
	tx domain.WorkspaceReadTx,
	operation *domain.WorkspaceOperationRecord,
) (*WorkspaceMutationOutcome, error) {
	if operation.ResultAction == nil {
		return nil, errors.New("terminal workspace mutation has no result action")
	}
	action := dto.WorkspaceV2Action(*operation.ResultAction)
	decoded, err := dto.DecodeWorkspaceV2Data(action, dto.WorkspaceFlowServerResponse, operation.ResultJSON)
	if err != nil {
		return nil, fmt.Errorf("decode terminal workspace mutation: %w", err)
	}
	switch result := decoded.(type) {
	case *dto.WorkspaceMutationAcceptedMessage:
		if err := result.Validate(); err != nil {
			return nil, fmt.Errorf("validate terminal accepted mutation: %w", err)
		}
		return &WorkspaceMutationOutcome{Accepted: result}, nil
	case *dto.WorkspaceMutationRejectedMessage:
		if err := result.Validate(); err != nil {
			return nil, fmt.Errorf("validate terminal rejected mutation: %w", err)
		}
		return workspaceConflictReplayMutation(tx, operation, &WorkspaceMutationOutcome{Rejected: result})
	default:
		return nil, fmt.Errorf("invalid terminal workspace mutation action %q", action)
	}
}

func workspaceSyncNullableHash(hash dto.WorkspaceContentHash) dto.WorkspaceNullableHash {
	copy := hash
	return dto.WorkspaceNullableHash{Present: true, Value: &copy}
}

func workspaceSyncPathRecord(workspaceID string, state dto.WorkspacePathState) domain.WorkspacePathRecord {
	return domain.WorkspacePathRecord{
		WorkspaceID:  workspaceID,
		Path:         state.Path,
		PathRevision: state.PathRevision,
		Kind:         state.Kind,
		ContentHash:  state.ContentHash.Value,
		Size:         state.Metadata.Size,
		ModifiedAtMS: state.Metadata.ModifiedAtMS,
		Executable:   state.Metadata.Executable,
		Tombstone:    state.Tombstone,
	}
}

func workspaceSyncStateFromRecord(record domain.WorkspacePathRecord) dto.WorkspacePathState {
	state := dto.WorkspacePathState{
		Path:         record.Path,
		PathRevision: record.PathRevision,
		Kind:         record.Kind,
		ContentHash:  dto.WorkspaceNullableHash{Present: true},
		Metadata: dto.WorkspaceFileMetadata{
			Size:         record.Size,
			ModifiedAtMS: record.ModifiedAtMS,
			Executable:   record.Executable,
		},
		Tombstone: record.Tombstone,
	}
	if record.ContentHash != nil {
		state.ContentHash = workspaceSyncNullableHash(*record.ContentHash)
	}
	return state
}

func workspaceSyncPathOwnerKey(workspaceID dto.WorkspaceUUID, path dto.WorkspacePath) string {
	sum := blake3.Sum256([]byte(path))
	return string(workspaceID) + "/" + hex.EncodeToString(sum[:])
}

func workspaceSyncEventOwnerKey(workspaceID dto.WorkspaceUUID, revision dto.WorkspaceRevision) string {
	return fmt.Sprintf("%s/%d", workspaceID, revision)
}
