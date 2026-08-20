package service

import (
	"context"
	"testing"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/stretchr/testify/require"
)

func TestWorkspaceSyncDesignBConflictCreationDoesNotAdvanceTreeRevisionOrWriteItem(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)

	ancestor := workspaceConflictCommitFile(t, ctx, env, service, "notes/design-b.md", 0, "ancestor", 401)
	current := workspaceConflictCommitFile(t, ctx, env, service, "notes/design-b.md", ancestor.PathState.PathRevision, "current", 402)
	incoming := workspaceConflictFileMutation(t, ctx, env, service, "notes/design-b.md", ancestor.PathState.PathRevision, "incoming", 403)

	outcome, err := service.ApplyMutation(ctx, env.UID, incoming)
	require.NoError(t, err)
	require.NotNil(t, outcome.Conflict)
	require.Equal(t, dto.WorkspaceConflictContent, outcome.Conflict.Kind)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, current.PathState.PathRevision, workspace.GlobalRevision)
		path, readErr := tx.Path(string(workspaceSyncWorkspaceID), incoming.Path)
		require.NoError(t, readErr)
		require.Equal(t, current.PathState, workspaceSyncStateFromRecord(*path))
		items, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, workspace.GlobalRevision)
		require.NoError(t, readErr)
		require.Len(t, items, 2)
		return nil
	}))
}

func TestWorkspaceSyncDesignBResolutionStoresTaggedItemWithoutSyntheticMutationEvent(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 410)
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictKeepCurrent, 414)
	resolved, err := fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	require.NoError(t, err)
	require.NotNil(t, resolved.Resolved)

	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		items, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, workspace.GlobalRevision)
		require.NoError(t, readErr)
		require.Len(t, items, 3)
		require.Empty(t, items[2].MutationJSON)
		require.NotEmpty(t, items[2].PathStateJSON)
		return nil
	}))
}

func TestWorkspaceSyncDesignBResolutionRejectsSourceDriftBeforeApplyingOldGeneration(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 420)
	oldCurrentHash := *fixture.created.Current.ContentHash.Value
	drift := workspaceConflictFileMutation(
		t, fixture.ctx, fixture.env, fixture.service, "notes/resolve.md",
		fixture.current.PathState.PathRevision, "drift", 425,
	)
	driftOutcome, err := fixture.service.ApplyMutation(fixture.ctx, fixture.env.UID, drift)
	require.NoError(t, err)
	require.NotNil(t, driftOutcome.Accepted)

	refreshTime := time.Date(2026, time.August, 10, 1, 2, 3, 0, time.UTC)
	fixture.service.now = func() time.Time { return refreshTime }
	request := workspaceConflictResolutionRequest(t, fixture, dto.WorkspaceConflictKeepCurrent, 426)
	_, err = fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, request)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorConflictRevisionStale)
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		refreshed, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID))
		require.NoError(t, readErr)
		require.NotEqual(t, fixture.created.ConflictRevision, refreshed.ConflictRevision)
		require.Equal(t, refreshTime, refreshed.UpdatedAt)
		created, readErr := workspaceConflictCreatedFromRecord(refreshed)
		require.NoError(t, readErr)
		require.Equal(t, workspaceConflictSideFromState(driftOutcome.Accepted.PathState), created.Current)
		require.Equal(t, fixture.created.Ancestor, created.Ancestor)
		require.Equal(t, fixture.created.Incoming, created.Incoming)
		require.Equal(t, fixture.created.CreatedByOperationID, created.CreatedByOperationID)
		return nil
	}))
	var oldConflictRefs, newConflictRefs int64
	require.NoError(t, fixture.env.UserDB(fixture.env.UID).Raw(
		"SELECT COUNT(*) FROM workspace_blob_ref WHERE owner_type = ? AND owner_key = ? AND content_hash = ?",
		"conflict", string(fixture.created.ConflictID), string(oldCurrentHash),
	).Scan(&oldConflictRefs).Error)
	require.NoError(t, fixture.env.UserDB(fixture.env.UID).Raw(
		"SELECT COUNT(*) FROM workspace_blob_ref WHERE owner_type = ? AND owner_key = ? AND content_hash = ?",
		"conflict", string(fixture.created.ConflictID), string(*driftOutcome.Accepted.PathState.ContentHash.Value),
	).Scan(&newConflictRefs).Error)
	require.Zero(t, oldConflictRefs)
	require.Equal(t, int64(1), newConflictRefs)
}

func TestWorkspaceSyncDesignBStaleResolutionRefreshesGenerationForSubscribeAndRetry(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 600)
	oldGeneration := fixture.created.ConflictRevision
	drift := workspaceConflictFileMutation(
		t, fixture.ctx, fixture.env, fixture.service, fixture.created.Path,
		fixture.current.PathState.PathRevision, "current", 604,
	)
	drift.Metadata.ModifiedAtMS = 123456789
	driftOutcome, err := fixture.service.ApplyMutation(fixture.ctx, fixture.env.UID, drift)
	require.NoError(t, err)
	require.NotNil(t, driftOutcome.Accepted)

	staleRequest := workspaceConflictResolutionRequest(
		t, fixture, dto.WorkspaceConflictKeepCurrent, 605,
	)
	_, err = fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, staleRequest)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorConflictRevisionStale)
	var staleError *WorkspaceServiceError
	require.ErrorAs(t, err, &staleError)
	require.NotNil(t, staleError.RefreshedConflict)
	require.Equal(t, fixture.created.ConflictID, staleError.RefreshedConflict.ConflictID)
	require.NotEqual(t, oldGeneration, staleError.RefreshedConflict.ConflictRevision)

	changeSet, err := fixture.service.Subscribe(fixture.ctx, fixture.env.UID, dto.WorkspaceSubscribeRequest{
		WorkspaceID:     workspaceSyncWorkspaceID,
		ClientID:        workspaceSyncClientID,
		LastAckRevision: driftOutcome.Accepted.Revision,
	})
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceSnapshotIncremental, changeSet.Mode)
	require.Equal(t, driftOutcome.Accepted.Revision, changeSet.FinalRevision)
	require.Equal(t, uint32(1), changeSet.ConflictCount)
	require.NotNil(t, changeSet.PendingConflicts)
	t.Cleanup(func() { require.NoError(t, changeSet.PendingConflicts.Close()) })
	refreshed, err := changeSet.PendingConflicts.Next(fixture.ctx)
	require.NoError(t, err)
	require.NotNil(t, refreshed)
	require.Equal(t, fixture.created.ConflictID, refreshed.ConflictID)
	require.NotEqual(t, oldGeneration, refreshed.ConflictRevision)
	require.Equal(t, workspaceConflictSideFromState(driftOutcome.Accepted.PathState), refreshed.Current)

	retry := workspaceConflictResolutionRequest(
		t,
		&workspaceConflictFixture{created: refreshed},
		dto.WorkspaceConflictKeepCurrent,
		606,
	)
	resolved, err := fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, retry)
	require.NoError(t, err)
	require.NotNil(t, resolved.Resolved)
	require.Equal(t, driftOutcome.Accepted.Revision+1, resolved.Resolved.Revision)
	require.Equal(t, refreshed.ConflictRevision, resolved.Resolved.ConflictRevision)
	require.Equal(t, driftOutcome.Accepted.PathState.Metadata, resolved.Resolved.PathState.Metadata)

	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, resolved.Resolved.Revision, workspace.GlobalRevision)
		pending, readErr := tx.PendingConflict(string(workspaceSyncWorkspaceID), fixture.created.Path)
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		require.Nil(t, pending)
		events, readErr := tx.EventsAfter(
			string(workspaceSyncWorkspaceID), resolved.Resolved.Revision-1, resolved.Resolved.Revision,
		)
		require.NoError(t, readErr)
		require.Len(t, events, 1)
		require.Equal(t, "conflict_resolved", events[0].Kind)
		return nil
	}))
}

func TestWorkspaceSyncDesignBConflictRefreshKeepsTreeRevisionAndConflictID(t *testing.T) {
	fixture := workspaceConflictNewContentFixture(t, 430)
	first := fixture.created
	secondMutation := workspaceConflictFileMutation(
		t, fixture.ctx, fixture.env, fixture.service, "notes/resolve.md",
		fixture.ancestor.PathState.PathRevision, "incoming-refresh", 434,
	)
	second, err := fixture.service.ApplyMutation(fixture.ctx, fixture.env.UID, secondMutation)
	require.NoError(t, err)
	require.NotNil(t, second.Conflict)
	require.Equal(t, first.ConflictID, second.Conflict.ConflictID)
	require.NotEqual(t, first.ConflictRevision, second.Conflict.ConflictRevision)

	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, fixture.current.PathState.PathRevision, workspace.GlobalRevision)
		path, readErr := tx.Path(string(workspaceSyncWorkspaceID), secondMutation.Path)
		require.NoError(t, readErr)
		require.Equal(t, fixture.current.PathState, workspaceSyncStateFromRecord(*path))
		items, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, workspace.GlobalRevision)
		require.NoError(t, readErr)
		require.Len(t, items, 2)
		return nil
	}))
}

func TestWorkspaceSyncDesignBRenameTargetDriftRejectsOldGeneration(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	source := workspaceConflictCommitFile(t, ctx, env, service, "notes/drift-source.md", 0, "source", 440)
	target := workspaceConflictCommitFile(t, ctx, env, service, "notes/drift-target.md", 0, "target", 441)
	targetPath := target.PathState.Path
	targetBase := dto.WorkspaceRevision(0)
	rename := dto.WorkspaceMutation{
		WorkspaceID:            workspaceSyncWorkspaceID,
		ClientID:               workspaceSyncClientID,
		OperationID:            workspaceConflictOperationID(442),
		Path:                   source.PathState.Path,
		BasePathRevision:       source.PathState.PathRevision,
		Kind:                   dto.WorkspaceMutationRename,
		ContentHash:            source.PathState.ContentHash,
		Metadata:               source.PathState.Metadata,
		NewPath:                &targetPath,
		TargetBasePathRevision: &targetBase,
	}
	created, err := service.ApplyMutation(ctx, env.UID, rename)
	require.NoError(t, err)
	require.NotNil(t, created.Conflict)

	drift := workspaceConflictFileMutation(t, ctx, env, service, targetPath, target.PathState.PathRevision, "target-drift", 443)
	driftOutcome, err := service.ApplyMutation(ctx, env.UID, drift)
	require.NoError(t, err)
	require.NotNil(t, driftOutcome.Accepted)

	request := workspaceConflictResolutionRequest(t, &workspaceConflictFixture{
		created: created.Conflict,
	}, dto.WorkspaceConflictUseIncoming, 444)
	_, err = service.ResolveConflict(ctx, env.UID, request)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorConflictRevisionStale)

	var refreshed *dto.WorkspaceConflictCreatedMessage
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		record, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(created.Conflict.ConflictID))
		require.NoError(t, readErr)
		require.NotEqual(t, created.Conflict.ConflictRevision, record.ConflictRevision)
		refreshed, readErr = workspaceConflictCreatedFromRecord(record)
		require.NoError(t, readErr)
		targetSnapshot, descendants, tagged, decodeErr := workspaceConflictDecodeRenameSnapshot(record.RenameTargetJSON)
		require.NoError(t, decodeErr)
		require.True(t, tagged)
		require.Empty(t, descendants)
		require.Equal(t, workspaceConflictSideFromState(driftOutcome.Accepted.PathState), targetSnapshot)
		return nil
	}))
	retry := workspaceConflictResolutionRequest(
		t, &workspaceConflictFixture{created: refreshed}, dto.WorkspaceConflictKeepCurrent, 445,
	)
	resolved, err := service.ResolveConflict(ctx, env.UID, retry)
	require.NoError(t, err)
	require.NotNil(t, resolved.Resolved)
	require.Equal(t, refreshed.ConflictRevision, resolved.Resolved.ConflictRevision)
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		storedTarget, readErr := tx.Path(string(workspaceSyncWorkspaceID), targetPath)
		require.NoError(t, readErr)
		require.Equal(t, driftOutcome.Accepted.PathState.ContentHash.Value, storedTarget.ContentHash)
		return nil
	}))
}

func TestWorkspaceSyncDesignBRefreshesDirectoryRenameDescendantSnapshotAtomically(t *testing.T) {
	fixture := workspaceConflictDirectoryRenameFixture(t, 700)
	drift := workspaceSyncPutMutation(
		t, fixture.ctx, fixture.env, fixture.service,
		fixture.child.PathState.Path, fixture.child.PathState.PathRevision, "drifted descendant",
	)
	drift.OperationID = workspaceConflictOperationID(710)
	driftOutcome, err := fixture.service.ApplyMutation(fixture.ctx, fixture.env.UID, drift)
	require.NoError(t, err)
	require.NotNil(t, driftOutcome.Accepted)

	oldRequest := workspaceConflictResolutionRequest(
		t, &workspaceConflictFixture{created: fixture.created}, dto.WorkspaceConflictUseIncoming, 711,
	)
	_, err = fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, oldRequest)
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorConflictRevisionStale)

	var refreshed *dto.WorkspaceConflictCreatedMessage
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		record, readErr := tx.Conflict(string(workspaceSyncWorkspaceID), string(fixture.created.ConflictID))
		require.NoError(t, readErr)
		require.NotEqual(t, fixture.created.ConflictRevision, record.ConflictRevision)
		refreshed, readErr = workspaceConflictCreatedFromRecord(record)
		require.NoError(t, readErr)
		_, descendants, tagged, decodeErr := workspaceConflictDecodeRenameSnapshot(record.RenameTargetJSON)
		require.NoError(t, decodeErr)
		require.True(t, tagged)
		var refreshedChild *dto.WorkspacePathState
		for i := range descendants {
			if descendants[i].Path == fixture.child.PathState.Path {
				refreshedChild = &descendants[i]
				break
			}
		}
		require.NotNil(t, refreshedChild)
		require.Equal(t, driftOutcome.Accepted.PathState, *refreshedChild)
		return nil
	}))

	retry := workspaceConflictResolutionRequest(
		t, &workspaceConflictFixture{created: refreshed}, dto.WorkspaceConflictKeepCurrent, 712,
	)
	resolved, err := fixture.service.ResolveConflict(fixture.ctx, fixture.env.UID, retry)
	require.NoError(t, err)
	require.NotNil(t, resolved.Resolved)
	require.Equal(t, refreshed.ConflictRevision, resolved.Resolved.ConflictRevision)
	require.NoError(t, fixture.env.WorkspaceRepo.Read(fixture.ctx, fixture.env.UID, func(tx domain.WorkspaceReadTx) error {
		child, readErr := tx.Path(string(workspaceSyncWorkspaceID), fixture.child.PathState.Path)
		require.NoError(t, readErr)
		require.Equal(t, driftOutcome.Accepted.PathState.ContentHash.Value, child.ContentHash)
		return nil
	}))
}

func TestWorkspaceSyncDesignBKeepCurrentResolvesRenameAfterSourceWasDeleted(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	ancestor := workspaceConflictCommitFile(t, ctx, env, service, "notes/deleted-source.md", 0, "ancestor", 800)
	current := workspaceConflictCommitFile(
		t, ctx, env, service, ancestor.PathState.Path, ancestor.PathState.PathRevision, "current", 801,
	)
	target := dto.WorkspacePath("notes/deleted-target.md")
	targetBase := dto.WorkspaceRevision(0)
	rename := dto.WorkspaceMutation{
		WorkspaceID:            workspaceSyncWorkspaceID,
		ClientID:               workspaceSyncClientID,
		OperationID:            workspaceConflictOperationID(802),
		Path:                   ancestor.PathState.Path,
		BasePathRevision:       ancestor.PathState.PathRevision,
		Kind:                   dto.WorkspaceMutationRename,
		ContentHash:            current.PathState.ContentHash,
		Metadata:               current.PathState.Metadata,
		NewPath:                &target,
		TargetBasePathRevision: &targetBase,
	}
	created, err := service.ApplyMutation(ctx, env.UID, rename)
	require.NoError(t, err)
	require.NotNil(t, created.Conflict)

	deleteMutation := dto.WorkspaceMutation{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      workspaceConflictOperationID(803),
		Path:             ancestor.PathState.Path,
		BasePathRevision: current.PathState.PathRevision,
		Kind:             dto.WorkspaceMutationDelete,
		ContentHash:      dto.WorkspaceNullableHash{Present: true},
	}
	deleted, err := service.ApplyMutation(ctx, env.UID, deleteMutation)
	require.NoError(t, err)
	require.NotNil(t, deleted.Accepted)
	require.True(t, deleted.Accepted.PathState.Tombstone)

	request := workspaceConflictResolutionRequest(
		t, &workspaceConflictFixture{created: created.Conflict}, dto.WorkspaceConflictKeepCurrent, 804,
	)
	resolved, err := service.ResolveConflict(ctx, env.UID, request)
	require.NoError(t, err)
	require.NotNil(t, resolved.Resolved)
	require.Equal(t, dto.WorkspaceConflictKeepCurrent, resolved.Resolved.Choice)
	require.True(t, resolved.Resolved.PathState.Tombstone)
	require.Equal(t, deleted.Accepted.Revision+1, resolved.Resolved.Revision)
	require.Equal(t, created.Conflict.ConflictRevision, resolved.Resolved.ConflictRevision)
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		pending, readErr := tx.PendingConflict(string(workspaceSyncWorkspaceID), ancestor.PathState.Path)
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		require.Nil(t, pending)
		return nil
	}))
}
