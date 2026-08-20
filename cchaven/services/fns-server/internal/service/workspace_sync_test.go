package service

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/config"
	"github.com/haierkeys/fast-note-sync-service/internal/dao"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/testutil"
	"github.com/stretchr/testify/require"
	"go.uber.org/zap"
	"gorm.io/gorm"
)

const (
	workspaceSyncWorkspaceID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000001")
	workspaceSyncClientID    = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000002")
	workspaceSyncOtherClient = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000003")
)

func TestWorkspaceSyncSubscribeCreatesOnlyRevisionZeroWorkspaceAndRegistersClient(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	service := NewWorkspaceSyncService(
		env.WorkspaceRepo,
		NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot)),
	)
	req := dto.WorkspaceSubscribeRequest{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncClientID,
	}

	changeSet, err := service.Subscribe(context.Background(), env.UID, req)
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceSnapshotFull, changeSet.Mode)
	require.Zero(t, changeSet.FromRevision)
	require.Zero(t, changeSet.FinalRevision)
	require.Empty(t, changeSet.Entries)
	require.Empty(t, changeSet.Events)

	require.NoError(t, env.WorkspaceRepo.Read(context.Background(), env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Zero(t, workspace.GlobalRevision)
		client, readErr := tx.Client(string(workspaceSyncWorkspaceID), string(workspaceSyncClientID))
		require.NoError(t, readErr)
		require.Zero(t, client.LastAckRevision)
		return nil
	}))

	_, err = service.Subscribe(context.Background(), env.UID, dto.WorkspaceSubscribeRequest{
		WorkspaceID:     workspaceSyncWorkspaceID,
		ClientID:        workspaceSyncOtherClient,
		LastAckRevision: 1,
	})
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorClientNotRegistered)

	_, err = service.Subscribe(context.Background(), env.UID, dto.WorkspaceSubscribeRequest{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncOtherClient,
	})
	require.NoError(t, err)

	missingWorkspace := dto.WorkspaceUUID("10000000-0000-4000-8000-000000000004")
	_, err = service.Subscribe(context.Background(), env.UID, dto.WorkspaceSubscribeRequest{
		WorkspaceID:     missingWorkspace,
		ClientID:        workspaceSyncClientID,
		LastAckRevision: 1,
	})
	workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorWorkspaceNotFound)
	require.ErrorIs(t, env.WorkspaceRepo.Read(context.Background(), env.UID, func(tx domain.WorkspaceReadTx) error {
		_, readErr := tx.Workspace(string(missingWorkspace))
		return readErr
	}), domain.ErrWorkspaceRecordNotFound)
}

func TestWorkspaceSyncApplyMutationAssignsMonotonicGlobalRevisionToPathState(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)

	first := workspaceSyncPutMutation(t, ctx, env, service, "notes/a.md", 0, "first")
	firstOutcome, err := service.ApplyMutation(ctx, env.UID, first)
	require.NoError(t, err)
	require.NotNil(t, firstOutcome.Accepted)
	require.Equal(t, dto.WorkspaceRevision(1), firstOutcome.Accepted.Revision)
	require.Equal(t, dto.WorkspaceRevision(1), firstOutcome.Accepted.PathState.PathRevision)

	second := workspaceSyncPutMutation(t, ctx, env, service, "notes/a.md", 1, "second")
	second.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000011")
	secondOutcome, err := service.ApplyMutation(ctx, env.UID, second)
	require.NoError(t, err)
	require.NotNil(t, secondOutcome.Accepted)
	require.Equal(t, dto.WorkspaceRevision(2), secondOutcome.Accepted.Revision)
	require.Equal(t, dto.WorkspaceRevision(2), secondOutcome.Accepted.PathState.PathRevision)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(2), workspace.GlobalRevision)
		path, readErr := tx.Path(string(workspaceSyncWorkspaceID), "notes/a.md")
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(2), path.PathRevision)
		return nil
	}))
}

func TestWorkspaceSyncApplyMutationDifferentPathsUseGlobalRevisionsOneAndTwo(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)

	first := workspaceSyncPutMutation(t, ctx, env, service, "notes/a.md", 0, "first")
	firstOutcome, err := service.ApplyMutation(ctx, env.UID, first)
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceRevision(1), firstOutcome.Accepted.Revision)
	require.Equal(t, dto.WorkspaceRevision(1), firstOutcome.Accepted.PathState.PathRevision)

	second := workspaceSyncPutMutation(t, ctx, env, service, "notes/b.md", 0, "second")
	second.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000011")
	secondOutcome, err := service.ApplyMutation(ctx, env.UID, second)
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceRevision(2), secondOutcome.Accepted.Revision)
	require.Equal(t, dto.WorkspaceRevision(2), secondOutcome.Accepted.PathState.PathRevision)

	third := workspaceSyncPutMutation(t, ctx, env, service, "notes/a.md", 1, "third")
	third.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000012")
	thirdOutcome, err := service.ApplyMutation(ctx, env.UID, third)
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceRevision(3), thirdOutcome.Accepted.Revision)
	require.Equal(t, dto.WorkspaceRevision(3), thirdOutcome.Accepted.PathState.PathRevision)
}

func TestWorkspaceSyncApplyMutationExactOperationReplaySurvivesServiceRestart(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	mutation := workspaceSyncPutMutation(t, ctx, env, service, "notes/a.md", 0, "receipt")

	first, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	restarted := NewWorkspaceSyncService(
		env.WorkspaceRepo,
		NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot)),
	)
	replayed, err := restarted.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.NotNil(t, replayed.Accepted)
	firstJSON, err := json.Marshal(first.Accepted)
	require.NoError(t, err)
	replayedJSON, err := json.Marshal(replayed.Accepted)
	require.NoError(t, err)
	require.Equal(t, firstJSON, replayedJSON)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(1), workspace.GlobalRevision)
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, workspace.GlobalRevision)
		require.NoError(t, readErr)
		require.Len(t, events, 1)
		operation, readErr := tx.Operation(string(workspaceSyncClientID), string(mutation.OperationID))
		require.NoError(t, readErr)
		require.Equal(t, "terminal", operation.State)
		require.Equal(t, string(dto.WorkspaceActionMutationAccepted), *operation.ResultAction)
		require.Equal(t, firstJSON, operation.ResultJSON)
		return nil
	}))
}

func TestWorkspaceSyncApplyMutationChangedPayloadWithSameOperationRejectsReuse(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	original := workspaceSyncPutMutation(t, ctx, env, service, "notes/a.md", 0, "original")
	first, err := service.ApplyMutation(ctx, env.UID, original)
	require.NoError(t, err)

	changed := workspaceSyncPutMutation(t, ctx, env, service, "notes/a.md", 1, "changed")
	changed.OperationID = original.OperationID
	outcome, err := service.ApplyMutation(ctx, env.UID, changed)
	require.NoError(t, err)
	require.Nil(t, outcome.Accepted)
	require.NotNil(t, outcome.Rejected)
	require.Equal(t, dto.WorkspaceMutationRejectOperationReused, outcome.Rejected.Reason)
	require.Equal(t, original.WorkspaceID, outcome.Rejected.WorkspaceID)
	require.Equal(t, original.ClientID, outcome.Rejected.ClientID)
	require.Equal(t, original.OperationID, outcome.Rejected.OperationID)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(1), workspace.GlobalRevision)
		operation, readErr := tx.Operation(string(workspaceSyncClientID), string(original.OperationID))
		require.NoError(t, readErr)
		firstJSON, marshalErr := json.Marshal(first.Accepted)
		require.NoError(t, marshalErr)
		require.Equal(t, firstJSON, operation.ResultJSON)
		return nil
	}))
}

func TestWorkspaceSyncApplyMutationMissingBlobPersistsWaitingDigestThenAcceptsAfterPut(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	content := []byte("upload after rejection")
	hash := workspaceBlobStoreHash(content)
	mutation := dto.WorkspaceMutation{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      dto.WorkspaceUUID("10000000-0000-4000-8000-000000000020"),
		Path:             "notes/upload.md",
		BasePathRevision: 0,
		Kind:             dto.WorkspaceMutationUpsertFile,
		ContentHash:      workspaceSyncNullableHash(hash),
		Metadata:         dto.WorkspaceFileMetadata{Size: uint64(len(content))},
	}

	waiting, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.Nil(t, waiting.Accepted)
	require.NotNil(t, waiting.Rejected)
	require.Equal(t, dto.WorkspaceMutationRejectBlobRequired, waiting.Rejected.Reason)
	require.Equal(t, &hash, waiting.Rejected.RequiredHash)
	require.Equal(t, &dto.WorkspaceBlobNeedUploadPush{
		WorkspaceID: workspaceSyncWorkspaceID,
		Direction:   dto.WorkspaceBlobUpload,
		OperationID: mutation.OperationID,
		ContentHash: hash,
		Size:        uint64(len(content)),
	}, waiting.RequiredUpload)
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Zero(t, workspace.GlobalRevision)
		operation, readErr := tx.Operation(string(workspaceSyncClientID), string(mutation.OperationID))
		require.NoError(t, readErr)
		require.Equal(t, "waiting_blob", operation.State)
		require.NotEmpty(t, operation.RequestDigest)
		require.Equal(t, &hash, operation.RequiredHash)
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, 1)
		require.NoError(t, readErr)
		require.Empty(t, events)
		return nil
	}))

	require.NoError(t, service.blobStore.Put(ctx, env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
	accepted, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.NotNil(t, accepted.Accepted)
	require.Equal(t, dto.WorkspaceRevision(1), accepted.Accepted.Revision)
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		operation, readErr := tx.Operation(string(workspaceSyncClientID), string(mutation.OperationID))
		require.NoError(t, readErr)
		require.Equal(t, "terminal", operation.State)
		require.Nil(t, operation.RequiredHash)
		return nil
	}))
}

func TestWorkspaceSyncWaitingBlobExactReplayReissuesSameNeedAfterServiceRestart(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	content := []byte("resume the exact missing blob after restart")
	hash := workspaceBlobStoreHash(content)
	mutation := dto.WorkspaceMutation{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      dto.WorkspaceUUID("10000000-0000-4000-8000-000000000021"),
		Path:             "notes/restart-upload.md",
		BasePathRevision: 0,
		Kind:             dto.WorkspaceMutationUpsertFile,
		ContentHash:      workspaceSyncNullableHash(hash),
		Metadata:         dto.WorkspaceFileMetadata{Size: uint64(len(content))},
	}

	first, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.NotNil(t, first.Rejected)
	require.Equal(t, dto.WorkspaceMutationRejectBlobRequired, first.Rejected.Reason)
	require.NotNil(t, first.RequiredUpload)

	restarted := NewWorkspaceSyncService(
		env.WorkspaceRepo,
		NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot)),
	)
	replayed, err := restarted.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.Equal(t, first.Rejected, replayed.Rejected)
	require.Equal(t, first.RequiredUpload, replayed.RequiredUpload)
	require.Nil(t, replayed.Accepted)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Zero(t, workspace.GlobalRevision)
		operation, readErr := tx.Operation(string(workspaceSyncClientID), string(mutation.OperationID))
		require.NoError(t, readErr)
		require.Equal(t, "waiting_blob", operation.State)
		require.Equal(t, &hash, operation.RequiredHash)
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, 1)
		require.NoError(t, readErr)
		require.Empty(t, events)
		return nil
	}))

	require.NoError(t, restarted.blobStore.Put(
		ctx,
		env.UID,
		hash,
		uint64(len(content)),
		bytes.NewReader(content),
	))
	restartedAfterPut := NewWorkspaceSyncService(
		env.WorkspaceRepo,
		NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot)),
	)
	accepted, err := restartedAfterPut.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.NotNil(t, accepted.Accepted)
	require.Equal(t, dto.WorkspaceRevision(1), accepted.Accepted.Revision)

	replayedAccepted, err := restartedAfterPut.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.Equal(t, accepted.Accepted, replayedAccepted.Accepted)
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(1), workspace.GlobalRevision)
		operation, readErr := tx.Operation(string(workspaceSyncClientID), string(mutation.OperationID))
		require.NoError(t, readErr)
		require.Equal(t, "terminal", operation.State)
		require.Nil(t, operation.RequiredHash)
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, workspace.GlobalRevision)
		require.NoError(t, readErr)
		require.Len(t, events, 1)
		return nil
	}))
}

func TestWorkspaceSyncApplyMutationTransactionFailureLeavesRevisionStateEventReceiptAndRefsUnchanged(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	mutation := workspaceSyncPutMutation(t, ctx, env, service, "notes/rollback.md", 0, "rollback")
	failure := errors.New("commit failpoint")
	failing := NewWorkspaceSyncService(
		&workspaceSyncFailAfterCallbackRepository{WorkspaceRepository: env.WorkspaceRepo, err: failure},
		service.blobStore,
	)

	outcome, err := failing.ApplyMutation(ctx, env.UID, mutation)
	require.Nil(t, outcome)
	require.ErrorIs(t, err, failure)
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Zero(t, workspace.GlobalRevision)
		_, readErr = tx.Path(string(workspaceSyncWorkspaceID), mutation.Path)
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, 1)
		require.NoError(t, readErr)
		require.Empty(t, events)
		_, readErr = tx.Operation(string(workspaceSyncClientID), string(mutation.OperationID))
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		blob, readErr := tx.Blob(*mutation.ContentHash.Value)
		require.NoError(t, readErr)
		require.Zero(t, blob.RefCount)
		return nil
	}))
}

func TestWorkspaceSyncConcurrentMutationsHaveUniqueGapFreeCommittedRevisions(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	content := []byte("shared concurrent blob")
	hash := workspaceBlobStoreHash(content)
	require.NoError(t, service.blobStore.Put(ctx, env.UID, hash, uint64(len(content)), bytes.NewReader(content)))

	const mutationCount = 12
	start := make(chan struct{})
	results := make(chan dto.WorkspaceRevision, mutationCount)
	errs := make(chan error, mutationCount)
	for i := range mutationCount {
		go func(index int) {
			<-start
			mutation := dto.WorkspaceMutation{
				WorkspaceID:      workspaceSyncWorkspaceID,
				ClientID:         workspaceSyncClientID,
				OperationID:      dto.WorkspaceUUID(fmt.Sprintf("10000000-0000-4000-8000-%012d", 100+index)),
				Path:             dto.WorkspacePath(fmt.Sprintf("notes/concurrent-%02d.md", index)),
				BasePathRevision: 0,
				Kind:             dto.WorkspaceMutationUpsertFile,
				ContentHash:      workspaceSyncNullableHash(hash),
				Metadata:         dto.WorkspaceFileMetadata{Size: uint64(len(content))},
			}
			outcome, err := service.ApplyMutation(ctx, env.UID, mutation)
			if err != nil {
				errs <- err
				return
			}
			results <- outcome.Accepted.Revision
		}(i)
	}
	close(start)

	revisions := make([]int, 0, mutationCount)
	for range mutationCount {
		select {
		case err := <-errs:
			require.NoError(t, err)
		case revision := <-results:
			revisions = append(revisions, int(revision))
		}
	}
	sort.Ints(revisions)
	require.Equal(t, mutationCount, len(revisions))
	for i := range mutationCount {
		require.Equal(t, i+1, revisions[i])
	}
}

func TestWorkspaceSyncDeleteKeepsTombstoneAtAssignedGlobalRevision(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	created := workspaceSyncPutMutation(t, ctx, env, service, "notes/delete.md", 0, "delete me")
	_, err := service.ApplyMutation(ctx, env.UID, created)
	require.NoError(t, err)
	deleted := dto.WorkspaceMutation{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      dto.WorkspaceUUID("10000000-0000-4000-8000-000000000030"),
		Path:             created.Path,
		BasePathRevision: 1,
		Kind:             dto.WorkspaceMutationDelete,
		ContentHash:      dto.WorkspaceNullableHash{Present: true},
		Metadata:         dto.WorkspaceFileMetadata{},
	}

	outcome, err := service.ApplyMutation(ctx, env.UID, deleted)
	require.NoError(t, err)
	require.NotNil(t, outcome.Accepted)
	require.Equal(t, dto.WorkspaceRevision(2), outcome.Accepted.Revision)
	require.Equal(t, dto.WorkspacePathState{
		Path:         deleted.Path,
		PathRevision: 2,
		Kind:         dto.WorkspaceEntryTombstone,
		ContentHash:  dto.WorkspaceNullableHash{Present: true},
		Metadata:     dto.WorkspaceFileMetadata{},
		Tombstone:    true,
	}, outcome.Accepted.PathState)
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(2), workspace.GlobalRevision)
		require.Zero(t, workspace.LivePathCount)
		require.Zero(t, workspace.LiveBytes)
		path, readErr := tx.Path(string(workspaceSyncWorkspaceID), deleted.Path)
		require.NoError(t, readErr)
		require.True(t, path.Tombstone)
		require.Equal(t, dto.WorkspaceRevision(2), path.PathRevision)
		blob, readErr := tx.Blob(*created.ContentHash.Value)
		require.NoError(t, readErr)
		require.Equal(t, int64(1), blob.RefCount)
		return nil
	}))
}

func TestWorkspaceSyncRenameCreatesOldTombstoneAndNewLiveStateAtOneRevision(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	created := workspaceSyncPutMutation(t, ctx, env, service, "notes/source.md", 0, "rename me")
	_, err := service.ApplyMutation(ctx, env.UID, created)
	require.NoError(t, err)
	target := dto.WorkspacePath("archive/target.md")
	targetBase := dto.WorkspaceRevision(0)
	rename := dto.WorkspaceMutation{
		WorkspaceID:            workspaceSyncWorkspaceID,
		ClientID:               workspaceSyncClientID,
		OperationID:            dto.WorkspaceUUID("10000000-0000-4000-8000-000000000031"),
		Path:                   created.Path,
		BasePathRevision:       1,
		Kind:                   dto.WorkspaceMutationRename,
		ContentHash:            created.ContentHash,
		Metadata:               created.Metadata,
		NewPath:                &target,
		TargetBasePathRevision: &targetBase,
	}

	outcome, err := service.ApplyMutation(ctx, env.UID, rename)
	require.NoError(t, err)
	require.NotNil(t, outcome.Accepted)
	require.Equal(t, dto.WorkspaceRevision(2), outcome.Accepted.Revision)
	require.NotNil(t, outcome.Accepted.OldPathState)
	require.NotNil(t, outcome.Accepted.NewPathState)
	require.Equal(t, dto.WorkspaceRevision(2), outcome.Accepted.OldPathState.PathRevision)
	require.True(t, outcome.Accepted.OldPathState.Tombstone)
	require.Equal(t, created.Path, outcome.Accepted.OldPathState.Path)
	require.Equal(t, dto.WorkspaceRevision(2), outcome.Accepted.NewPathState.PathRevision)
	require.False(t, outcome.Accepted.NewPathState.Tombstone)
	require.Equal(t, target, outcome.Accepted.NewPathState.Path)
	require.Equal(t, *outcome.Accepted.NewPathState, outcome.Accepted.PathState)
	require.NoError(t, outcome.Accepted.Validate())

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		source, readErr := tx.Path(string(workspaceSyncWorkspaceID), created.Path)
		require.NoError(t, readErr)
		require.True(t, source.Tombstone)
		require.Equal(t, dto.WorkspaceRevision(2), source.PathRevision)
		destination, readErr := tx.Path(string(workspaceSyncWorkspaceID), target)
		require.NoError(t, readErr)
		require.False(t, destination.Tombstone)
		require.Equal(t, dto.WorkspaceRevision(2), destination.PathRevision)
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, int64(1), workspace.LivePathCount)
		require.Equal(t, created.Metadata.Size, workspace.LiveBytes)
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 1, 2)
		require.NoError(t, readErr)
		require.Len(t, events, 1)
		require.NotEmpty(t, events[0].OldPathStateJSON)
		require.NotEmpty(t, events[0].NewPathStateJSON)
		return nil
	}))
}

func TestWorkspaceSyncRenameRejectsEitherMismatchedSourceOrTargetBase(t *testing.T) {
	tests := []struct {
		name       string
		sourceBase dto.WorkspaceRevision
		targetBase dto.WorkspaceRevision
	}{
		{name: "source", sourceBase: 0, targetBase: 0},
		{name: "target", sourceBase: 1, targetBase: 1},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			env, service := workspaceSyncNewService(t)
			ctx := context.Background()
			workspaceSyncSubscribe(t, ctx, env, service)
			created := workspaceSyncPutMutation(t, ctx, env, service, "notes/source.md", 0, "rename base")
			_, err := service.ApplyMutation(ctx, env.UID, created)
			require.NoError(t, err)
			target := dto.WorkspacePath("notes/target.md")
			rename := dto.WorkspaceMutation{
				WorkspaceID:            workspaceSyncWorkspaceID,
				ClientID:               workspaceSyncClientID,
				OperationID:            dto.WorkspaceUUID("10000000-0000-4000-8000-000000000032"),
				Path:                   created.Path,
				BasePathRevision:       tc.sourceBase,
				Kind:                   dto.WorkspaceMutationRename,
				ContentHash:            created.ContentHash,
				Metadata:               created.Metadata,
				NewPath:                &target,
				TargetBasePathRevision: &tc.targetBase,
			}

			outcome, err := service.ApplyMutation(ctx, env.UID, rename)
			require.NoError(t, err)
			require.Nil(t, outcome.Accepted)
			require.NotNil(t, outcome.Rejected)
			require.Equal(t, dto.WorkspaceMutationRejectConflictCreated, outcome.Rejected.Reason)
			require.NotNil(t, outcome.Conflict)
			require.NotNil(t, outcome.Rejected.CurrentPathState)
			require.Equal(t, dto.WorkspaceRevision(1), outcome.Rejected.CurrentPathState.PathRevision)

			replayed, err := service.ApplyMutation(ctx, env.UID, rename)
			require.NoError(t, err)
			require.Equal(t, outcome.Rejected, replayed.Rejected)
			require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
				workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
				require.NoError(t, readErr)
				require.Equal(t, dto.WorkspaceRevision(1), workspace.GlobalRevision)
				operation, readErr := tx.Operation(string(workspaceSyncClientID), string(rename.OperationID))
				require.NoError(t, readErr)
				require.Equal(t, "terminal", operation.State)
				require.Equal(t, string(dto.WorkspaceActionMutationRejected), *operation.ResultAction)
				return nil
			}))
		})
	}
}

func TestWorkspaceSyncDirectoryRenameMovesDescendantsAtomically(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	root := workspaceSyncMkdirMutation("docs", 0, 40)
	_, err := service.ApplyMutation(ctx, env.UID, root)
	require.NoError(t, err)
	first := workspaceSyncPutMutation(t, ctx, env, service, "docs/a.md", 0, "a")
	first.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000041")
	_, err = service.ApplyMutation(ctx, env.UID, first)
	require.NoError(t, err)
	nested := workspaceSyncMkdirMutation("docs/nested", 0, 42)
	_, err = service.ApplyMutation(ctx, env.UID, nested)
	require.NoError(t, err)
	second := workspaceSyncPutMutation(t, ctx, env, service, "docs/nested/b.md", 0, "bb")
	second.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000043")
	_, err = service.ApplyMutation(ctx, env.UID, second)
	require.NoError(t, err)

	target := dto.WorkspacePath("archive")
	targetBase := dto.WorkspaceRevision(0)
	rename := dto.WorkspaceMutation{
		WorkspaceID:            workspaceSyncWorkspaceID,
		ClientID:               workspaceSyncClientID,
		OperationID:            dto.WorkspaceUUID("10000000-0000-4000-8000-000000000044"),
		Path:                   root.Path,
		BasePathRevision:       1,
		Kind:                   dto.WorkspaceMutationRename,
		ContentHash:            dto.WorkspaceNullableHash{Present: true},
		Metadata:               dto.WorkspaceFileMetadata{},
		NewPath:                &target,
		TargetBasePathRevision: &targetBase,
	}
	outcome, err := service.ApplyMutation(ctx, env.UID, rename)
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceRevision(5), outcome.Accepted.Revision)
	require.Equal(t, target, outcome.Accepted.PathState.Path)

	wantMoves := map[dto.WorkspacePath]dto.WorkspacePath{
		"docs":             "archive",
		"docs/a.md":        "archive/a.md",
		"docs/nested":      "archive/nested",
		"docs/nested/b.md": "archive/nested/b.md",
	}
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		for oldPath, newPath := range wantMoves {
			oldRecord, readErr := tx.Path(string(workspaceSyncWorkspaceID), oldPath)
			require.NoError(t, readErr)
			require.True(t, oldRecord.Tombstone)
			require.Equal(t, dto.WorkspaceRevision(5), oldRecord.PathRevision)
			newRecord, readErr := tx.Path(string(workspaceSyncWorkspaceID), newPath)
			require.NoError(t, readErr)
			require.False(t, newRecord.Tombstone)
			require.Equal(t, dto.WorkspaceRevision(5), newRecord.PathRevision)
		}
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, int64(4), workspace.LivePathCount)
		require.Equal(t, uint64(3), workspace.LiveBytes)
		firstBlob, readErr := tx.Blob(*first.ContentHash.Value)
		require.NoError(t, readErr)
		require.Equal(t, int64(2), firstBlob.RefCount)
		secondBlob, readErr := tx.Blob(*second.ContentHash.Value)
		require.NoError(t, readErr)
		require.Equal(t, int64(2), secondBlob.RefCount)
		return nil
	}))
}

func TestWorkspaceSyncDirectoryRenameCanReturnToTombstonedSubtree(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	root := workspaceSyncMkdirMutation("docs", 0, 140)
	_, err := service.ApplyMutation(ctx, env.UID, root)
	require.NoError(t, err)
	child := workspaceSyncPutMutation(t, ctx, env, service, "docs/a.md", 0, "a")
	child.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000141")
	_, err = service.ApplyMutation(ctx, env.UID, child)
	require.NoError(t, err)
	nested := workspaceSyncMkdirMutation("docs/nested", 0, 142)
	_, err = service.ApplyMutation(ctx, env.UID, nested)
	require.NoError(t, err)
	nestedChild := workspaceSyncPutMutation(t, ctx, env, service, "docs/nested/b.md", 0, "bb")
	nestedChild.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000143")
	_, err = service.ApplyMutation(ctx, env.UID, nestedChild)
	require.NoError(t, err)

	archive := dto.WorkspacePath("archive")
	missingRevision := dto.WorkspaceRevision(0)
	forward := dto.WorkspaceMutation{
		WorkspaceID:            workspaceSyncWorkspaceID,
		ClientID:               workspaceSyncClientID,
		OperationID:            dto.WorkspaceUUID("10000000-0000-4000-8000-000000000144"),
		Path:                   root.Path,
		BasePathRevision:       1,
		Kind:                   dto.WorkspaceMutationRename,
		ContentHash:            dto.WorkspaceNullableHash{Present: true},
		Metadata:               dto.WorkspaceFileMetadata{},
		NewPath:                &archive,
		TargetBasePathRevision: &missingRevision,
	}
	forwardOutcome, err := service.ApplyMutation(ctx, env.UID, forward)
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceRevision(5), forwardOutcome.Accepted.Revision)

	originalIDs := make(map[dto.WorkspacePath]int64)
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		for _, path := range []dto.WorkspacePath{"docs", "docs/a.md", "docs/nested", "docs/nested/b.md"} {
			record, readErr := tx.Path(string(workspaceSyncWorkspaceID), path)
			require.NoError(t, readErr)
			require.True(t, record.Tombstone)
			originalIDs[path] = record.ID
		}
		return nil
	}))

	docs := dto.WorkspacePath("docs")
	tombstoneRevision := dto.WorkspaceRevision(5)
	inverse := dto.WorkspaceMutation{
		WorkspaceID:            workspaceSyncWorkspaceID,
		ClientID:               workspaceSyncClientID,
		OperationID:            dto.WorkspaceUUID("10000000-0000-4000-8000-000000000145"),
		Path:                   archive,
		BasePathRevision:       5,
		Kind:                   dto.WorkspaceMutationRename,
		ContentHash:            dto.WorkspaceNullableHash{Present: true},
		Metadata:               dto.WorkspaceFileMetadata{},
		NewPath:                &docs,
		TargetBasePathRevision: &tombstoneRevision,
	}
	inverseOutcome, err := service.ApplyMutation(ctx, env.UID, inverse)
	require.NoError(t, err)
	require.NotNil(t, inverseOutcome.Accepted)
	require.Equal(t, dto.WorkspaceRevision(6), inverseOutcome.Accepted.Revision)

	replayed, err := service.ApplyMutation(ctx, env.UID, inverse)
	require.NoError(t, err)
	require.Equal(t, inverseOutcome.Accepted, replayed.Accepted)
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		moves := map[dto.WorkspacePath]dto.WorkspacePath{
			"docs": "archive", "docs/a.md": "archive/a.md",
			"docs/nested": "archive/nested", "docs/nested/b.md": "archive/nested/b.md",
		}
		for destination, source := range moves {
			destinationRecord, readErr := tx.Path(string(workspaceSyncWorkspaceID), destination)
			require.NoError(t, readErr)
			require.False(t, destinationRecord.Tombstone)
			require.Equal(t, dto.WorkspaceRevision(6), destinationRecord.PathRevision)
			require.Equal(t, originalIDs[destination], destinationRecord.ID)
			sourceRecord, readErr := tx.Path(string(workspaceSyncWorkspaceID), source)
			require.NoError(t, readErr)
			require.True(t, sourceRecord.Tombstone)
			require.Equal(t, dto.WorkspaceRevision(6), sourceRecord.PathRevision)
		}
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(6), workspace.GlobalRevision)
		require.Equal(t, int64(4), workspace.LivePathCount)
		require.Equal(t, uint64(3), workspace.LiveBytes)
		return nil
	}))

	duplicate := inverse
	duplicate.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000146")
	duplicateOutcome, err := service.ApplyMutation(ctx, env.UID, duplicate)
	require.NoError(t, err)
	require.Nil(t, duplicateOutcome.Accepted)
	require.NotNil(t, duplicateOutcome.Rejected)
	require.Equal(t, dto.WorkspaceMutationRejectStaleBase, duplicateOutcome.Rejected.Reason)
	require.Nil(t, duplicateOutcome.Conflict)
	require.NotNil(t, duplicateOutcome.Rejected.CurrentPathState)
	require.True(t, duplicateOutcome.Rejected.CurrentPathState.Tombstone)
}

func TestWorkspaceSyncRenameCollisionRollsBackWholeSubtree(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	root := workspaceSyncMkdirMutation("docs", 0, 50)
	_, err := service.ApplyMutation(ctx, env.UID, root)
	require.NoError(t, err)
	child := workspaceSyncPutMutation(t, ctx, env, service, "docs/a.md", 0, "source")
	child.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000051")
	_, err = service.ApplyMutation(ctx, env.UID, child)
	require.NoError(t, err)
	collision := workspaceSyncPutMutation(t, ctx, env, service, "archive/a.md", 0, "target")
	collision.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000052")
	_, err = service.ApplyMutation(ctx, env.UID, collision)
	require.NoError(t, err)

	target := dto.WorkspacePath("archive")
	targetBase := dto.WorkspaceRevision(0)
	rename := dto.WorkspaceMutation{
		WorkspaceID:            workspaceSyncWorkspaceID,
		ClientID:               workspaceSyncClientID,
		OperationID:            dto.WorkspaceUUID("10000000-0000-4000-8000-000000000053"),
		Path:                   root.Path,
		BasePathRevision:       1,
		Kind:                   dto.WorkspaceMutationRename,
		ContentHash:            dto.WorkspaceNullableHash{Present: true},
		Metadata:               dto.WorkspaceFileMetadata{},
		NewPath:                &target,
		TargetBasePathRevision: &targetBase,
	}
	outcome, err := service.ApplyMutation(ctx, env.UID, rename)
	require.Nil(t, outcome)
	require.ErrorContains(t, err, "destination collision")

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(3), workspace.GlobalRevision)
		for _, path := range []dto.WorkspacePath{"docs", "docs/a.md", "archive/a.md"} {
			record, pathErr := tx.Path(string(workspaceSyncWorkspaceID), path)
			require.NoError(t, pathErr)
			require.False(t, record.Tombstone)
		}
		_, readErr = tx.Path(string(workspaceSyncWorkspaceID), target)
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		_, readErr = tx.Operation(string(workspaceSyncClientID), string(rename.OperationID))
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		return nil
	}))
}

func TestWorkspaceSyncEnforcesLivePathAndByteLimitsBeforeRevisionAllocation(t *testing.T) {
	t.Run("live paths", func(t *testing.T) {
		env, service := workspaceSyncNewService(t)
		ctx := context.Background()
		workspaceSyncSubscribe(t, ctx, env, service)
		require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
			workspace, err := tx.Workspace(string(workspaceSyncWorkspaceID))
			if err != nil {
				return err
			}
			workspace.LivePathCount = workspaceSyncMaxLivePaths
			return tx.SaveWorkspace(*workspace)
		}))
		mutation := workspaceSyncMkdirMutation("over-limit", 0, 60)
		outcome, err := service.ApplyMutation(ctx, env.UID, mutation)
		require.Nil(t, outcome)
		workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorWorkspaceLimitExceeded)
		workspaceSyncRequireUnallocatedMutation(t, ctx, env, mutation)
	})

	t.Run("live bytes", func(t *testing.T) {
		env, service := workspaceSyncNewService(t)
		ctx := context.Background()
		workspaceSyncSubscribe(t, ctx, env, service)
		require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
			workspace, err := tx.Workspace(string(workspaceSyncWorkspaceID))
			if err != nil {
				return err
			}
			workspace.LiveBytes = workspaceSyncMaxLiveBytes
			return tx.SaveWorkspace(*workspace)
		}))
		mutation := workspaceSyncPutMutation(t, ctx, env, service, "over-limit.md", 0, "x")
		mutation.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000061")
		outcome, err := service.ApplyMutation(ctx, env.UID, mutation)
		require.Nil(t, outcome)
		workspaceSyncRequireServiceError(t, err, dto.WorkspaceErrorWorkspaceLimitExceeded)
		workspaceSyncRequireUnallocatedMutation(t, ctx, env, mutation)
	})
}

func TestWorkspaceSyncAccepts4096ByteCanonicalPath(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	path := dto.WorkspacePath("a/" + strings.Repeat("b", 4094))
	require.Len(t, []byte(path), 4096)
	mutation := workspaceSyncMkdirMutation(path, 0, 62)

	outcome, err := service.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.NotNil(t, outcome.Accepted)
	require.Equal(t, path, outcome.Accepted.PathState.Path)
	require.Equal(t, dto.WorkspaceRevision(1), outcome.Accepted.PathState.PathRevision)
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		stored, readErr := tx.Path(string(workspaceSyncWorkspaceID), path)
		require.NoError(t, readErr)
		require.Equal(t, path, stored.Path)
		return nil
	}))
}

func TestWorkspaceSyncApplyMutationGCClaimBetweenHintAndTransactionCannotCommitReference(t *testing.T) {
	env, service := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, service)
	mutation := workspaceSyncPutMutation(t, ctx, env, service, "notes/gc-race.md", 0, "claim before finalize")
	hash := *mutation.ContentHash.Value
	has, err := service.blobStore.Has(ctx, env.UID, hash, mutation.Metadata.Size)
	require.NoError(t, err)
	require.True(t, has)

	now := time.Now().UTC()
	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		blob, readErr := tx.Blob(hash)
		if readErr != nil {
			return readErr
		}
		blob.UnreferencedAt = workspaceSyncTimePointer(now.Add(-2 * time.Hour))
		return tx.SaveBlob(*blob)
	}))

	claimed := make(chan struct{})
	releaseClaim := make(chan struct{})
	claimErr := make(chan error, 1)
	go func() {
		claimErr <- env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
			ok, claimReadErr := tx.ClaimBlobForGC(hash, now.Add(-time.Hour))
			if claimReadErr != nil {
				return claimReadErr
			}
			if !ok {
				return errors.New("expected blob GC claim")
			}
			close(claimed)
			<-releaseClaim
			return nil
		})
	}()
	<-claimed

	mutationStarted := make(chan struct{})
	mutationResult := make(chan *WorkspaceMutationOutcome, 1)
	mutationErr := make(chan error, 1)
	go func() {
		close(mutationStarted)
		outcome, applyErr := service.ApplyMutation(ctx, env.UID, mutation)
		mutationResult <- outcome
		mutationErr <- applyErr
	}()
	<-mutationStarted
	close(releaseClaim)
	require.NoError(t, <-claimErr)
	outcome := <-mutationResult
	require.NoError(t, <-mutationErr)
	require.Nil(t, outcome.Accepted)
	require.NotNil(t, outcome.Rejected)
	require.Equal(t, dto.WorkspaceMutationRejectBlobRequired, outcome.Rejected.Reason)
	require.Equal(t, &hash, outcome.Rejected.RequiredHash)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Zero(t, workspace.GlobalRevision)
		_, readErr = tx.Path(string(workspaceSyncWorkspaceID), mutation.Path)
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, 1)
		require.NoError(t, readErr)
		require.Empty(t, events)
		operation, readErr := tx.Operation(string(workspaceSyncClientID), string(mutation.OperationID))
		require.NoError(t, readErr)
		require.Equal(t, "waiting_blob", operation.State)
		_, readErr = tx.Blob(hash)
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		return nil
	}))
}

func TestWorkspaceSyncConcurrentSameOperationAcrossServiceInstancesReplaysWinner(t *testing.T) {
	env := workspaceSyncNewIndependentWritersEnv(t)
	ctx := context.Background()
	workspaceSyncSubscribeWithRepository(t, ctx, env.UID, env.FirstRepo, env.FirstService, workspaceSyncWorkspaceID)
	content := []byte("same operation race")
	hash := workspaceBlobStoreHash(content)
	require.NoError(t, env.FirstService.blobStore.Put(ctx, env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
	mutation := workspaceSyncMutationForWorkspace(workspaceSyncWorkspaceID, "notes/race.md", hash, content, 70)

	delayed := newWorkspaceSyncDelayedOperationRepository(
		env.SecondRepo, string(mutation.ClientID), string(mutation.OperationID), 1,
	)
	loserService := NewWorkspaceSyncService(delayed, env.SecondService.blobStore)
	loserResult := make(chan workspaceSyncApplyResult, 1)
	go func() {
		outcome, err := loserService.ApplyMutation(ctx, env.UID, mutation)
		loserResult <- workspaceSyncApplyResult{outcome: outcome, err: err}
	}()
	<-delayed.observedMissing

	winner, err := env.FirstService.ApplyMutation(ctx, env.UID, mutation)
	require.NoError(t, err)
	require.NotNil(t, winner.Accepted)
	winnerReceipt := workspaceSyncReadOperation(t, ctx, env.FirstRepo, env.UID, mutation)
	close(delayed.release)
	loser := <-loserResult
	require.NoError(t, loser.err)
	require.NotNil(t, loser.outcome.Accepted)
	winnerJSON, err := json.Marshal(winner.Accepted)
	require.NoError(t, err)
	loserJSON, err := json.Marshal(loser.outcome.Accepted)
	require.NoError(t, err)
	require.Equal(t, winnerJSON, loserJSON)

	finalReceipt := workspaceSyncReadOperation(t, ctx, env.FirstRepo, env.UID, mutation)
	workspaceSyncRequireSameOperationRecord(t, winnerReceipt, finalReceipt)
	require.Equal(t, string(dto.WorkspaceActionMutationAccepted), *finalReceipt.ResultAction)
	require.Equal(t, winnerJSON, finalReceipt.ResultJSON)
	require.NoError(t, env.FirstRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(1), workspace.GlobalRevision)
		events, readErr := tx.EventsAfter(string(workspaceSyncWorkspaceID), 0, 2)
		require.NoError(t, readErr)
		require.Len(t, events, 1)
		return nil
	}))
}

func TestWorkspaceSyncConcurrentSameOperationAcrossWorkspacesPreservesWinnerReceipt(t *testing.T) {
	env := workspaceSyncNewIndependentWritersEnv(t)
	ctx := context.Background()
	otherWorkspace := dto.WorkspaceUUID("10000000-0000-4000-8000-000000000071")
	workspaceSyncSubscribeWithRepository(t, ctx, env.UID, env.FirstRepo, env.FirstService, workspaceSyncWorkspaceID)
	workspaceSyncSubscribeWithRepository(t, ctx, env.UID, env.FirstRepo, env.FirstService, otherWorkspace)
	content := []byte("cross workspace operation race")
	hash := workspaceBlobStoreHash(content)
	require.NoError(t, env.FirstService.blobStore.Put(ctx, env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
	winnerMutation := workspaceSyncMutationForWorkspace(workspaceSyncWorkspaceID, "notes/a.md", hash, content, 72)
	loserMutation := workspaceSyncMutationForWorkspace(otherWorkspace, "notes/b.md", hash, content, 72)

	delayed := newWorkspaceSyncDelayedOperationRepository(
		env.SecondRepo, string(loserMutation.ClientID), string(loserMutation.OperationID), 2,
	)
	loserService := NewWorkspaceSyncService(delayed, env.SecondService.blobStore)
	loserResult := make(chan workspaceSyncApplyResult, 1)
	go func() {
		outcome, err := loserService.ApplyMutation(ctx, env.UID, loserMutation)
		loserResult <- workspaceSyncApplyResult{outcome: outcome, err: err}
	}()
	<-delayed.observedMissing

	winner, err := env.FirstService.ApplyMutation(ctx, env.UID, winnerMutation)
	require.NoError(t, err)
	require.NotNil(t, winner.Accepted)
	winnerReceipt := workspaceSyncReadOperation(t, ctx, env.FirstRepo, env.UID, winnerMutation)
	close(delayed.release)
	loser := <-loserResult
	require.NoError(t, loser.err)
	require.Nil(t, loser.outcome.Accepted)
	require.NotNil(t, loser.outcome.Rejected)
	require.Equal(t, dto.WorkspaceMutationRejectOperationReused, loser.outcome.Rejected.Reason)

	finalReceipt := workspaceSyncReadOperation(t, ctx, env.FirstRepo, env.UID, winnerMutation)
	workspaceSyncRequireSameOperationRecord(t, winnerReceipt, finalReceipt)
	winnerJSON, err := json.Marshal(winner.Accepted)
	require.NoError(t, err)
	require.Equal(t, winnerJSON, finalReceipt.ResultJSON)
	require.Equal(t, string(winnerMutation.WorkspaceID), finalReceipt.WorkspaceID)
	require.NoError(t, env.FirstRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		winnerWorkspace, readErr := tx.Workspace(string(winnerMutation.WorkspaceID))
		require.NoError(t, readErr)
		require.Equal(t, dto.WorkspaceRevision(1), winnerWorkspace.GlobalRevision)
		loserWorkspace, readErr := tx.Workspace(string(loserMutation.WorkspaceID))
		require.NoError(t, readErr)
		require.Zero(t, loserWorkspace.GlobalRevision)
		winnerEvents, readErr := tx.EventsAfter(string(winnerMutation.WorkspaceID), 0, 2)
		require.NoError(t, readErr)
		require.Len(t, winnerEvents, 1)
		loserEvents, readErr := tx.EventsAfter(string(loserMutation.WorkspaceID), 0, 2)
		require.NoError(t, readErr)
		require.Empty(t, loserEvents)
		return nil
	}))
}

type workspaceSyncIndependentWritersEnv struct {
	UID           int64
	FirstRepo     domain.WorkspaceRepository
	SecondRepo    domain.WorkspaceRepository
	FirstService  *workspaceSyncService
	SecondService *workspaceSyncService
}

func workspaceSyncNewIndependentWritersEnv(t *testing.T) *workspaceSyncIndependentWritersEnv {
	t.Helper()
	const uid int64 = 73
	tempDir := t.TempDir()
	queueDisabled := false
	dbConfig := config.DatabaseConfig{
		Type:             "sqlite",
		Path:             filepath.Join(tempDir, "database", "main.sqlite3"),
		EnableWriteQueue: &queueDisabled,
		MaxOpenConns:     4,
	}
	logger := zap.NewNop()
	mainDBs := make([]*gorm.DB, 2)
	daos := make([]*dao.Dao, 2)
	repos := make([]domain.WorkspaceRepository, 2)
	for i := range 2 {
		mainDB, err := dao.NewEngine(dbConfig, logger)
		require.NoError(t, err)
		mainDBs[i] = mainDB
		daos[i] = dao.New(
			mainDB,
			context.Background(),
			dao.WithConfig(&dbConfig),
			dao.WithUserDatabaseConfig(&dbConfig),
			dao.WithLogger(logger),
		)
		repos[i] = dao.NewWorkspaceRepository(daos[i])
		require.NoError(t, repos[i].Migrate(context.Background(), uid))
	}
	for i := range 2 {
		userDB := daos[i].ResolveDB("user_workspace_73")
		t.Cleanup(func() { workspaceSyncCloseDB(t, userDB) })
		mainDB := mainDBs[i]
		t.Cleanup(func() { workspaceSyncCloseDB(t, mainDB) })
	}
	blobRoot := filepath.Join(tempDir, "workspace-blobs")
	firstStore := NewWorkspaceBlobStore(repos[0], workspaceBlobStoreConfig(t, blobRoot))
	secondStore := NewWorkspaceBlobStore(repos[1], workspaceBlobStoreConfig(t, blobRoot))
	return &workspaceSyncIndependentWritersEnv{
		UID:           uid,
		FirstRepo:     repos[0],
		SecondRepo:    repos[1],
		FirstService:  NewWorkspaceSyncService(repos[0], firstStore),
		SecondService: NewWorkspaceSyncService(repos[1], secondStore),
	}
}

func workspaceSyncCloseDB(t *testing.T, db *gorm.DB) {
	t.Helper()
	sqlDB, err := db.DB()
	if err == nil {
		require.NoError(t, sqlDB.Close())
	}
}

func workspaceSyncSubscribeWithRepository(
	t *testing.T,
	ctx context.Context,
	uid int64,
	repo domain.WorkspaceRepository,
	service *workspaceSyncService,
	workspaceID dto.WorkspaceUUID,
) {
	t.Helper()
	_, err := service.Subscribe(ctx, uid, dto.WorkspaceSubscribeRequest{
		WorkspaceID: workspaceID,
		ClientID:    workspaceSyncClientID,
	})
	require.NoError(t, err)
	require.NoError(t, repo.Read(ctx, uid, func(tx domain.WorkspaceReadTx) error {
		_, readErr := tx.Client(string(workspaceID), string(workspaceSyncClientID))
		return readErr
	}))
}

func workspaceSyncMutationForWorkspace(
	workspaceID dto.WorkspaceUUID,
	path dto.WorkspacePath,
	hash dto.WorkspaceContentHash,
	content []byte,
	operation int,
) dto.WorkspaceMutation {
	return dto.WorkspaceMutation{
		WorkspaceID:      workspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      dto.WorkspaceUUID(fmt.Sprintf("10000000-0000-4000-8000-%012d", operation)),
		Path:             path,
		BasePathRevision: 0,
		Kind:             dto.WorkspaceMutationUpsertFile,
		ContentHash:      workspaceSyncNullableHash(hash),
		Metadata:         dto.WorkspaceFileMetadata{Size: uint64(len(content))},
	}
}

type workspaceSyncApplyResult struct {
	outcome *WorkspaceMutationOutcome
	err     error
}

type workspaceSyncDelayedOperationRepository struct {
	domain.WorkspaceRepository
	clientID        string
	operationID     string
	staleReads      int
	observedMissing chan struct{}
	release         chan struct{}
}

func newWorkspaceSyncDelayedOperationRepository(
	repo domain.WorkspaceRepository,
	clientID string,
	operationID string,
	staleReads int,
) *workspaceSyncDelayedOperationRepository {
	return &workspaceSyncDelayedOperationRepository{
		WorkspaceRepository: repo,
		clientID:            clientID,
		operationID:         operationID,
		staleReads:          staleReads,
		observedMissing:     make(chan struct{}),
		release:             make(chan struct{}),
	}
}

func (r *workspaceSyncDelayedOperationRepository) Write(
	ctx context.Context,
	uid int64,
	fn func(domain.WorkspaceWriteTx) error,
) error {
	err := r.WorkspaceRepository.Read(ctx, uid, func(tx domain.WorkspaceReadTx) error {
		_, readErr := tx.Operation(r.clientID, r.operationID)
		return readErr
	})
	if !errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
		return fmt.Errorf("delayed operation precondition: %w", err)
	}
	close(r.observedMissing)
	select {
	case <-r.release:
	case <-ctx.Done():
		return ctx.Err()
	}
	remaining := r.staleReads
	return r.WorkspaceRepository.Write(ctx, uid, func(tx domain.WorkspaceWriteTx) error {
		return fn(&workspaceSyncStaleOperationWriteTx{
			WorkspaceWriteTx: tx,
			clientID:         r.clientID,
			operationID:      r.operationID,
			remaining:        &remaining,
		})
	})
}

type workspaceSyncStaleOperationWriteTx struct {
	domain.WorkspaceWriteTx
	clientID    string
	operationID string
	remaining   *int
}

func (tx *workspaceSyncStaleOperationWriteTx) Operation(
	clientID,
	operationID string,
) (*domain.WorkspaceOperationRecord, error) {
	if clientID == tx.clientID && operationID == tx.operationID && *tx.remaining > 0 {
		*tx.remaining--
		return nil, domain.ErrWorkspaceRecordNotFound
	}
	return tx.WorkspaceWriteTx.Operation(clientID, operationID)
}

func workspaceSyncReadOperation(
	t *testing.T,
	ctx context.Context,
	repo domain.WorkspaceRepository,
	uid int64,
	mutation dto.WorkspaceMutation,
) domain.WorkspaceOperationRecord {
	t.Helper()
	var result domain.WorkspaceOperationRecord
	require.NoError(t, repo.Read(ctx, uid, func(tx domain.WorkspaceReadTx) error {
		operation, err := tx.Operation(string(mutation.ClientID), string(mutation.OperationID))
		if err == nil {
			result = *operation
			result.ResultJSON = append([]byte(nil), operation.ResultJSON...)
		}
		return err
	}))
	return result
}

func workspaceSyncRequireSameOperationRecord(
	t *testing.T,
	want domain.WorkspaceOperationRecord,
	got domain.WorkspaceOperationRecord,
) {
	t.Helper()
	require.Equal(t, want.WorkspaceID, got.WorkspaceID)
	require.Equal(t, want.ClientID, got.ClientID)
	require.Equal(t, want.OperationID, got.OperationID)
	require.Equal(t, want.RequestKind, got.RequestKind)
	require.Equal(t, want.RequestDigest, got.RequestDigest)
	require.Equal(t, want.State, got.State)
	require.Equal(t, want.ResultAction, got.ResultAction)
	require.Equal(t, want.ResultJSON, got.ResultJSON)
}

func workspaceSyncTimePointer(value time.Time) *time.Time {
	return &value
}

func workspaceSyncRequireUnallocatedMutation(
	t *testing.T,
	ctx context.Context,
	env *testutil.WorkspaceEnv,
	mutation dto.WorkspaceMutation,
) {
	t.Helper()
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, readErr := tx.Workspace(string(workspaceSyncWorkspaceID))
		require.NoError(t, readErr)
		require.Zero(t, workspace.GlobalRevision)
		_, readErr = tx.Path(string(workspaceSyncWorkspaceID), mutation.Path)
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		_, readErr = tx.Operation(string(workspaceSyncClientID), string(mutation.OperationID))
		require.ErrorIs(t, readErr, domain.ErrWorkspaceRecordNotFound)
		return nil
	}))
}

func workspaceSyncMkdirMutation(path dto.WorkspacePath, base dto.WorkspaceRevision, operation int) dto.WorkspaceMutation {
	return dto.WorkspaceMutation{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      dto.WorkspaceUUID(fmt.Sprintf("10000000-0000-4000-8000-%012d", operation)),
		Path:             path,
		BasePathRevision: base,
		Kind:             dto.WorkspaceMutationMkdir,
		ContentHash:      dto.WorkspaceNullableHash{Present: true},
		Metadata:         dto.WorkspaceFileMetadata{},
	}
}

type workspaceSyncFailAfterCallbackRepository struct {
	domain.WorkspaceRepository
	err error
}

func (r *workspaceSyncFailAfterCallbackRepository) Write(
	ctx context.Context,
	uid int64,
	fn func(domain.WorkspaceWriteTx) error,
) error {
	return r.WorkspaceRepository.Write(ctx, uid, func(tx domain.WorkspaceWriteTx) error {
		if err := fn(tx); err != nil {
			return err
		}
		return r.err
	})
}

func workspaceSyncNewService(t *testing.T) (*testutil.WorkspaceEnv, *workspaceSyncService) {
	t.Helper()
	env := testutil.NewWorkspaceEnv(t)
	service := NewWorkspaceSyncService(
		env.WorkspaceRepo,
		NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot)),
	)
	return env, service
}

func workspaceSyncSubscribe(
	t *testing.T,
	ctx context.Context,
	env *testutil.WorkspaceEnv,
	service *workspaceSyncService,
) {
	t.Helper()
	_, err := service.Subscribe(ctx, env.UID, dto.WorkspaceSubscribeRequest{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncClientID,
	})
	require.NoError(t, err)
}

func workspaceSyncPutMutation(
	t *testing.T,
	ctx context.Context,
	env *testutil.WorkspaceEnv,
	service *workspaceSyncService,
	path dto.WorkspacePath,
	base dto.WorkspaceRevision,
	content string,
) dto.WorkspaceMutation {
	t.Helper()
	hash := workspaceBlobStoreHash([]byte(content))
	require.NoError(t, service.blobStore.Put(ctx, env.UID, hash, uint64(len(content)), bytes.NewBufferString(content)))
	return dto.WorkspaceMutation{
		WorkspaceID:      workspaceSyncWorkspaceID,
		ClientID:         workspaceSyncClientID,
		OperationID:      dto.WorkspaceUUID("10000000-0000-4000-8000-000000000010"),
		Path:             path,
		BasePathRevision: base,
		Kind:             dto.WorkspaceMutationUpsertFile,
		ContentHash: dto.WorkspaceNullableHash{
			Present: true,
			Value:   &hash,
		},
		Metadata: dto.WorkspaceFileMetadata{Size: uint64(len(content))},
	}
}

func workspaceSyncRequireServiceError(t *testing.T, err error, code dto.WorkspaceV2ErrorCode) {
	t.Helper()
	var serviceErr *WorkspaceServiceError
	require.ErrorAs(t, err, &serviceErr)
	require.Equal(t, code, serviceErr.Code)
}
