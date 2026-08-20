package service

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"testing"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/testutil"
	"github.com/stretchr/testify/require"
)

func TestWorkspaceSyncStaleDifferentContentCreatesConflictWithoutChangingCurrentContent(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)

	ancestor := workspaceConflictCommitFile(t, ctx, env, service, "notes/conflict.md", 0, "ancestor", 101)
	current := workspaceConflictCommitFile(t, ctx, env, service, "notes/conflict.md", ancestor.PathState.PathRevision, "current", 102)
	incoming := workspaceConflictFileMutation(t, ctx, env, service, "notes/conflict.md", ancestor.PathState.PathRevision, "incoming", 103)

	outcome, err := service.ApplyMutation(ctx, env.UID, incoming)
	require.NoError(t, err)
	require.NotNil(t, outcome.Rejected)
	require.Equal(t, dto.WorkspaceMutationRejectConflictCreated, outcome.Rejected.Reason)
	require.NotNil(t, outcome.Conflict)
	require.Equal(t, dto.WorkspaceConflictContent, outcome.Conflict.Kind)
	require.NotEqual(t, dto.WorkspaceConflictRevision{}, outcome.Conflict.ConflictRevision)
	require.Equal(t, outcome.Rejected.ConflictID, &outcome.Conflict.ConflictID)
	require.Equal(t, workspaceConflictSideFromState(ancestor.PathState), outcome.Conflict.Ancestor)
	require.Equal(t, workspaceConflictSideFromState(current.PathState), outcome.Conflict.Current)
	require.Equal(t, incoming.Path, *outcome.Conflict.Incoming.Path)
	require.Equal(t, incoming.BasePathRevision, outcome.Conflict.Incoming.PathRevision)
	require.Equal(t, incoming.ContentHash, outcome.Conflict.Incoming.ContentHash)
	require.Equal(t, incoming.Metadata, outcome.Conflict.Incoming.Metadata)
	require.NoError(t, outcome.Conflict.Validate())
	authoritative, err := service.CurrentPendingConflict(ctx, env.UID, outcome.Conflict.WorkspaceID, outcome.Conflict.ConflictID)
	require.NoError(t, err)
	require.Equal(t, outcome.Conflict, authoritative)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(2), workspace.GlobalRevision)
		path, readErr := tx.Path(string(workspaceSyncWorkspaceID), incoming.Path)
		require.NoError(t, readErr)
		require.Equal(t, current.PathState, workspaceSyncStateFromRecord(*path))
		stored, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(outcome.Conflict.ConflictID))
		require.NoError(t, readErr)
		require.Equal(t, "pending", stored.Status)
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, workspace.GlobalRevision)
		require.NoError(t, readErr)
		require.Len(t, events, 2)
		operation, readErr := tx.Operation(string(incoming.ClientID), string(incoming.OperationID))
		require.NoError(t, readErr)
		require.Equal(t, "terminal", operation.State)
		return nil
	}))
}

func TestWorkspaceSyncStaleDeleteCreatesDeleteModifyConflict(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)

	ancestor := workspaceConflictCommitFile(t, ctx, env, service, "notes/delete.md", 0, "ancestor", 111)
	current := workspaceConflictCommitFile(t, ctx, env, service, "notes/delete.md", ancestor.PathState.PathRevision, "current", 112)
	mutation := dto.WorkspaceMutation{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      workspaceConflictOperationID(113),
		Path:             "notes/delete.md",
		BasePathRevision: ancestor.PathState.PathRevision,
		Kind:             dto.WorkspaceMutationDelete,
		ContentHash:      dto.WorkspaceNullableHash{Present: true},
		Metadata:         dto.WorkspaceFileMetadata{},
	}

	outcome, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceMutationRejectConflictCreated, outcome.Rejected.Reason)
	require.NotNil(t, outcome.Conflict)
	require.Equal(t, dto.WorkspaceConflictDeleteModify, outcome.Conflict.Kind)
	require.Equal(t, workspaceConflictSideFromState(ancestor.PathState), outcome.Conflict.Ancestor)
	require.Equal(t, workspaceConflictSideFromState(current.PathState), outcome.Conflict.Current)
	require.Equal(t, dto.WorkspaceConflictSide{
		PathRevision: mutation.BasePathRevision,
		ContentHash:  dto.WorkspaceNullableHash{Present: true},
		Metadata:     dto.WorkspaceFileMetadata{},
		Tombstone:    true,
	}, outcome.Conflict.Incoming)
	require.NoError(t, outcome.Conflict.Validate())
}

func TestWorkspaceSyncStaleRenameCreatesRenameConflict(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)

	source := workspaceConflictCommitFile(t, ctx, env, service, "notes/source.md", 0, "source", 121)
	target := workspaceConflictCommitFile(t, ctx, env, service, "notes/target.md", 0, "target", 122)
	targetBase := dto.WorkspaceRevision(0)
	targetPath := target.PathState.Path
	mutation := dto.WorkspaceMutation{
		WorkspaceID:            workspaceSyncWorkspaceID,
		ClientID:               workspaceSyncClientID,
		OperationID:            workspaceConflictOperationID(123),
		Path:                   source.PathState.Path,
		BasePathRevision:       source.PathState.PathRevision,
		Kind:                   dto.WorkspaceMutationRename,
		ContentHash:            source.PathState.ContentHash,
		Metadata:               source.PathState.Metadata,
		NewPath:                &targetPath,
		TargetBasePathRevision: &targetBase,
	}

	outcome, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceMutationRejectConflictCreated, outcome.Rejected.Reason)
	require.NotNil(t, outcome.Conflict)
	require.Equal(t, dto.WorkspaceConflictRename, outcome.Conflict.Kind)
	require.Equal(t, workspaceConflictSideFromState(source.PathState), outcome.Conflict.Ancestor)
	require.Equal(t, workspaceConflictSideFromState(source.PathState), outcome.Conflict.Current)
	require.Equal(t, &targetPath, outcome.Conflict.Incoming.Path)
	require.Equal(t, targetBase, outcome.Conflict.Incoming.PathRevision)
	require.Equal(t, source.PathState.ContentHash, outcome.Conflict.Incoming.ContentHash)
	require.Equal(t, source.PathState.Metadata, outcome.Conflict.Incoming.Metadata)
	require.NoError(t, outcome.Conflict.Validate())

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		storedSource, readErr := tx.Path(string(workspaceSyncWorkspaceID), source.PathState.Path)
		require.NoError(t, readErr)
		require.Equal(t, source.PathState, workspaceSyncStateFromRecord(*storedSource))
		storedTarget, readErr := tx.Path(string(workspaceSyncWorkspaceID), target.PathState.Path)
		require.NoError(t, readErr)
		require.Equal(t, target.PathState, workspaceSyncStateFromRecord(*storedTarget))
		return nil
	}))
}

func TestWorkspaceSyncConflictCreationDoesNotPersistRevisionItem(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)

	source := workspaceConflictCommitFile(t, ctx, env, service, "notes/event-source.md", 0, "source", 161)
	target := workspaceConflictCommitFile(t, ctx, env, service, "notes/event-target.md", 0, "target", 162)
	targetBase := dto.WorkspaceRevision(0)
	targetPath := target.PathState.Path
	rejectedRename := dto.WorkspaceMutation{
		WorkspaceID:            workspaceSyncWorkspaceID,
		ClientID:               workspaceSyncClientID,
		OperationID:            workspaceConflictOperationID(163),
		Path:                   source.PathState.Path,
		BasePathRevision:       source.PathState.PathRevision,
		Kind:                   dto.WorkspaceMutationRename,
		ContentHash:            source.PathState.ContentHash,
		Metadata:               source.PathState.Metadata,
		NewPath:                &targetPath,
		TargetBasePathRevision: &targetBase,
	}
	outcome, err := service.ApplyMutation(ctx, env.UID, rejectedRename)
	require.NoError(t, err)
	require.NotNil(t, outcome.Conflict)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, workspace.GlobalRevision)
		require.NoError(t, readErr)
		require.Len(t, events, 2)
		sourceRow, readErr := tx.Path(string(workspaceSyncWorkspaceID), source.PathState.Path)
		require.NoError(t, readErr)
		require.Equal(t, source.PathState, workspaceSyncStateFromRecord(*sourceRow))
		targetRow, readErr := tx.Path(string(workspaceSyncWorkspaceID), target.PathState.Path)
		require.NoError(t, readErr)
		require.Equal(t, target.PathState, workspaceSyncStateFromRecord(*targetRow))
		return nil
	}))
}

func TestWorkspaceSyncInvalidUTF8BlobCreatesBinaryConflict(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)

	ancestor := workspaceConflictCommitFile(t, ctx, env, service, "notes/binary.dat", 0, "ancestor", 131)
	workspaceConflictCommitFile(t, ctx, env, service, "notes/binary.dat", ancestor.PathState.PathRevision, "current", 132)
	content := []byte{0xff, 0xfe, 0xfd}
	hash := workspaceBlobStoreHash(content)
	require.NoError(t, service.blobStore.Put(ctx, env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
	mutation := dto.WorkspaceMutation{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      workspaceConflictOperationID(133),
		Path:             "notes/binary.dat",
		BasePathRevision: ancestor.PathState.PathRevision,
		Kind:             dto.WorkspaceMutationUpsertFile,
		ContentHash:      workspaceSyncNullableHash(hash),
		Metadata:         dto.WorkspaceFileMetadata{Size: uint64(len(content))},
	}

	outcome, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceMutationRejectConflictCreated, outcome.Rejected.Reason)
	require.NotNil(t, outcome.Conflict)
	require.Equal(t, dto.WorkspaceConflictBinary, outcome.Conflict.Kind)
	require.NoError(t, outcome.Conflict.Validate())
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		blob, readErr := tx.Blob(hash)
		require.NoError(t, readErr)
		require.False(t, blob.UTF8Valid)
		return nil
	}))
}

func TestWorkspaceSyncConflictCreationStoresAncestorCurrentIncomingRefsAtomically(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)

	ancestor := workspaceConflictCommitFile(t, ctx, env, service, "notes/atomic.md", 0, "ancestor-ref", 141)
	current := workspaceConflictCommitFile(t, ctx, env, service, "notes/atomic.md", ancestor.PathState.PathRevision, "current-ref", 142)
	incoming := workspaceConflictFileMutation(t, ctx, env, service, "notes/atomic.md", ancestor.PathState.PathRevision, "incoming-ref", 143)
	ancestorHash := *ancestor.PathState.ContentHash.Value
	currentHash := *current.PathState.ContentHash.Value
	incomingHash := *incoming.ContentHash.Value

	sentinel := errors.New("rollback conflict transaction")
	failing := NewWorkspaceSyncService(
		&workspaceSyncFailAfterCallbackRepository{WorkspaceRepository: env.WorkspaceRepo, err: sentinel},
		service.blobStore,
	)
	_, err := failing.ApplyMutation(ctx, env.UID, incoming)
	require.ErrorIs(t, err, sentinel)
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(2), workspace.GlobalRevision)
		operation, readErr := tx.Operation(string(incoming.ClientID), string(incoming.OperationID))
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		require.Nil(t, operation)
		return nil
	}))

	outcome, err := service.ApplyMutation(ctx, env.UID, incoming)
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceMutationRejectConflictCreated, outcome.Rejected.Reason)
	require.NotNil(t, outcome.Conflict)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		stored, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(outcome.Conflict.ConflictID))
		require.NoError(t, readErr)
		var side dto.WorkspaceConflictSide
		require.NoError(t, json.Unmarshal(stored.AncestorJSON, &side))
		require.Equal(t, outcome.Conflict.Ancestor, side)
		require.NoError(t, json.Unmarshal(stored.CurrentJSON, &side))
		require.Equal(t, outcome.Conflict.Current, side)
		require.NoError(t, json.Unmarshal(stored.IncomingJSON, &side))
		require.Equal(t, outcome.Conflict.Incoming, side)
		return nil
	}))

	type storedRef struct {
		ContentHash string `gorm:"column:content_hash"`
		OwnerKey    string `gorm:"column:owner_key"`
	}
	var refs []storedRef
	require.NoError(t, env.UserDB(env.UID).Raw(
		"SELECT content_hash, owner_key FROM workspace_blob_ref WHERE owner_type = ? ORDER BY content_hash",
		"conflict",
	).Scan(&refs).Error)
	require.Len(t, refs, 3)
	require.ElementsMatch(t, []string{string(ancestorHash), string(currentHash), string(incomingHash)}, []string{
		refs[0].ContentHash, refs[1].ContentHash, refs[2].ContentHash,
	})
	require.Equal(t, refs[0].OwnerKey, refs[1].OwnerKey)
	require.Equal(t, refs[0].OwnerKey, refs[2].OwnerKey)
	require.Equal(t, string(outcome.Conflict.ConflictID), refs[0].OwnerKey)
}

func TestWorkspaceSyncConflictCreationExactOperationReplayReturnsSameConflictIDAndRevision(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)

	ancestor := workspaceConflictCommitFile(t, ctx, env, service, "notes/replay.md", 0, "ancestor", 151)
	workspaceConflictCommitFile(t, ctx, env, service, "notes/replay.md", ancestor.PathState.PathRevision, "current", 152)
	mutation := workspaceConflictFileMutation(t, ctx, env, service, "notes/replay.md", ancestor.PathState.PathRevision, "incoming", 153)

	first, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.NotNil(t, first.Conflict)
	replayed, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.NotNil(t, replayed.Conflict)
	require.Equal(t, first.Rejected, replayed.Rejected)
	require.Equal(t, first.Conflict, replayed.Conflict)
	require.Equal(t, first.Conflict.ConflictID, replayed.Conflict.ConflictID)
	require.Equal(t, first.Conflict.ConflictRevision, replayed.Conflict.ConflictRevision)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(2), workspace.GlobalRevision)
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, workspace.GlobalRevision)
		require.NoError(t, readErr)
		require.Len(t, events, 2)
		return nil
	}))
}

func TestWorkspaceSyncTerminalConflictMutationReplayDoesNotRebroadcastResolvedGeneration(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 360)
	resolver := workspaceConflictRequireResolver(t, fixture.service)
	resolve := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictKeepCurrent, 364)

	_, err := resolver.ResolveConflict(fixture.ctx, fixture.env.UID, resolve)
	require.NoError(t, err)

	replayed, err := fixture.service.ApplyMutation(fixture.ctx, fixture.env.UID, fixture.incoming)
	require.NoError(t, err)
	require.NotNil(t, replayed.Rejected)
	require.Equal(t, dto.WorkspaceMutationRejectConflictCreated, replayed.Rejected.Reason)
	require.Nil(t, replayed.Conflict, "a terminal replay must not rebroadcast a conflict generation that is no longer pending")
}

func TestWorkspaceSyncResolveConflictCurrentIncomingMergedDeleteAreAtomic(t *testing.T) {
	choices := []dto.WorkspaceConflictChoice{
		dto.WorkspaceConflictKeepCurrent,
		dto.WorkspaceConflictUseIncoming,
		dto.WorkspaceConflictUseMerged,
		dto.WorkspaceConflictDelete,
	}
	for index, choice := range choices {
		t.Run(string(choice), func(t *testing.T) {
			fixture := workspaceConflictNewContentFixture(t, 200+index*10)
			request := workspaceConflictResolutionRequest(t, fixture, choice, 204+index*10)
			if choice == dto.WorkspaceConflictUseMerged {
				content := []byte("merged resolution")
				hash := workspaceBlobStoreHash(content)
				request.ContentHash = workspaceSyncNullableHash(hash)
				request.Metadata = dto.WorkspaceFileMetadata{Size: uint64(len(content)), ModifiedAtMS: 99}
				require.NoError(t, fixture.service.blobStore.Put(
					fixture.ctx, fixture.env.UID, hash, uint64(len(content)), bytes.NewReader(content),
				))
			}

			sentinel := errors.New("rollback conflict resolution")
			failingService := NewWorkspaceSyncService(
				&workspaceSyncFailAfterCallbackRepository{WorkspaceRepository: fixture.env.WorkspaceRepo, err: sentinel},
				fixture.service.blobStore,
			)
			_, err := workspaceConflictRequireResolver(t, failingService).ResolveConflict(fixture.ctx, fixture.env.UID, request)
			require.ErrorIs(t, err, sentinel)
			workspaceConflictRequirePendingAtRevision(t, fixture, fixture.current.PathState.PathRevision)

			outcome, err := workspaceConflictRequireResolver(t, fixture.service).ResolveConflict(
				fixture.ctx, fixture.env.UID, request,
			)
			require.NoError(t, err)
			require.NotNil(t, outcome.Resolved)
			require.Equal(t, dto.WorkspaceRevision(3), outcome.Resolved.Revision)
			require.Equal(t, outcome.Resolved.Revision, outcome.Resolved.PathState.PathRevision)
			require.Equal(t, choice, outcome.Resolved.Choice)
			require.NoError(t, outcome.Resolved.Validate())

			require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
				workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
				require.NoError(t, readErr)
				require.Equal(t, outcome.Resolved.Revision, workspace.GlobalRevision)
				path, readErr := tx.Path(string(workspaceSyncWorkspaceID), outcome.Resolved.PathState.Path)
				require.NoError(t, readErr)
				require.Equal(t, outcome.Resolved.PathState, workspaceSyncStateFromRecord(*path))
				conflict, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID))
				require.NoError(t, readErr)
				require.Equal(t, "resolved", conflict.Status)
				require.Equal(t, string(request.OperationID), *conflict.ResolutionOperationID)
				require.Equal(t, outcome.Resolved.Revision, *conflict.ResolutionRevision)
				operation, readErr := tx.Operation(string(request.ClientID), string(request.OperationID))
				require.NoError(t, readErr)
				require.Equal(t, "terminal", operation.State)
				require.Equal(t, string(dto.WorkspaceActionConflictResolved), *operation.ResultAction)
				events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, workspace.GlobalRevision)
				require.NoError(t, readErr)
				require.Len(t, events, 3)
				return nil
			}))
			var conflictRefCount int64
			require.NoError(t, fixture.env.UserDB(fixture.env.UID).Raw(
				"SELECT COUNT(*) FROM workspace_blob_ref WHERE owner_type = ? AND owner_key = ?",
				"conflict", string(fixture.created.ConflictID),
			).Scan(&conflictRefCount).Error)
			require.Zero(t, conflictRefCount)
		})
	}
}

func TestWorkspaceSyncResolveConflictRejectsStaleRevisionBeforeChoiceValidation(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 250)
	request := dto.WorkspaceConflictResolvedRequest{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      workspaceConflictOperationID(254),
		ConflictID:       fixture.created.ConflictID,
		ConflictRevision: workspaceDifferentConflictRevision(fixture.created.ConflictRevision),
		Choice:           "not-a-choice",
		Path:             fixture.created.Path,
		ContentHash:      dto.WorkspaceNullableHash{Present: true},
	}

	_, err := workspaceConflictRequireResolver(t, fixture.service).ResolveConflict(
		fixture.ctx, fixture.env.UID, request,
	)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorConflictRevisionStale)
	workspaceConflictRequirePendingAtRevision(t, fixture, fixture.current.PathState.PathRevision)
	require.ErrorIs(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		_, readErr := tx.Operation(string(request.ClientID), string(request.OperationID))
		return readErr
	}), domain.ErrWorkspaceRecordNotFound)
}

func TestWorkspaceSyncResolveMergedMissingBlobPersistsWaitingBeforeReturningBlobRequired(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 260)
	now := time.Date(2026, time.August, 7, 1, 2, 3, 4, time.UTC)
	fixture.service.now = func() time.Time { return now }
	content := []byte("missing merged content")
	hash := workspaceBlobStoreHash(content)
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictUseMerged, 264)
	request.ContentHash = workspaceSyncNullableHash(hash)
	request.Metadata = dto.WorkspaceFileMetadata{Size: uint64(len(content)), ModifiedAtMS: 321}

	outcome, err := workspaceConflictRequireResolver(t, fixture.service).ResolveConflict(
		fixture.ctx, fixture.env.UID, request,
	)
	require.Nil(t, outcome)
	var serviceErr *WorkspaceServiceError
	require.ErrorAs(t, err, &serviceErr)
	require.Equal(t, dto.WorkspaceErrorBlobRequired, serviceErr.Code)
	require.Equal(t, &dto.WorkspaceBlobNeedUploadPush{
		WorkspaceID: workspaceSyncWorkspaceID,
		Direction:   dto.WorkspaceBlobUpload,
		OperationID: request.OperationID,
		ContentHash: hash,
		Size:        uint64(len(content)),
	}, serviceErr.RequiredUpload)
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		operation, readErr := tx.Operation(string(request.ClientID), string(request.OperationID))
		require.NoError(t, readErr)
		require.Equal(t, "waiting_blob", operation.State)
		require.Equal(t, string(dto.WorkspaceActionConflictResolved), operation.RequestKind)
		require.NotEmpty(t, operation.RequestDigest)
		require.Equal(t, &hash, operation.RequiredHash)
		require.Equal(t, &fixture.created.ConflictRevision, operation.ConflictRevision)
		require.Equal(t, now, operation.CreatedAt)
		require.Equal(t, now.Add(24*time.Hour), *operation.ExpiresAt)
		require.Nil(t, operation.ResultAction)
		require.Empty(t, operation.ResultJSON)
		return nil
	}))
	workspaceConflictRequirePendingAtRevision(t, fixture, fixture.current.PathState.PathRevision)
}

func TestWorkspaceSyncResolveMergedReconnectMissingReturnsBlobRequiredAndNeedAgain(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 270)
	now := time.Date(2026, time.August, 7, 2, 0, 0, 0, time.UTC)
	fixture.service.now = func() time.Time { return now }
	content := []byte("still missing after reconnect")
	hash := workspaceBlobStoreHash(content)
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictUseMerged, 274)
	request.ContentHash = workspaceSyncNullableHash(hash)
	request.Metadata = dto.WorkspaceFileMetadata{Size: uint64(len(content))}
	resolver := workspaceConflictRequireResolver(t, fixture.service)

	_, firstErr := resolver.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	var firstServiceErr *WorkspaceServiceError
	require.ErrorAs(t, firstErr, &firstServiceErr)
	require.Equal(t, dto.WorkspaceErrorBlobRequired, firstServiceErr.Code)
	firstOperation := workspaceConflictReadOperation(t, fixture, request)

	now = now.Add(time.Hour)
	_, secondErr := resolver.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	var secondServiceErr *WorkspaceServiceError
	require.ErrorAs(t, secondErr, &secondServiceErr)
	require.Equal(t, dto.WorkspaceErrorBlobRequired, secondServiceErr.Code)
	require.Equal(t, firstServiceErr.RequiredUpload, secondServiceErr.RequiredUpload)
	secondOperation := workspaceConflictReadOperation(t, fixture, request)
	workspaceSyncRequireSameOperationRecord(t, firstOperation, secondOperation)
	require.Equal(t, firstOperation.CreatedAt, secondOperation.CreatedAt)
	require.Equal(t, firstOperation.ExpiresAt, secondOperation.ExpiresAt)
}

func TestWorkspaceSyncResolveMergedAfterUploadCommitsSameOperation(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 280)
	content := []byte("uploaded merged resolution")
	hash := workspaceBlobStoreHash(content)
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictUseMerged, 284)
	request.ContentHash = workspaceSyncNullableHash(hash)
	request.Metadata = dto.WorkspaceFileMetadata{Size: uint64(len(content)), ModifiedAtMS: 444}
	resolver := workspaceConflictRequireResolver(t, fixture.service)

	_, err := resolver.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorBlobRequired)
	waiting := workspaceConflictReadOperation(t, fixture, request)
	require.Equal(t, "waiting_blob", waiting.State)
	require.NoError(t, fixture.service.blobStore.Put(
		fixture.ctx, fixture.env.UID, hash, uint64(len(content)), bytes.NewReader(content),
	))

	outcome, err := resolver.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	require.NoError(t, err)
	require.NotNil(t, outcome.Resolved)
	require.Equal(t, request.OperationID, outcome.Resolved.OperationID)
	require.Equal(t, hash, *outcome.Resolved.PathState.ContentHash.Value)
	terminal := workspaceConflictReadOperation(t, fixture, request)
	require.Equal(t, "terminal", terminal.State)
	require.Equal(t, waiting.CreatedAt, terminal.CreatedAt)
	require.Nil(t, terminal.RequiredHash)
	require.Nil(t, terminal.ConflictRevision)
	require.Nil(t, terminal.ExpiresAt)
}

func TestWorkspaceSyncResolveMergedStaleAfterUploadDeletesWaitingAndLeavesOrphanForGC(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 290)
	orphanContent := []byte("orphaned merged upload")
	orphanHash := workspaceBlobStoreHash(orphanContent)
	waitingRequest := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictUseMerged, 294)
	waitingRequest.ContentHash = workspaceSyncNullableHash(orphanHash)
	waitingRequest.Metadata = dto.WorkspaceFileMetadata{Size: uint64(len(orphanContent))}
	resolver := workspaceConflictRequireResolver(t, fixture.service)

	_, err := resolver.ResolveConflict(fixture.ctx, fixture.env.UID, waitingRequest)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorBlobRequired)
	resolveCurrent := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictKeepCurrent, 295)
	resolved, err := resolver.ResolveConflict(fixture.ctx, fixture.env.UID, resolveCurrent)
	require.NoError(t, err)
	require.NotNil(t, resolved.Resolved)
	require.NoError(t, fixture.service.blobStore.Put(
		fixture.ctx, fixture.env.UID, orphanHash, uint64(len(orphanContent)), bytes.NewReader(orphanContent),
	))

	_, err = resolver.ResolveConflict(fixture.ctx, fixture.env.UID, waitingRequest)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorConflictRevisionStale)
	guard := workspaceConflictReadOperation(t, fixture, waitingRequest)
	require.Equal(t, "expired_guard", guard.State)
	require.Nil(t, guard.RequiredHash)
	require.Nil(t, guard.ConflictRevision)
	require.Nil(t, guard.ExpiresAt)
	require.Nil(t, guard.ResultAction)
	require.Empty(t, guard.ResultJSON)
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		conflict, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID))
		require.NoError(t, readErr)
		require.Equal(t, "resolved", conflict.Status)
		blob, readErr := tx.Blob(orphanHash)
		require.NoError(t, readErr)
		require.Zero(t, blob.RefCount)
		return nil
	}))
}

func TestWorkspaceSyncPendingResolveExpiresAtExactly24HoursAndKeepsReuseGuard(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 300)
	createdAt := time.Date(2026, time.August, 7, 3, 0, 0, 0, time.UTC)
	now := createdAt
	fixture.service.now = func() time.Time { return now }
	content := []byte("uploaded at expiry")
	hash := workspaceBlobStoreHash(content)
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictUseMerged, 304)
	request.ContentHash = workspaceSyncNullableHash(hash)
	request.Metadata = dto.WorkspaceFileMetadata{Size: uint64(len(content))}
	resolver := workspaceConflictRequireResolver(t, fixture.service)

	_, err := resolver.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorBlobRequired)
	waiting := workspaceConflictReadOperation(t, fixture, request)
	require.Equal(t, createdAt.Add(24*time.Hour), *waiting.ExpiresAt)
	require.NoError(t, fixture.service.blobStore.Put(
		fixture.ctx, fixture.env.UID, hash, uint64(len(content)), bytes.NewReader(content),
	))
	var beforeConflict domain.WorkspaceConflictRecord
	var beforeBlob domain.WorkspaceBlobRecord
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		conflict, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID))
		require.NoError(t, readErr)
		beforeConflict = *conflict
		blob, readErr := tx.Blob(hash)
		require.NoError(t, readErr)
		beforeBlob = *blob
		return nil
	}))

	now = *waiting.ExpiresAt
	_, err = resolver.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorOperationReused)
	guard := workspaceConflictReadOperation(t, fixture, request)
	require.Equal(t, "expired_guard", guard.State)
	require.Equal(t, waiting.CreatedAt, guard.CreatedAt)
	require.Nil(t, guard.RequiredHash)
	require.Nil(t, guard.ConflictRevision)
	require.Nil(t, guard.ExpiresAt)
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		conflict, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID))
		require.NoError(t, readErr)
		require.Equal(t, beforeConflict.Status, conflict.Status)
		require.Equal(t, beforeConflict.ConflictRevision, conflict.ConflictRevision)
		require.Equal(t, beforeConflict.AncestorJSON, conflict.AncestorJSON)
		require.Equal(t, beforeConflict.CurrentJSON, conflict.CurrentJSON)
		require.Equal(t, beforeConflict.IncomingJSON, conflict.IncomingJSON)
		blob, readErr := tx.Blob(hash)
		require.NoError(t, readErr)
		require.Equal(t, beforeBlob.Size, blob.Size)
		require.Equal(t, beforeBlob.RefCount, blob.RefCount)
		return nil
	}))
}

func TestWorkspaceSyncResolvedOperationExactReplaySurvivesConflictRetention(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 310)
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictKeepCurrent, 314)
	resolver := workspaceConflictRequireResolver(t, fixture.service)

	first, err := resolver.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	require.NoError(t, err)
	require.NotNil(t, first.Resolved)
	require.NoError(t, fixture.env.UserDB(fixture.env.UID).Exec(
		"DELETE FROM workspace_conflict WHERE workspace_id = ? AND conflict_id = ?",
		string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID),
	).Error)

	replayed, err := resolver.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	require.NoError(t, err)
	require.NotNil(t, replayed.Resolved)
	firstJSON, err := json.Marshal(first.Resolved)
	require.NoError(t, err)
	replayedJSON, err := json.Marshal(replayed.Resolved)
	require.NoError(t, err)
	require.Equal(t, firstJSON, replayedJSON)
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, first.Resolved.Revision, workspace.GlobalRevision)
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, workspace.GlobalRevision)
		require.NoError(t, readErr)
		require.Len(t, events, 3)
		return nil
	}))
}

func TestWorkspaceSyncConcurrentSameResolveOperationReplaysWinner(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 320)
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictKeepCurrent, 324)
	delayed := newWorkspaceSyncDelayedOperationRepository(
		fixture.env.WorkspaceRepo, string(request.ClientID), string(request.OperationID), 1,
	)
	loserService := NewWorkspaceSyncService(delayed, fixture.service.blobStore)
	type resolveResult struct {
		outcome *WorkspaceResolveOutcome
		err     error
	}
	loserResult := make(chan resolveResult, 1)
	go func() {
		outcome, err := loserService.ResolveConflict(fixture.ctx, fixture.env.UID, request)
		loserResult <- resolveResult{outcome: outcome, err: err}
	}()
	<-delayed.observedMissing

	winner, err := fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	require.NoError(t, err)
	require.NotNil(t, winner.Resolved)
	close(delayed.release)
	loser := <-loserResult
	require.NoError(t, loser.err)
	require.NotNil(t, loser.outcome.Resolved)
	winnerJSON, err := json.Marshal(winner.Resolved)
	require.NoError(t, err)
	loserJSON, err := json.Marshal(loser.outcome.Resolved)
	require.NoError(t, err)
	require.Equal(t, winnerJSON, loserJSON)
}

func TestWorkspaceSyncResolvedConflictCannotBeOverwritten(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 330)
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictKeepCurrent, 334)
	_, err := fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	require.NoError(t, err)
	var resolved domain.WorkspaceConflictRecord
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		record, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID))
		if readErr == nil {
			resolved = *record
		}
		return readErr
	}))

	attempt := resolved
	attempt.Status = "pending"
	attempt.ResolutionOperationID = nil
	attempt.ResolutionRevision = nil
	attempt.ResolutionChoice = nil
	attempt.ResolutionPathStateJSON = nil
	attempt.ResolvedByClientID = nil
	attempt.ResolvedAt = nil
	err = fixture.env.WorkspaceRepo.Write(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.SaveConflict(attempt)
	})
	require.Error(t, err)
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		record, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID))
		require.NoError(t, readErr)
		require.Equal(t, resolved.Status, record.Status)
		require.Equal(t, resolved.ResolutionOperationID, record.ResolutionOperationID)
		require.Equal(t, resolved.ResolutionRevision, record.ResolutionRevision)
		return nil
	}))
}

func TestWorkspaceSyncResolveRenameIncomingPersistsTaggedResolutionItem(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	source := workspaceConflictCommitFile(t, ctx, env, service, "notes/rename-source.md", 0, "source", 341)
	target := workspaceConflictCommitFile(t, ctx, env, service, "notes/rename-target.md", 0, "target", 342)
	targetBase := dto.WorkspaceRevision(0)
	targetPath := target.PathState.Path
	rejectedRename := dto.WorkspaceMutation{
		WorkspaceID:            workspaceSyncWorkspaceID,
		ClientID:               workspaceSyncClientID,
		OperationID:            workspaceConflictOperationID(343),
		Path:                   source.PathState.Path,
		BasePathRevision:       source.PathState.PathRevision,
		Kind:                   dto.WorkspaceMutationRename,
		ContentHash:            source.PathState.ContentHash,
		Metadata:               source.PathState.Metadata,
		NewPath:                &targetPath,
		TargetBasePathRevision: &targetBase,
	}
	createdOutcome, err := service.ApplyMutation(ctx, env.UID, rejectedRename)
	require.NoError(t, err)
	require.NotNil(t, createdOutcome.Conflict)
	request := dto.WorkspaceConflictResolvedRequest{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      workspaceConflictOperationID(344),
		ConflictID:       createdOutcome.Conflict.ConflictID,
		ConflictRevision: createdOutcome.Conflict.ConflictRevision,
		Choice:           dto.WorkspaceConflictUseIncoming,
		Path:             *createdOutcome.Conflict.Incoming.Path,
		ContentHash:      createdOutcome.Conflict.Incoming.ContentHash,
		Metadata:         createdOutcome.Conflict.Incoming.Metadata,
	}
	resolved, err := service.ResolveConflict(ctx, env.UID, request)
	require.NoError(t, err)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), resolved.Resolved.Revision-1, resolved.Resolved.Revision)
		require.NoError(t, readErr)
		require.Len(t, events, 1)
		event := events[0]
		require.Equal(t, "conflict_resolved", event.Kind)
		require.Empty(t, event.MutationJSON)
		require.Equal(t, resolved.Resolved.PathState, func() dto.WorkspacePathState {
			var state dto.WorkspacePathState
			require.NoError(t, json.Unmarshal(event.PathStateJSON, &state))
			return state
		}())
		var resolvedItem dto.WorkspaceConflictResolvedMessage
		require.NoError(t, json.Unmarshal(event.ResolvedJSON, &resolvedItem))
		require.Equal(t, *resolved.Resolved, resolvedItem)
		return nil
	}))
}

func TestWorkspaceSyncResolveWaitingReceiptRejectsPayloadOverwrite(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 350)
	now := time.Date(2026, time.August, 7, 4, 0, 0, 0, time.UTC)
	fixture.service.now = func() time.Time { return now }
	content := []byte("immutable waiting payload")
	hash := workspaceBlobStoreHash(content)
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictUseMerged, 354)
	request.ContentHash = workspaceSyncNullableHash(hash)
	request.Metadata = dto.WorkspaceFileMetadata{Size: uint64(len(content))}
	_, err := fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorBlobRequired)
	waiting := workspaceConflictReadOperation(t, fixture, request)

	overwrite := waiting
	changedRevision := workspaceDifferentConflictRevision(*waiting.ConflictRevision)
	changedExpiry := waiting.ExpiresAt.Add(time.Hour)
	overwrite.ConflictRevision = &changedRevision
	overwrite.ExpiresAt = &changedExpiry
	err = fixture.env.WorkspaceRepo.Write(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.SaveOperation(overwrite)
	})
	require.Error(t, err)
	after := workspaceConflictReadOperation(t, fixture, request)
	require.Equal(t, waiting.ConflictRevision, after.ConflictRevision)
	require.Equal(t, waiting.ExpiresAt, after.ExpiresAt)
}

type workspaceConflictResolver interface {
	ResolveConflict(
		ctx context.Context,
		uid int64,
		req dto.WorkspaceConflictResolvedRequest,
	) (*WorkspaceResolveOutcome, error)
}

type workspaceConflictFixture struct {
	ctx      context.Context
	env      *testutil.WorkspaceEnv
	service  *workspaceSyncService
	ancestor *dto.WorkspaceMutationAcceptedMessage
	current  *dto.WorkspaceMutationAcceptedMessage
	incoming dto.WorkspaceMutation
	created  *dto.WorkspaceConflictCreatedMessage
}

func workspaceConflictRequireResolver(t *testing.T, service any) workspaceConflictResolver {
	t.Helper()
	resolver, ok := service.(workspaceConflictResolver)
	require.True(t, ok, "workspace sync service must implement ResolveConflict")
	return resolver
}

func workspaceConflictNewContentFixture(t *testing.T, operationBase int) *workspaceConflictFixture {
	t.Helper()
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	ancestor := workspaceConflictCommitFile(t, ctx, env, service, "notes/resolve.md", 0, "ancestor", operationBase+1)
	current := workspaceConflictCommitFile(
		t, ctx, env, service, "notes/resolve.md", ancestor.PathState.PathRevision, "current", operationBase+2,
	)
	incoming := workspaceConflictFileMutation(
		t, ctx, env, service, "notes/resolve.md", ancestor.PathState.PathRevision, "incoming", operationBase+3,
	)
	outcome, err := service.ApplyMutation(ctx, env.UID, incoming)
	require.NoError(t, err)
	require.NotNil(t, outcome.Conflict)
	return &workspaceConflictFixture{
		ctx: ctx, env: env, service: service, ancestor: ancestor, current: current, incoming: incoming, created: outcome.Conflict,
	}
}

func workspaceConflictResolutionRequest(
	t *testing.T,
	fixture *workspaceConflictFixture,
	choice dto.WorkspaceConflictChoice,
	operation int,
) dto.WorkspaceConflictResolvedRequest {
	t.Helper()
	request := dto.WorkspaceConflictResolvedRequest{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      workspaceConflictOperationID(operation),
		ConflictID:       fixture.created.ConflictID,
		ConflictRevision: fixture.created.ConflictRevision,
		Choice:           choice,
		Path:             fixture.created.Path,
		ContentHash:      dto.WorkspaceNullableHash{Present: true},
	}
	var side dto.WorkspaceConflictSide
	switch choice {
	case dto.WorkspaceConflictKeepCurrent:
		side = fixture.created.Current
	case dto.WorkspaceConflictUseIncoming:
		side = fixture.created.Incoming
	case dto.WorkspaceConflictDelete:
		return request
	case dto.WorkspaceConflictUseMerged:
		return request
	default:
		t.Fatalf("unsupported conflict choice %q", choice)
	}
	require.NotNil(t, side.Path)
	request.Path = *side.Path
	request.ContentHash = side.ContentHash
	request.Metadata = side.Metadata
	return request
}

func workspaceConflictRequirePendingAtRevision(
	t *testing.T,
	fixture *workspaceConflictFixture,
	revision dto.WorkspaceRevision,
) {
	t.Helper()
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, revision, workspace.GlobalRevision)
		conflict, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID))
		require.NoError(t, readErr)
		require.Equal(t, "pending", conflict.Status)
		return nil
	}))
}

func workspaceConflictReadOperation(
	t *testing.T,
	fixture *workspaceConflictFixture,
	request dto.WorkspaceConflictResolvedRequest,
) domain.WorkspaceOperationRecord {
	t.Helper()
	var result domain.WorkspaceOperationRecord
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		operation, err := tx.Operation(string(request.ClientID), string(request.OperationID))
		if err == nil {
			result = *operation
			result.ResultJSON = append([]byte(nil), operation.ResultJSON...)
		}
		return err
	}))
	return result
}

func workspaceConflictCommitFile(
	t *testing.T,
	ctx context.Context,
	env *testutil.WorkspaceEnv,
	service *workspaceSyncService,
	path dto.WorkspacePath,
	base dto.WorkspaceRevision,
	content string,
	operation int,
) *dto.WorkspaceMutationAcceptedMessage {
	t.Helper()
	mutation := workspaceConflictFileMutation(t, ctx, env, service, path, base, content, operation)
	outcome, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.NotNil(t, outcome.Accepted)
	return outcome.Accepted
}

func workspaceConflictFileMutation(
	t *testing.T,
	ctx context.Context,
	env *testutil.WorkspaceEnv,
	service *workspaceSyncService,
	path dto.WorkspacePath,
	base dto.WorkspaceRevision,
	content string,
	operation int,
) dto.WorkspaceMutation {
	t.Helper()
	hash := workspaceBlobStoreHash([]byte(content))
	require.NoError(t, service.blobStore.Put(ctx, env.UID, hash, uint64(len(content)), bytes.NewBufferString(content)))
	return dto.WorkspaceMutation{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      workspaceConflictOperationID(operation),
		Path:             path,
		BasePathRevision: base,
		Kind:             dto.WorkspaceMutationUpsertFile,
		ContentHash:      workspaceSyncNullableHash(hash),
		Metadata:         dto.WorkspaceFileMetadata{Size: uint64(len(content))},
	}
}

func workspaceConflictOperationID(operation int) dto.WorkspaceUUID {
	return dto.WorkspaceUUID(fmt.Sprintf("20000000-0000-4000-8000-%012d", operation))
}

func workspaceConflictSideFromState(state dto.WorkspacePathState) dto.WorkspaceConflictSide {
	path := state.Path
	return dto.WorkspaceConflictSide{
		Path:         &path,
		PathRevision: state.PathRevision,
		ContentHash:  state.ContentHash,
		Metadata:     state.Metadata,
		Tombstone:    state.Tombstone,
	}
}

func workspaceDifferentConflictRevision(current dto.WorkspaceConflictRevision) dto.WorkspaceConflictRevision {
	currentJSON, _ := json.Marshal(current)
	var currentText string
	_ = json.Unmarshal(currentJSON, &currentText)
	for _, candidate := range []string{"1", "2", "3"} {
		if candidate == currentText {
			continue
		}
		parsed, err := dto.ParseWorkspaceConflictRevision(candidate)
		if err == nil {
			return parsed
		}
	}
	panic("unable to construct a different conflict revision")
}
