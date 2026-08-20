package websocket_router

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"testing"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/service"
	"github.com/haierkeys/fast-note-sync-service/internal/testutil"
	"github.com/stretchr/testify/require"
)

func TestWorkspaceV2AckReplayAfterLostSuccessReturnsSameCorrelatedResponse(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	blobStore := &workspaceV2AckReplayBlobStore{}
	initialService := service.NewWorkspaceSyncService(env.WorkspaceRepo, blobStore)
	ctx := context.Background()
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)

	_, err := initialService.Subscribe(ctx, env.UID, dto.WorkspaceSubscribeRequest{
		WorkspaceID: workspaceID,
		ClientID:    clientID,
	})
	require.NoError(t, err)
	outcome, err := initialService.ApplyMutation(ctx, env.UID, dto.WorkspaceMutation{
		WorkspaceID:      workspaceID,
		ClientID:         clientID,
		OperationID:      dto.WorkspaceUUID("10000000-0000-4000-8000-000000000301"),
		Path:             dto.WorkspacePath("notes/ack-replay"),
		BasePathRevision: 0,
		Kind:             dto.WorkspaceMutationMkdir,
		ContentHash:      dto.WorkspaceNullableHash{Present: true},
		Metadata:         dto.WorkspaceFileMetadata{},
	})
	require.NoError(t, err)
	require.NotNil(t, outcome.Accepted)

	ack := dto.WorkspaceAckRequest{
		WorkspaceID: workspaceID,
		ClientID:    clientID,
		Revision:    outcome.Accepted.Revision,
	}
	require.NoError(t, initialService.Acknowledge(ctx, env.UID, ack, ack.Revision))

	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.syncService = service.NewWorkspaceSyncService(env.WorkspaceRepo, blobStore)
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000302")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe,
		"10000000-0000-4000-8000-000000000303",
		workspaceV2SubscribeData(string(workspaceID), string(clientID), 0),
	)
	beginFrame := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionSnapshotBegin), workspaceV2Action(beginFrame))
	var beginEnvelope dto.WorkspaceV2Response[dto.WorkspaceSnapshotBeginMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(beginFrame), &beginEnvelope))
	require.NotNil(t, beginEnvelope.Data)
	require.Zero(t, beginEnvelope.Data.FromRevision)
	require.Equal(t, ack.Revision, beginEnvelope.Data.FinalRevision)
	require.Equal(t, string(dto.WorkspaceActionSnapshotEntry), workspaceV2Action(workspaceV2Receive(t, events)))
	require.Equal(t, string(dto.WorkspaceActionSnapshotEnd), workspaceV2Action(workspaceV2Receive(t, events)))

	requestID := dto.WorkspaceUUID("10000000-0000-4000-8000-000000000304")
	workspaceV2Send(t, conn, dto.WorkspaceActionAck, string(requestID), ack)
	response := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionAck), workspaceV2Action(response))

	var envelope dto.WorkspaceV2Response[dto.WorkspaceAckRequest]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(response), &envelope))
	require.True(t, envelope.Status)
	require.Nil(t, envelope.Error)
	require.NotNil(t, envelope.RequestID)
	require.Equal(t, requestID, *envelope.RequestID)
	require.NotNil(t, envelope.Data)
	require.Equal(t, ack, *envelope.Data)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		client, readErr := tx.Client(string(workspaceID), string(clientID))
		require.NoError(t, readErr)
		require.Equal(t, ack.Revision, client.LastAckRevision)
		return nil
	}))
}

type workspaceV2AckReplayBlobStore struct {
}

var errWorkspaceV2AckUnexpectedBlobAccess = errors.New("unexpected blob access during Ack replay test")

func (*workspaceV2AckReplayBlobStore) Has(
	context.Context,
	int64,
	dto.WorkspaceContentHash,
	uint64,
) (bool, error) {
	return false, errWorkspaceV2AckUnexpectedBlobAccess
}

func (*workspaceV2AckReplayBlobStore) Put(
	context.Context,
	int64,
	dto.WorkspaceContentHash,
	uint64,
	io.Reader,
) error {
	return errWorkspaceV2AckUnexpectedBlobAccess
}

func (*workspaceV2AckReplayBlobStore) Open(
	context.Context,
	int64,
	dto.WorkspaceContentHash,
) (io.ReadCloser, uint64, error) {
	return nil, 0, errWorkspaceV2AckUnexpectedBlobAccess
}

func (*workspaceV2AckReplayBlobStore) ReconcileAndGC(context.Context, int64, time.Time) error {
	return errWorkspaceV2AckUnexpectedBlobAccess
}

var _ service.WorkspaceBlobStore = (*workspaceV2AckReplayBlobStore)(nil)
