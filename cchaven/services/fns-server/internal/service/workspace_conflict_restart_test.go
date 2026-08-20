package service

import (
	"bytes"
	"io"
	"testing"

	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/stretchr/testify/require"
)

func TestWorkspaceSyncResolveMergedRetryAfterServiceRestartCommitsOnce(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 600)
	content := []byte("merged content surviving service restart")
	hash := workspaceBlobStoreHash(content)
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictUseMerged, 604)
	request.ContentHash = workspaceSyncNullableHash(hash)
	request.Metadata = dto.WorkspaceFileMetadata{Size: uint64(len(content)), ModifiedAtMS: 604}

	_, err := fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorBlobRequired)
	waiting := workspaceConflictReadOperation(t, fixture, request)
	require.Equal(t, "waiting_blob", waiting.State)

	restartedStore := NewWorkspaceBlobStore(
		fixture.env.WorkspaceRepo,
		workspaceBlobStoreConfig(t, fixture.env.BlobRoot),
	)
	restarted := NewWorkspaceSyncService(fixture.env.WorkspaceRepo, restartedStore)
	_, err = restarted.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorBlobRequired)
	workspaceSyncRequireSameOperationRecord(t, waiting, workspaceConflictReadOperation(t, fixture, request))

	require.NoError(t, restartedStore.Put(
		fixture.ctx,
		fixture.env.UID,
		hash,
		uint64(len(content)),
		bytes.NewReader(content),
	))

	afterUpload := NewWorkspaceSyncService(
		fixture.env.WorkspaceRepo,
		NewWorkspaceBlobStore(fixture.env.WorkspaceRepo, workspaceBlobStoreConfig(t, fixture.env.BlobRoot)),
	)
	first, err := afterUpload.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	require.NoError(t, err)
	require.NotNil(t, first.Resolved)
	require.Equal(t, request.OperationID, first.Resolved.OperationID)
	require.Equal(t, hash, *first.Resolved.PathState.ContentHash.Value)

	replayStore := NewWorkspaceBlobStore(
		fixture.env.WorkspaceRepo,
		workspaceBlobStoreConfig(t, fixture.env.BlobRoot),
	)
	replayed, err := NewWorkspaceSyncService(fixture.env.WorkspaceRepo, replayStore).
		ResolveConflict(fixture.ctx, fixture.env.UID, request)
	require.NoError(t, err)
	require.Equal(t, first.Resolved, replayed.Resolved)

	reader, size, err := replayStore.Open(fixture.ctx, fixture.env.UID, hash)
	require.NoError(t, err)
	require.Equal(t, uint64(len(content)), size)
	actual, err := io.ReadAll(reader)
	require.NoError(t, err)
	require.NoError(t, reader.Close())
	require.Equal(t, content, actual)

	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, first.Resolved.Revision, workspace.GlobalRevision)

		conflict, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(request.ConflictID))
		require.NoError(t, readErr)
		require.Equal(t, "resolved", conflict.Status)
		require.Equal(t, string(request.OperationID), *conflict.ResolutionOperationID)
		require.Equal(t, first.Resolved.Revision, *conflict.ResolutionRevision)

		operation, readErr := tx.Operation(string(request.ClientID), string(request.OperationID))
		require.NoError(t, readErr)
		require.Equal(t, "terminal", operation.State)
		require.Nil(t, operation.RequiredHash)
		require.Nil(t, operation.ExpiresAt)

		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, workspace.GlobalRevision)
		require.NoError(t, readErr)
		resolvedCount := 0
		for _, event := range events {
			if event.Kind == "conflict_resolved" {
				resolvedCount++
				require.Equal(t, first.Resolved.Revision, event.Revision)
				require.Equal(t, string(request.OperationID), event.OperationID)
			}
		}
		require.Equal(t, 1, resolvedCount)
		return nil
	}))
}
