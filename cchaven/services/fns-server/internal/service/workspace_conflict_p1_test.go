package service

import (
	"context"
	"testing"

	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/testutil"
	"github.com/stretchr/testify/require"
)

func TestWorkspaceSyncStaleUpsertMissingBlobReturnsWaitingWithoutConflict(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	ancestor := workspaceConflictCommitFile(t, ctx, env, service, "notes/stale-missing.md", 0, "ancestor", 501)
	workspaceConflictCommitFile(t, ctx, env, service, ancestor.PathState.Path, ancestor.PathState.PathRevision, "current", 502)

	content := "stale missing blob"
	hash := workspaceBlobStoreHash([]byte(content))
	mutation := dto.WorkspaceMutation{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      workspaceConflictOperationID(503),
		Path:             ancestor.PathState.Path,
		BasePathRevision: ancestor.PathState.PathRevision,
		Kind:             dto.WorkspaceMutationUpsertFile,
		ContentHash:      workspaceSyncNullableHash(hash),
		Metadata:         dto.WorkspaceFileMetadata{Size: uint64(len(content))},
	}

	outcome, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.NotNil(t, outcome.Rejected)
	require.Equal(t, dto.WorkspaceMutationRejectBlobRequired, outcome.Rejected.Reason)
	require.Nil(t, outcome.Conflict)
	require.NotNil(t, outcome.RequiredUpload)
	require.Equal(t, hash, outcome.RequiredUpload.ContentHash)
	require.Equal(t, uint64(len(content)), outcome.RequiredUpload.Size)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		operation, readErr := tx.Operation(string(mutation.ClientID), string(mutation.OperationID))
		require.NoError(t, readErr)
		require.Equal(t, "waiting_blob", operation.State)
		require.Equal(t, &hash, operation.RequiredHash)
		_, readErr = tx.PendingConflict(string(mutation.WorkspaceID), mutation.Path)
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		return nil
	}))
}

func TestWorkspaceSyncStaleUpsertSizeMismatchReturnsWaitingWithoutConflict(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	ancestor := workspaceConflictCommitFile(t, ctx, env, service, "notes/stale-size.md", 0, "ancestor", 511)
	workspaceConflictCommitFile(t, ctx, env, service, ancestor.PathState.Path, ancestor.PathState.PathRevision, "current", 512)

	content := "stale size mismatch"
	mutation := workspaceSyncPutMutation(t, ctx, env, service, ancestor.PathState.Path, ancestor.PathState.PathRevision, content)
	mutation.OperationID = workspaceConflictOperationID(513)
	mutation.Metadata.Size++
	hash := *mutation.ContentHash.Value

	outcome, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.NotNil(t, outcome.Rejected)
	require.Equal(t, dto.WorkspaceMutationRejectBlobRequired, outcome.Rejected.Reason)
	require.Nil(t, outcome.Conflict)
	require.NotNil(t, outcome.RequiredUpload)
	require.Equal(t, hash, outcome.RequiredUpload.ContentHash)
	require.Equal(t, mutation.Metadata.Size, outcome.RequiredUpload.Size)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		operation, readErr := tx.Operation(string(mutation.ClientID), string(mutation.OperationID))
		require.NoError(t, readErr)
		require.Equal(t, "waiting_blob", operation.State)
		require.Equal(t, &hash, operation.RequiredHash)
		_, readErr = tx.PendingConflict(string(mutation.WorkspaceID), mutation.Path)
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		return nil
	}))
}

func TestWorkspaceSyncResolveDirectoryRenameConflictMovesDescendants(t *testing.T) {
	fixture := workspaceConflictDirectoryRenameFixture(t, 520)
	request := workspaceConflictResolutionRequest(
		t,
		&workspaceConflictFixture{created: fixture.created},
		dto.WorkspaceConflictUseIncoming,
		530,
	)

	resolved, err := fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	require.NoError(t, err)
	require.NotNil(t, resolved.Resolved)
	require.Equal(t, fixture.target, resolved.Resolved.PathState.Path)
	require.Equal(t, dto.WorkspaceRevision(6), resolved.Resolved.Revision)

	wantMoves := map[dto.WorkspacePath]dto.WorkspacePath{
		"docs":             "archive",
		"docs/a.md":        "archive/a.md",
		"docs/nested":      "archive/nested",
		"docs/nested/b.md": "archive/nested/b.md",
	}
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		for oldPath, newPath := range wantMoves {
			oldRecord, readErr := tx.Path(string(workspaceSyncWorkspaceID), oldPath)
			require.NoError(t, readErr)
			require.True(t, oldRecord.Tombstone)
			require.Equal(t, dto.WorkspaceRevision(6), oldRecord.PathRevision)
			newRecord, readErr := tx.Path(string(workspaceSyncWorkspaceID), newPath)
			require.NoError(t, readErr)
			require.False(t, newRecord.Tombstone)
			require.Equal(t, dto.WorkspaceRevision(6), newRecord.PathRevision)
		}
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, int64(4), workspace.LivePathCount)
		require.Equal(t, uint64(3), workspace.LiveBytes)
		return nil
	}))
}

func TestWorkspaceSyncResolveDirectoryRenameConflictRejectsDescendantDriftWithoutPartialWrites(t *testing.T) {
	fixture := workspaceConflictDirectoryRenameFixture(t, 540)
	drift := workspaceSyncPutMutation(
		t,
		fixture.ctx,
		fixture.env,
		fixture.service,
		fixture.child.PathState.Path,
		fixture.child.PathState.PathRevision,
		"drifted child",
	)
	drift.OperationID = workspaceConflictOperationID(550)
	driftOutcome, err := fixture.service.ApplyMutation(fixture.ctx, fixture.env.UID, drift)
	require.NoError(t, err)
	require.NotNil(t, driftOutcome.Accepted)

	request := workspaceConflictResolutionRequest(
		t,
		&workspaceConflictFixture{created: fixture.created},
		dto.WorkspaceConflictUseIncoming,
		551,
	)
	_, err = fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorConflictRevisionStale)

	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(6), workspace.GlobalRevision)
		source, readErr := tx.Path(string(workspaceSyncWorkspaceID), "docs")
		require.NoError(t, readErr)
		require.False(t, source.Tombstone)
		_, readErr = tx.Path(string(workspaceSyncWorkspaceID), fixture.target)
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		conflict, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID))
		require.NoError(t, readErr)
		require.Equal(t, "pending", conflict.Status)
		return nil
	}))
}

func TestWorkspaceSyncResolveDirectoryRenameConflictRejectsDescendantCollisionWithoutPartialWrites(t *testing.T) {
	fixture := workspaceConflictDirectoryRenameFixture(t, 560)
	collision := workspaceSyncPutMutation(
		t,
		fixture.ctx,
		fixture.env,
		fixture.service,
		"archive/a.md",
		0,
		"collision",
	)
	collision.OperationID = workspaceConflictOperationID(570)
	collisionOutcome, err := fixture.service.ApplyMutation(fixture.ctx, fixture.env.UID, collision)
	require.NoError(t, err)
	require.NotNil(t, collisionOutcome.Accepted)

	request := workspaceConflictResolutionRequest(
		t,
		&workspaceConflictFixture{created: fixture.created},
		dto.WorkspaceConflictUseIncoming,
		571,
	)
	_, err = fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	require.ErrorContains(t, err, "workspace directory rename destination collision at archive/a.md")

	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(6), workspace.GlobalRevision)
		source, readErr := tx.Path(string(workspaceSyncWorkspaceID), "docs")
		require.NoError(t, readErr)
		require.False(t, source.Tombstone)
		target, readErr := tx.Path(string(workspaceSyncWorkspaceID), "archive")
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		require.Nil(t, target)
		storedCollision, readErr := tx.Path(string(workspaceSyncWorkspaceID), collision.Path)
		require.NoError(t, readErr)
		require.False(t, storedCollision.Tombstone)
		conflict, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID))
		require.NoError(t, readErr)
		require.Equal(t, "pending", conflict.Status)
		return nil
	}))
}

type workspaceDirectoryRenameFixture struct {
	ctx     context.Context
	env     *testutil.WorkspaceEnv
	service *workspaceSyncService
	created *dto.WorkspaceConflictCreatedMessage
	child   *dto.WorkspaceMutationAcceptedMessage
	target  dto.WorkspacePath
}

func workspaceConflictDirectoryRenameFixture(t *testing.T, operationBase int) *workspaceDirectoryRenameFixture {
	t.Helper()
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)

	root := workspaceSyncMkdirMutation("docs", 0, operationBase)
	rootOutcome, err := service.ApplyMutation(ctx, env.UID, root)
	require.NoError(t, err)
	require.NotNil(t, rootOutcome.Accepted)

	child := workspaceSyncPutMutation(t, ctx, env, service, "docs/a.md", 0, "a")
	child.OperationID = workspaceConflictOperationID(operationBase + 1)
	childOutcome, err := service.ApplyMutation(ctx, env.UID, child)
	require.NoError(t, err)
	require.NotNil(t, childOutcome.Accepted)

	nested := workspaceSyncMkdirMutation("docs/nested", 0, operationBase+2)
	nestedOutcome, err := service.ApplyMutation(ctx, env.UID, nested)
	require.NoError(t, err)
	require.NotNil(t, nestedOutcome.Accepted)

	grandchild := workspaceSyncPutMutation(t, ctx, env, service, "docs/nested/b.md", 0, "bb")
	grandchild.OperationID = workspaceConflictOperationID(operationBase + 3)
	grandchildOutcome, err := service.ApplyMutation(ctx, env.UID, grandchild)
	require.NoError(t, err)
	require.NotNil(t, grandchildOutcome.Accepted)

	refresh := workspaceSyncMkdirMutation("docs", rootOutcome.Accepted.PathState.PathRevision, operationBase+4)
	refreshOutcome, err := service.ApplyMutation(ctx, env.UID, refresh)
	require.NoError(t, err)
	require.NotNil(t, refreshOutcome.Accepted)

	target := dto.WorkspacePath("archive")
	targetBase := dto.WorkspaceRevision(0)
	rename := dto.WorkspaceMutation{
		WorkspaceID:            workspaceSyncWorkspaceID,
		ClientID:               workspaceSyncClientID,
		OperationID:            workspaceConflictOperationID(operationBase + 5),
		Path:                   root.Path,
		BasePathRevision:       rootOutcome.Accepted.PathState.PathRevision,
		Kind:                   dto.WorkspaceMutationRename,
		ContentHash:            refresh.ContentHash,
		Metadata:               refresh.Metadata,
		NewPath:                &target,
		TargetBasePathRevision: &targetBase,
	}
	createdOutcome, err := service.ApplyMutation(ctx, env.UID, rename)
	require.NoError(t, err)
	require.NotNil(t, createdOutcome.Conflict)
	require.Equal(t, dto.WorkspaceConflictRename, createdOutcome.Conflict.Kind)

	return &workspaceDirectoryRenameFixture{
		ctx: ctx, env: env, service: service, created: createdOutcome.Conflict,
		child: childOutcome.Accepted, target: target,
	}
}
