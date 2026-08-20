package service

import (
	"context"
	"errors"
	"fmt"
	"testing"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/testutil"
	"github.com/stretchr/testify/require"
)

func TestWorkspaceSyncAcknowledgeExactDurableReplayIsIdempotent(t *testing.T) {
	env, syncService := workspaceSyncNewService(t)
	ctx := context.Background()
	ackTime := time.Date(2026, time.August, 9, 12, 0, 0, 0, time.UTC)
	syncService.now = func() time.Time { return ackTime }
	workspaceSyncSubscribe(t, ctx, env, syncService)

	first := workspaceSyncPutMutation(t, ctx, env, syncService, "notes/ack-one.md", 0, "one")
	firstOutcome, err := syncService.ApplyMutation(ctx, env.UID, first)
	require.NoError(t, err)
	second := workspaceSyncPutMutation(t, ctx, env, syncService, "notes/ack-two.md", 0, "two")
	second.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000011")
	secondOutcome, err := syncService.ApplyMutation(ctx, env.UID, second)
	require.NoError(t, err)
	require.Equal(t, firstOutcome.Accepted.Revision+1, secondOutcome.Accepted.Revision)

	ack := dto.WorkspaceAckRequest{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncClientID,
		Revision:    secondOutcome.Accepted.Revision,
	}
	require.NoError(t, syncService.Acknowledge(ctx, env.UID, ack, ack.Revision))
	beforeReplay := workspaceAckClient(t, ctx, env, workspaceSyncWorkspaceID, workspaceSyncClientID)

	restarted := NewWorkspaceSyncService(
		env.WorkspaceRepo,
		NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot)),
	)
	restarted.now = func() time.Time { return ackTime.Add(time.Hour) }
	require.NoError(t, restarted.Acknowledge(ctx, env.UID, ack, ack.Revision))

	afterReplay := workspaceAckClient(t, ctx, env, workspaceSyncWorkspaceID, workspaceSyncClientID)
	require.Equal(t, beforeReplay, afterReplay, "exact Ack replay must not rewrite durable client state")
}

func TestWorkspaceSyncAcknowledgeReplayKeepsRegressionCeilingAndIdentityGuards(t *testing.T) {
	env, syncService := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, syncService)

	first := workspaceSyncPutMutation(t, ctx, env, syncService, "notes/guard-one.md", 0, "one")
	firstOutcome, err := syncService.ApplyMutation(ctx, env.UID, first)
	require.NoError(t, err)
	second := workspaceSyncPutMutation(t, ctx, env, syncService, "notes/guard-two.md", 0, "two")
	second.OperationID = dto.WorkspaceUUID("10000000-0000-4000-8000-000000000012")
	secondOutcome, err := syncService.ApplyMutation(ctx, env.UID, second)
	require.NoError(t, err)

	durableAck := secondOutcome.Accepted.Revision
	require.NoError(t, syncService.Acknowledge(ctx, env.UID, dto.WorkspaceAckRequest{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncClientID,
		Revision:    durableAck,
	}, durableAck))

	t.Run("below durable Ack remains a regression", func(t *testing.T) {
		err := syncService.Acknowledge(ctx, env.UID, dto.WorkspaceAckRequest{
			WorkspaceID: workspaceSyncWorkspaceID,
			ClientID:    workspaceSyncClientID,
			Revision:    firstOutcome.Accepted.Revision,
		}, durableAck)
		workspaceAckRequireValidation(t, err, "revision", "ack_regression")
	})

	t.Run("above delivered ceiling remains rejected", func(t *testing.T) {
		err := syncService.Acknowledge(ctx, env.UID, dto.WorkspaceAckRequest{
			WorkspaceID: workspaceSyncWorkspaceID,
			ClientID:    workspaceSyncClientID,
			Revision:    durableAck + 1,
		}, durableAck)
		workspaceAckRequireValidation(t, err, "revision", "ack_overshoot")
	})

	t.Run("different client remains rejected", func(t *testing.T) {
		err := syncService.Acknowledge(ctx, env.UID, dto.WorkspaceAckRequest{
			WorkspaceID: workspaceSyncWorkspaceID,
			ClientID:    workspaceSyncOtherClient,
			Revision:    durableAck,
		}, durableAck)
		require.Error(t, err)
		require.True(t, errors.Is(err, domain.ErrWorkspaceRecordNotFound), "unexpected error: %v", err)
	})

	t.Run("different workspace remains rejected", func(t *testing.T) {
		err := syncService.Acknowledge(ctx, env.UID, dto.WorkspaceAckRequest{
			WorkspaceID: dto.WorkspaceUUID("10000000-0000-4000-8000-000000000099"),
			ClientID:    workspaceSyncClientID,
			Revision:    durableAck,
		}, durableAck)
		require.Error(t, err)
		require.True(t, errors.Is(err, domain.ErrWorkspaceRecordNotFound), "unexpected error: %v", err)
	})

	require.Equal(t, durableAck, workspaceAckClient(
		t, ctx, env, workspaceSyncWorkspaceID, workspaceSyncClientID,
	).LastAckRevision)
}

func TestWorkspaceSyncSubscribeReplaysFromRequestedAckBehindDurableClientAck(t *testing.T) {
	env, syncService := workspaceSyncNewService(t)
	ctx := context.Background()
	workspaceSyncSubscribe(t, ctx, env, syncService)

	accepted := make([]dto.WorkspaceRevision, 0, 5)
	for index := 1; index <= 5; index++ {
		mutation := workspaceSyncPutMutation(
			t,
			ctx,
			env,
			syncService,
			dto.WorkspacePath(fmt.Sprintf("notes/replay-%d.md", index)),
			0,
			fmt.Sprintf("event-%d", index),
		)
		mutation.OperationID = dto.WorkspaceUUID(fmt.Sprintf(
			"10000000-0000-4000-8000-%012d",
			100+index,
		))
		outcome, err := syncService.ApplyMutation(ctx, env.UID, mutation)
		require.NoError(t, err)
		require.NotNil(t, outcome.Accepted)
		accepted = append(accepted, outcome.Accepted.Revision)
	}
	require.Equal(t, []dto.WorkspaceRevision{1, 2, 3, 4, 5}, accepted)

	durableAck := accepted[len(accepted)-1]
	require.NoError(t, syncService.Acknowledge(ctx, env.UID, dto.WorkspaceAckRequest{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncClientID,
		Revision:    durableAck,
	}, durableAck))

	restarted := NewWorkspaceSyncService(
		env.WorkspaceRepo,
		NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot)),
	)
	requestedAck := accepted[0]
	changeSet, err := restarted.Subscribe(ctx, env.UID, dto.WorkspaceSubscribeRequest{
		WorkspaceID:     workspaceSyncWorkspaceID,
		ClientID:        workspaceSyncClientID,
		LastAckRevision: requestedAck,
	})
	require.NoError(t, err)
	require.Equal(t, dto.WorkspaceSnapshotIncremental, changeSet.Mode)
	require.Equal(t, requestedAck, changeSet.FromRevision)
	require.Equal(t, durableAck, changeSet.FinalRevision)
	require.Len(t, changeSet.RevisionItems, 4)
	for index, item := range changeSet.RevisionItems {
		require.Equal(t, accepted[index+1], item.Revision)
		require.NotNil(t, item.Event)
	}
	require.Equal(t, durableAck, workspaceAckClient(
		t, ctx, env, workspaceSyncWorkspaceID, workspaceSyncClientID,
	).LastAckRevision, "Subscribe replay must not regress the durable server Ack")

	require.NoError(t, restarted.Acknowledge(ctx, env.UID, dto.WorkspaceAckRequest{
		WorkspaceID: workspaceSyncWorkspaceID,
		ClientID:    workspaceSyncClientID,
		Revision:    durableAck,
	}, durableAck))
	require.Equal(t, durableAck, workspaceAckClient(
		t, ctx, env, workspaceSyncWorkspaceID, workspaceSyncClientID,
	).LastAckRevision)
}

func workspaceAckClient(
	t *testing.T,
	ctx context.Context,
	env *testutil.WorkspaceEnv,
	workspaceID,
	clientID dto.WorkspaceUUID,
) domain.WorkspaceClientRecord {
	t.Helper()
	var result domain.WorkspaceClientRecord
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		client, err := tx.Client(string(workspaceID), string(clientID))
		if err == nil {
			result = *client
		}
		return err
	}))
	return result
}

func workspaceAckRequireValidation(t *testing.T, err error, field, reason string) {
	t.Helper()
	var validationErr *dto.WorkspaceValidationError
	require.ErrorAs(t, err, &validationErr)
	require.Equal(t, field, validationErr.Field)
	require.Equal(t, reason, validationErr.Reason)
}
