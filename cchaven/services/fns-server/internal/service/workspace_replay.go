package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"sync"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
)

type workspacePendingConflictCursor struct {
	mu       sync.Mutex
	snapshot domain.WorkspaceReadSnapshot
	items    []*dto.WorkspaceConflictCreatedMessage
	next     int
	afterID  string
	done     bool
	fetch    func(context.Context, string) ([]*dto.WorkspaceConflictCreatedMessage, error)
}

const workspacePendingConflictPageSize = 500

func workspaceUint32Count(value int) (uint32, error) {
	if value < 0 || uint64(value) > uint64(^uint32(0)) {
		return 0, fmt.Errorf("workspace count %d exceeds uint32", value)
	}
	return uint32(value), nil
}

func workspaceUint32Count64(value int64) (uint32, error) {
	if value < 0 || uint64(value) > uint64(^uint32(0)) {
		return 0, fmt.Errorf("workspace count %d exceeds uint32", value)
	}
	return uint32(value), nil
}

func (c *workspacePendingConflictCursor) Next(ctx context.Context) (*dto.WorkspaceConflictCreatedMessage, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	for c.next >= len(c.items) && !c.done {
		page, err := c.fetch(ctx, c.afterID)
		if err != nil {
			return nil, err
		}
		if len(page) == 0 {
			c.done = true
			if err := c.closeLocked(); err != nil {
				return nil, err
			}
			break
		}
		c.items = page
		c.next = 0
	}
	if c.next >= len(c.items) {
		if c.done {
			if err := c.closeLocked(); err != nil {
				return nil, err
			}
		}
		return nil, nil
	}
	item := c.items[c.next]
	c.next++
	c.afterID = string(item.ConflictID)
	copy := *item
	return &copy, nil
}

func (c *workspacePendingConflictCursor) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.closeLocked()
}

func (c *workspacePendingConflictCursor) closeLocked() error {
	if c.snapshot == nil {
		return nil
	}
	snapshot := c.snapshot
	c.snapshot = nil
	return snapshot.Close()
}

func (s *workspaceSyncService) subscribeReplay(
	ctx context.Context,
	uid int64,
	req dto.WorkspaceSubscribeRequest,
) (*domain.WorkspaceChangeSet, error) {
	if s == nil {
		return nil, errors.New("workspace sync service is nil")
	}
	if s.initErr != nil {
		return nil, s.initErr
	}
	if err := req.Validate(); err != nil {
		return nil, err
	}
	if err := s.repo.Migrate(ctx, uid); err != nil {
		return nil, err
	}

	err := s.repo.Write(ctx, uid, func(tx domain.WorkspaceWriteTx) error {
		workspace, err := tx.Workspace(string(req.WorkspaceID))
		if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
			if req.LastAckRevision != 0 {
				return &WorkspaceServiceError{Code: dto.WorkspaceErrorWorkspaceNotFound}
			}
			workspace = &domain.WorkspaceRecord{WorkspaceID: string(req.WorkspaceID)}
			if err := tx.CreateWorkspace(*workspace); err != nil {
				return err
			}
		} else if err != nil {
			return err
		}
		client, err := tx.Client(string(req.WorkspaceID), string(req.ClientID))
		if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
			if req.LastAckRevision != 0 {
				return &WorkspaceServiceError{Code: dto.WorkspaceErrorClientNotRegistered}
			}
			client = &domain.WorkspaceClientRecord{
				WorkspaceID: string(req.WorkspaceID),
				ClientID:    string(req.ClientID),
			}
		} else if err != nil {
			return err
		}
		if req.LastAckRevision > workspace.GlobalRevision {
			return &WorkspaceServiceError{Code: dto.WorkspaceErrorInvalidRevision}
		}
		client.LastSeenAt = s.now()
		if err := tx.SaveClient(*client); err != nil {
			return err
		}
		return nil
	})
	if err != nil {
		return nil, fmt.Errorf("subscribe workspace: %w", err)
	}

	snapshot, err := s.repo.OpenReadSnapshot(ctx, uid)
	if err != nil {
		return nil, fmt.Errorf("open workspace snapshot: %w", err)
	}
	keepSnapshot := false
	defer func() {
		if !keepSnapshot {
			_ = snapshot.Close()
		}
	}()

	workspace, err := snapshot.Workspace(string(req.WorkspaceID))
	if err != nil {
		return nil, fmt.Errorf("read workspace snapshot: %w", err)
	}
	pendingCount, err := snapshot.PendingConflictCount(string(req.WorkspaceID))
	if err != nil {
		return nil, fmt.Errorf("read pending conflict count: %w", err)
	}
	pendingBoundary, err := snapshot.PendingConflictBoundary(string(req.WorkspaceID))
	if err != nil {
		return nil, fmt.Errorf("read pending conflict boundary: %w", err)
	}
	pendingRecords, err := snapshot.PendingConflictPage(
		string(req.WorkspaceID), "", pendingBoundary, workspacePendingConflictPageSize,
	)
	if err != nil {
		return nil, fmt.Errorf("read pending conflict page: %w", err)
	}
	pending, err := workspacePendingConflictMessages(pendingRecords)
	if err != nil {
		return nil, err
	}
	cursor := &workspacePendingConflictCursor{
		snapshot: snapshot,
		items:    pending,
		done:     pendingBoundary == "" || int64(len(pending)) >= pendingCount,
		fetch: func(fetchCtx context.Context, afterID string) ([]*dto.WorkspaceConflictCreatedMessage, error) {
			records, pageErr := snapshot.PendingConflictPage(
				string(req.WorkspaceID), afterID, pendingBoundary, workspacePendingConflictPageSize,
			)
			if pageErr != nil {
				return nil, pageErr
			}
			return workspacePendingConflictMessages(records)
		},
	}
	changeSet := &domain.WorkspaceChangeSet{
		FromRevision:     req.LastAckRevision,
		FinalRevision:    workspace.GlobalRevision,
		PendingConflicts: cursor,
	}
	changeSet.ConflictCount, err = workspaceUint32Count64(pendingCount)
	if err != nil {
		return nil, err
	}

	if req.LastAckRevision == 0 || req.LastAckRevision < workspace.ReplayFloorRevision {
		entries, entryErr := workspaceSnapshotEntries(snapshot, string(req.WorkspaceID))
		if entryErr != nil {
			return nil, entryErr
		}
		changeSet.Mode = dto.WorkspaceSnapshotFull
		changeSet.Entries = entries
		changeSet.EntryCount, err = workspaceUint32Count(len(entries))
		if err != nil {
			return nil, err
		}
	} else {
		records, readErr := snapshot.EventsAfter(
			string(req.WorkspaceID), req.LastAckRevision, workspace.GlobalRevision,
		)
		if readErr != nil {
			return nil, readErr
		}
		want := uint64(workspace.GlobalRevision - req.LastAckRevision)
		if uint64(len(records)) != want {
			return nil, fmt.Errorf("workspace revision item gap: expected %d, got %d", want, len(records))
		}
		items, itemErr := workspaceRevisionItems(records)
		if itemErr != nil {
			return nil, itemErr
		}
		changeSet.Mode = dto.WorkspaceSnapshotIncremental
		changeSet.RevisionItems = items
		changeSet.EventCount, err = workspaceUint32Count(len(items))
		if err != nil {
			return nil, err
		}
		for _, item := range items {
			if item.Event != nil {
				changeSet.Events = append(changeSet.Events, *item.Event)
			}
		}
	}
	if changeSet.ConflictCount != 0 {
		keepSnapshot = true
	} else {
		changeSet.PendingConflicts = nil
	}
	return changeSet, nil
}

func workspaceSnapshotEntries(tx domain.WorkspaceReadTx, workspaceID string) ([]dto.WorkspacePathState, error) {
	paths, err := tx.Paths(workspaceID)
	if err != nil {
		return nil, err
	}
	sort.Slice(paths, func(i, j int) bool { return string(paths[i].Path) < string(paths[j].Path) })
	entries := make([]dto.WorkspacePathState, 0, len(paths))
	for _, path := range paths {
		state := workspaceSyncStateFromRecord(path)
		if err := state.Validate(); err != nil {
			return nil, fmt.Errorf("validate workspace snapshot path %q: %w", path.Path, err)
		}
		entries = append(entries, state)
	}
	return entries, nil
}

func workspacePendingConflictMessages(records []domain.WorkspaceConflictRecord) ([]*dto.WorkspaceConflictCreatedMessage, error) {
	result := make([]*dto.WorkspaceConflictCreatedMessage, 0, len(records))
	for i := range records {
		created, err := workspaceConflictCreatedFromRecord(&records[i])
		if err != nil {
			return nil, fmt.Errorf("decode pending workspace conflict %q: %w", records[i].ConflictID, err)
		}
		result = append(result, created)
	}
	return result, nil
}

func workspaceRevisionItems(records []domain.WorkspaceEventRecord) ([]domain.WorkspaceRevisionItem, error) {
	items := make([]domain.WorkspaceRevisionItem, 0, len(records))
	previous := dto.WorkspaceRevision(0)
	for _, record := range records {
		if record.Revision <= previous {
			return nil, fmt.Errorf("workspace revision items are not strictly increasing at %d", record.Revision)
		}
		previous = record.Revision
		if record.Kind == "conflict_resolved" {
			var resolved dto.WorkspaceConflictResolvedMessage
			if err := json.Unmarshal(record.ResolvedJSON, &resolved); err != nil {
				return nil, fmt.Errorf("decode conflict resolved revision %d: %w", record.Revision, err)
			}
			if err := resolved.Validate(); err != nil {
				return nil, fmt.Errorf("validate conflict resolved revision %d: %w", record.Revision, err)
			}
			if resolved.Revision != record.Revision {
				return nil, fmt.Errorf("conflict resolved revision mismatch: row=%d body=%d", record.Revision, resolved.Revision)
			}
			copy := resolved
			items = append(items, domain.WorkspaceRevisionItem{Revision: record.Revision, ConflictResolved: &copy})
			continue
		}
		stored, err := workspaceStoredEvent(record)
		if err != nil {
			return nil, err
		}
		items = append(items, domain.WorkspaceRevisionItem{Revision: record.Revision, Event: stored})
	}
	return items, nil
}

func workspaceStoredEvent(record domain.WorkspaceEventRecord) (*domain.WorkspaceStoredEvent, error) {
	var mutation dto.WorkspaceMutation
	if err := json.Unmarshal(record.MutationJSON, &mutation); err != nil {
		return nil, fmt.Errorf("decode workspace event mutation %d: %w", record.Revision, err)
	}
	if err := mutation.Validate(); err != nil {
		return nil, fmt.Errorf("validate workspace event mutation %d: %w", record.Revision, err)
	}
	var state dto.WorkspacePathState
	if err := json.Unmarshal(record.PathStateJSON, &state); err != nil {
		return nil, fmt.Errorf("decode workspace event path state %d: %w", record.Revision, err)
	}
	if err := state.Validate(); err != nil {
		return nil, fmt.Errorf("validate workspace event path state %d: %w", record.Revision, err)
	}
	if state.PathRevision != record.Revision {
		return nil, fmt.Errorf("workspace event path revision mismatch at %d", record.Revision)
	}
	operationID, err := dto.ParseWorkspaceUUID("operationId", record.OperationID)
	if err != nil {
		return nil, err
	}
	originClientID, err := dto.ParseWorkspaceUUID("originClientId", record.OriginClientID)
	if err != nil {
		return nil, err
	}
	result := &domain.WorkspaceStoredEvent{
		Revision:       record.Revision,
		OperationID:    operationID,
		OriginClientID: originClientID,
		Mutation:       mutation,
		PathState:      state,
	}
	if len(record.OldPathStateJSON) != 0 {
		var oldState dto.WorkspacePathState
		if err := json.Unmarshal(record.OldPathStateJSON, &oldState); err != nil {
			return nil, err
		}
		result.OldPathState = &oldState
	}
	if len(record.NewPathStateJSON) != 0 {
		var newState dto.WorkspacePathState
		if err := json.Unmarshal(record.NewPathStateJSON, &newState); err != nil {
			return nil, err
		}
		result.NewPathState = &newState
	}
	wire := dto.WorkspaceEventMessage{
		WorkspaceID:    mutation.WorkspaceID,
		StreamID:       dto.WorkspaceUUID("30000000-0000-4000-8000-000000000001"),
		Index:          1,
		Revision:       record.Revision,
		OperationID:    operationID,
		OriginClientID: originClientID,
		Mutation:       mutation,
		PathState:      state,
		OldPathState:   result.OldPathState,
		NewPathState:   result.NewPathState,
	}
	if record.Revision == 0 {
		return nil, errors.New("workspace event revision must be positive")
	}
	if err := wire.Validate(0, record.Revision-1); err != nil {
		return nil, fmt.Errorf("validate workspace event %d: %w", record.Revision, err)
	}
	return result, nil
}

func (s *workspaceSyncService) Acknowledge(
	ctx context.Context,
	uid int64,
	req dto.WorkspaceAckRequest,
	lastDelivered dto.WorkspaceRevision,
) error {
	if s == nil {
		return errors.New("workspace sync service is nil")
	}
	if s.initErr != nil {
		return s.initErr
	}
	if err := req.Validate(0, lastDelivered); err != nil {
		return err
	}
	if err := s.repo.Migrate(ctx, uid); err != nil {
		return err
	}
	return s.repo.Write(ctx, uid, func(tx domain.WorkspaceWriteTx) error {
		workspace, err := tx.Workspace(string(req.WorkspaceID))
		if err != nil {
			return err
		}
		client, err := tx.Client(string(req.WorkspaceID), string(req.ClientID))
		if err != nil {
			return err
		}
		if req.Revision == client.LastAckRevision {
			if lastDelivered > workspace.GlobalRevision {
				return &WorkspaceServiceError{Code: dto.WorkspaceErrorInvalidRevision}
			}
			return nil
		}
		if err := req.Validate(client.LastAckRevision, lastDelivered); err != nil {
			return err
		}
		if lastDelivered > workspace.GlobalRevision {
			return &WorkspaceServiceError{Code: dto.WorkspaceErrorInvalidRevision}
		}
		client.LastAckRevision = req.Revision
		client.LastSeenAt = s.now()
		return tx.SaveClient(*client)
	})
}

func (s *workspaceSyncService) PruneUser(ctx context.Context, uid int64, now time.Time) error {
	if s == nil {
		return errors.New("workspace sync service is nil")
	}
	if s.initErr != nil {
		return s.initErr
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	if now.IsZero() {
		now = s.now()
	}
	if err := s.repo.Migrate(ctx, uid); err != nil {
		return err
	}
	err := s.repo.Write(ctx, uid, func(tx domain.WorkspaceWriteTx) error {
		var afterWorkspaceID int64
		for {
			workspaces, err := tx.WorkspacesPage(afterWorkspaceID, s.pruneBatchSize)
			if err != nil {
				return err
			}
			if len(workspaces) == 0 {
				break
			}
			for i := range workspaces {
				workspace := &workspaces[i]
				operations, operationErr := tx.ExpiredWaitingOperations(
					workspace.WorkspaceID, now, s.pruneBatchSize,
				)
				if operationErr != nil {
					return operationErr
				}
				for j := range operations {
					operation := operations[j]
					if operation.State != "waiting_blob" || operation.ExpiresAt == nil || now.Before(*operation.ExpiresAt) {
						continue
					}
					if err := workspaceConflictExpireWaiting(tx, &operation, now); err != nil {
						return err
					}
				}

				cutoff := now.Add(-s.eventRetention)
				countCutoff := dto.WorkspaceRevision(0)
				if workspace.GlobalRevision > dto.WorkspaceRevision(s.eventMaxPerWorkspace) {
					countCutoff = workspace.GlobalRevision - dto.WorkspaceRevision(s.eventMaxPerWorkspace)
				}
				deleteEvents, eventErr := tx.PrunableEvents(
					workspace.WorkspaceID, cutoff, countCutoff, s.pruneBatchSize,
				)
				if eventErr != nil {
					return eventErr
				}
				if len(deleteEvents) != 0 {
					floor := deleteEvents[len(deleteEvents)-1].Revision
					if floor > workspace.ReplayFloorRevision {
						workspace.ReplayFloorRevision = floor
					}
					workspace.UpdatedAt = now
					if err := tx.SaveWorkspace(*workspace); err != nil {
						return err
					}
					ids := make([]int64, 0, len(deleteEvents))
					for _, event := range deleteEvents {
						ids = append(ids, event.ID)
						if err := tx.RemoveBlobRefs("event", workspaceSyncEventOwnerKey(dto.WorkspaceUUID(workspace.WorkspaceID), event.Revision), now); err != nil {
							return err
						}
					}
					if err := tx.DeleteEvents(ids); err != nil {
						return err
					}
				}
				if err := workspacePruneTombstones(tx, workspace, now, s.pruneBatchSize); err != nil {
					return err
				}
				afterWorkspaceID = workspace.ID
			}
		}
		return nil
	})
	if err != nil {
		return err
	}
	return s.blobStore.ReconcileAndGC(ctx, uid, now)
}

func workspacePruneTombstones(
	tx domain.WorkspaceWriteTx,
	workspace *domain.WorkspaceRecord,
	now time.Time,
	pageSize int,
) error {
	paths, err := tx.Tombstones(
		workspace.WorkspaceID, workspace.ReplayFloorRevision, pageSize,
	)
	if err != nil {
		return err
	}
	for _, path := range paths {
		if !path.Tombstone || path.PathRevision > workspace.ReplayFloorRevision {
			continue
		}
		if _, conflictErr := tx.PendingConflict(workspace.WorkspaceID, path.Path); conflictErr == nil {
			continue
		} else if !errors.Is(conflictErr, domain.ErrWorkspaceRecordNotFound) {
			return conflictErr
		}
		eligible := true
		afterClientID := ""
		for eligible {
			clients, clientErr := tx.ClientsPage(workspace.WorkspaceID, afterClientID, pageSize)
			if clientErr != nil {
				return clientErr
			}
			for _, client := range clients {
				if client.LastAckRevision < path.PathRevision && client.LastAckRevision >= workspace.ReplayFloorRevision {
					eligible = false
					break
				}
				afterClientID = client.ClientID
			}
			if len(clients) < pageSize || !eligible {
				break
			}
		}
		if !eligible {
			continue
		}
		if err := tx.RemoveBlobRefs("path", workspaceSyncPathOwnerKey(dto.WorkspaceUUID(workspace.WorkspaceID), path.Path), now); err != nil {
			return err
		}
		if err := tx.DeletePath(path.ID); err != nil && !errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
			return err
		}
	}
	return nil
}
