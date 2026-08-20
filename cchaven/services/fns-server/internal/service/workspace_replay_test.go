package service

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"testing"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/model"
	"github.com/haierkeys/fast-note-sync-service/internal/testutil"
	"github.com/stretchr/testify/require"
)

func TestWorkspaceSyncPruneUsesConfiguredEventRetention(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	cfg := workspaceBlobStoreConfig(t, env.BlobRoot)
	cfg.EventRetention = "1h"
	store := NewWorkspaceBlobStore(env.WorkspaceRepo, cfg)
	service := NewWorkspaceSyncService(env.WorkspaceRepo, store, cfg)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	mutation := workspaceSyncPutMutation(t, ctx, env, service, "notes/retention.md", 0, "retention")
	outcome, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.NotNil(t, outcome.Accepted)

	old := time.Date(2026, time.August, 7, 0, 0, 0, 0, time.UTC)
	require.NoError(t, env.UserDB(env.UID).Model(&model.WorkspaceEvent{}).
		Where("workspace_id = ? AND revision = ?", string(workspaceSyncWorkspaceID), uint64(outcome.Accepted.Revision)).
		Update("created_at", old).Error)

	now := old.Add(2 * time.Hour)
	service.now = func() time.Time { return now }
	require.NoError(t, service.PruneUser(ctx, env.UID, now))
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, outcome.Accepted.Revision, workspace.ReplayFloorRevision)
		return nil
	}))
}

func TestWorkspaceSyncSubscribeReturnsContiguousMixedRevisionItems(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 500)
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictKeepCurrent, 504)
	resolved, err := fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	require.NoError(t, err)
	require.NotNil(t, resolved.Resolved)

	accepted := workspaceConflictCommitFile(
		t, fixture.ctx, fixture.env, fixture.service, "notes/after-resolve.md", 0, "after", 505,
	)
	changeSet, err := fixture.service.Subscribe(fixture.ctx, fixture.env.UID, dto.WorkspaceSubscribeRequest{
		WorkspaceID:     workspaceSyncWorkspaceID,
		ClientID:        workspaceSyncClientID,
		LastAckRevision: fixture.current.PathState.PathRevision,
	})
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceSnapshotIncremental, changeSet.Mode)
	require.Equal(t, accepted.Revision, changeSet.FinalRevision)
	require.Len(t, changeSet.RevisionItems, 2)
	require.NotNil(t, changeSet.RevisionItems[0].ConflictResolved)
	require.Equal(t, resolved.Resolved.Revision, changeSet.RevisionItems[0].Revision)
	require.NotNil(t, changeSet.RevisionItems[1].Event)
	require.Equal(t, accepted.Revision, changeSet.RevisionItems[1].Revision)
	require.Equal(t, uint32(2), changeSet.EventCount)
}

func TestWorkspaceSyncSnapshotIncludesAuthoritativePendingConflicts(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	ancestor := workspaceConflictCommitFile(t, ctx, env, service, "notes/pending.md", 0, "ancestor", 510)
	workspaceConflictCommitFile(t, ctx, env, service, "notes/pending.md", ancestor.PathState.PathRevision, "current", 511)
	incoming := workspaceConflictFileMutation(t, ctx, env, service, "notes/pending.md", ancestor.PathState.PathRevision, "incoming", 512)
	created, err := service.ApplyMutation(ctx, env.UID, incoming)
	require.NoError(t, err)
	require.NotNil(t, created.Conflict)

	changeSet, err := service.Subscribe(ctx, env.UID, dto.WorkspaceSubscribeRequest{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncClientID,
	})
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceSnapshotFull, changeSet.Mode)
	require.Equal(t, dto.WorkspaceRevision(2), changeSet.FinalRevision)
	require.Equal(t, uint32(1), changeSet.ConflictCount)
	require.NotNil(t, changeSet.PendingConflicts)
	pending, err := changeSet.PendingConflicts.Next(ctx)
	require.NoError(t, err)
	require.Equal(t, created.Conflict, pending)
	end, err := changeSet.PendingConflicts.Next(ctx)
	require.NoError(t, err)
	require.Nil(t, end)
}

func TestWorkspaceSyncPendingConflictCursorStaysOnOneReadSnapshot(t *testing.T) {
	env, baseService := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, baseService)
	ancestor := workspaceConflictCommitFile(t, ctx, env, baseService, "notes/paged.md", 0, "ancestor", 540)
	workspaceConflictCommitFile(t, ctx, env, baseService, "notes/paged.md", ancestor.PathState.PathRevision, "current", 541)
	incoming := workspaceConflictFileMutation(t, ctx, env, baseService, "notes/paged.md", ancestor.PathState.PathRevision, "incoming", 542)
	created, err := baseService.ApplyMutation(ctx, env.UID, incoming)
	require.NoError(t, err)
	require.NotNil(t, created.Conflict)

	var base model.WorkspaceConflict
	require.NoError(t, env.UserDB(env.UID).Where(
		"workspace_id = ? AND conflict_id = ?",
		string(workspaceSyncWorkspaceID), string(created.Conflict.ConflictID),
	).Take(&base).Error)
	duplicates := make([]model.WorkspaceConflict, 0, workspacePendingConflictPageSize)
	for index := 1; index <= workspacePendingConflictPageSize; index++ {
		copy := base
		copy.ConflictID = fmt.Sprintf("71000000-0000-4000-8000-%012d", index)
		duplicates = append(duplicates, copy)
	}
	require.NoError(t, env.UserDB(env.UID).Create(&duplicates).Error)

	guard := &workspaceReplayReadGuard{WorkspaceRepository: env.WorkspaceRepo}
	service := NewWorkspaceSyncService(guard, baseService.blobStore)
	changeSet, err := service.Subscribe(ctx, env.UID, dto.WorkspaceSubscribeRequest{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncClientID,
	})
	require.NoError(t, err)
	require.Equal(t, uint32(workspacePendingConflictPageSize+1), changeSet.ConflictCount)
	guard.rejectReads = true

	for index := uint32(0); index < changeSet.ConflictCount; index++ {
		conflict, nextErr := changeSet.PendingConflicts.Next(ctx)
		require.NoError(t, nextErr)
		require.NotNil(t, conflict)
	}
	conflict, err := changeSet.PendingConflicts.Next(ctx)
	require.NoError(t, err)
	require.Nil(t, conflict)
}

type workspaceReplayReadGuard struct {
	domain.WorkspaceRepository
	rejectReads bool
}

func (r *workspaceReplayReadGuard) Read(
	ctx context.Context,
	uid int64,
	fn func(domain.WorkspaceReadTx) error,
) error {
	if r.rejectReads {
		return errors.New("unexpected workspace replay read after snapshot creation")
	}
	return r.WorkspaceRepository.Read(ctx, uid, fn)
}

func TestWorkspaceSyncSubscribeReturnsEmptyIncrementalAtFinalRevision(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	accepted := workspaceSyncPutMutation(t, ctx, env, service, "notes/final.md", 0, "final")
	outcome, err := service.ApplyMutation(ctx, env.UID, accepted)
	require.NoError(t, err)

	changeSet, err := service.Subscribe(ctx, env.UID, dto.WorkspaceSubscribeRequest{
		WorkspaceID:     workspaceSyncWorkspaceID,
		ClientID:        workspaceSyncClientID,
		LastAckRevision: outcome.Accepted.Revision,
	})
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceSnapshotIncremental, changeSet.Mode)
	require.Equal(t, outcome.Accepted.Revision, changeSet.FromRevision)
	require.Equal(t, outcome.Accepted.Revision, changeSet.FinalRevision)
	require.Empty(t, changeSet.RevisionItems)
	require.Zero(t, changeSet.EventCount)
}

func TestWorkspaceSyncSnapshotUsesUTF8BytePathOrder(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	workspaceReplayCommitFile(t, ctx, env, service, "z.md", "z", "60000000-0000-4000-8000-000000000001")
	workspaceReplayCommitFile(t, ctx, env, service, "a.md", "a", "60000000-0000-4000-8000-000000000002")
	changeSet, err := service.Subscribe(ctx, env.UID, dto.WorkspaceSubscribeRequest{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncClientID,
	})
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceSnapshotFull, changeSet.Mode)
	require.Len(t, changeSet.Entries, 2)
	require.Equal(t, dto.WorkspacePath("a.md"), changeSet.Entries[0].Path)
	require.Equal(t, dto.WorkspacePath("z.md"), changeSet.Entries[1].Path)
}

func workspaceReplayCommitFile(
	t *testing.T,
	ctx context.Context,
	env *testutil.WorkspaceEnv,
	service *workspaceSyncService,
	path dto.WorkspacePath,
	content string,
	operationID dto.WorkspaceUUID,
) {
	t.Helper()
	hash := workspaceBlobStoreHash([]byte(content))
	require.NoError(t, service.blobStore.Put(ctx, env.UID, hash, uint64(len(content)), bytes.NewBufferString(content)))
	outcome, err := service.ApplyMutation(ctx, env.UID, dto.WorkspaceMutation{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncClientID,
		OperationID: operationID,
		Path:        path,
		Kind:        dto.WorkspaceMutationUpsertFile,
		ContentHash: workspaceSyncNullableHash(hash),
		Metadata:    dto.WorkspaceFileMetadata{Size: uint64(len(content))},
	})
	require.NoError(t, err)
	require.NotNil(t, outcome.Accepted)
}

func TestWorkspaceSyncAcknowledgeMovesForwardOnlyAndCannotExceedDeliveredEnd(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	accepted := workspaceSyncPutMutation(t, ctx, env, service, "notes/ack.md", 0, "ack")
	outcome, err := service.ApplyMutation(ctx, env.UID, accepted)
	require.NoError(t, err)

	require.NoError(t, service.Acknowledge(ctx, env.UID, dto.WorkspaceAckRequest{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncClientID,
		Revision:    outcome.Accepted.Revision,
	}, outcome.Accepted.Revision))
	err = service.Acknowledge(ctx, env.UID, dto.WorkspaceAckRequest{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncClientID,
		Revision:    outcome.Accepted.Revision + 1,
	}, outcome.Accepted.Revision)
	var validationErr *dto.WorkspaceValidationError
	require.ErrorAs(t, err, &validationErr)
	require.Equal(t, "revision", validationErr.Field)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		client, readErr := tx.Client(string(workspaceSyncWorkspaceID), string(workspaceSyncClientID))
		require.NoError(t, readErr)
		require.Equal(t, outcome.Accepted.Revision, client.LastAckRevision)
		return nil
	}))
}

func TestWorkspaceSyncPruneExpiresOnlyWaitingResolvePayloadAt24Hours(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 520)
	now := time.Date(2026, time.August, 8, 0, 0, 0, 0, time.UTC)
	fixture.service.now = func() time.Time { return now }
	content := []byte("waiting prune")
	hash := workspaceBlobStoreHash(content)
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictUseMerged, 524)
	request.ContentHash = workspaceSyncNullableHash(hash)
	request.Metadata = dto.WorkspaceFileMetadata{Size: uint64(len(content))}
	_, err := fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorBlobRequired)

	require.NoError(t, fixture.service.PruneUser(fixture.ctx, fixture.env.UID, now.Add(24*time.Hour)))
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		operation, readErr := tx.Operation(string(request.ClientID), string(request.OperationID))
		require.NoError(t, readErr)
		require.Equal(t, "expired_guard", operation.State)
		require.Nil(t, operation.RequiredHash)
		require.Nil(t, operation.ConflictRevision)
		conflict, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID))
		require.NoError(t, readErr)
		require.Equal(t, "pending", conflict.Status)
		return nil
	}))
}

func TestWorkspaceSyncPruneKeepsPendingConflictRefs(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 530)
	require.NoError(t, fixture.service.PruneUser(fixture.ctx, fixture.env.UID, time.Now().UTC().Add(365*24*time.Hour)))
	var count int64
	require.NoError(t, fixture.env.UserDB(fixture.env.UID).Raw(
		"SELECT COUNT(*) FROM workspace_blob_ref WHERE owner_type = ? AND owner_key = ?",
		"conflict", string(fixture.created.ConflictID),
	).Scan(&count).Error)
	require.NotZero(t, count)
}
