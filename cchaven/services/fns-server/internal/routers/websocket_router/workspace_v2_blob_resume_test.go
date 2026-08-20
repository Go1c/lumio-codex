package websocket_router

import (
	"encoding/json"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/haierkeys/fast-note-sync-service/internal/config"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/middleware"
	"github.com/haierkeys/fast-note-sync-service/internal/service"
	"github.com/haierkeys/fast-note-sync-service/internal/testutil"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
	"github.com/stretchr/testify/require"
)

func TestWorkspaceV2MutationBlobNeedResumeOnExactRetry(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	blobConfig := workspaceV2BlobResumeConfig(t, env.BlobRoot)
	authenticate := func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	}
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000118")
	content := []byte("retry the exact mutation after a service restart")
	hash := workspaceV2TestBlobHash(content)
	mutation := dto.WorkspaceMutation{
		WorkspaceID:      workspaceID,
		ClientID:         clientID,
		OperationID:      operationID,
		Path:             "notes/resume-upload.md",
		BasePathRevision: 0,
		Kind:             dto.WorkspaceMutationUpsertFile,
		ContentHash:      dto.WorkspaceNullableHash{Present: true, Value: &hash},
		Metadata:         dto.WorkspaceFileMetadata{Size: uint64(len(content))},
	}

	firstServer, firstHTTP := workspaceV2BlobResumeServer(t, env, blobConfig, authenticate)
	firstConn, firstEvents := newWorkspaceV2StreamClient(t, firstHTTP)
	workspaceV2Hello(t, firstConn, firstEvents, "10000000-0000-4000-8000-000000000118")
	workspaceV2Send(t, firstConn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000119", workspaceV2SubscribeData(string(workspaceID), string(clientID), 0))
	for range 2 {
		workspaceV2Receive(t, firstEvents)
	}
	workspaceV2Send(t, firstConn, dto.WorkspaceActionMutation, "10000000-0000-4000-8000-000000000120", mutation)
	firstRejected := workspaceV2BlobResumeRejected(t, workspaceV2Receive(t, firstEvents))
	firstNeed := workspaceV2BlobResumeNeed(t, workspaceV2Receive(t, firstEvents))
	require.NoError(t, firstConn.NetConn().Close())
	firstServer.Close()
	require.NoError(t, firstServer.WaitAllClosed(time.Second))
	firstHTTP.Close()

	restartedServer, restartedHTTP := workspaceV2BlobResumeServer(t, env, blobConfig, authenticate)
	restartedConn, restartedEvents := newWorkspaceV2StreamClient(t, restartedHTTP)
	workspaceV2Hello(t, restartedConn, restartedEvents, "10000000-0000-4000-8000-000000000121")
	workspaceV2Send(t, restartedConn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000122", workspaceV2SubscribeData(string(workspaceID), string(clientID), 0))
	for range 2 {
		workspaceV2Receive(t, restartedEvents)
	}
	workspaceV2Send(t, restartedConn, dto.WorkspaceActionMutation, "10000000-0000-4000-8000-000000000123", mutation)
	replayedRejected := workspaceV2BlobResumeRejected(t, workspaceV2Receive(t, restartedEvents))
	replayedNeed := workspaceV2BlobResumeNeed(t, workspaceV2Receive(t, restartedEvents))

	require.Equal(t, firstRejected, replayedRejected)
	require.Equal(t, firstNeed, replayedNeed)
	require.Equal(t, operationID, replayedNeed.OperationID)
	require.Equal(t, hash, replayedNeed.ContentHash)
	require.Equal(t, uint64(len(content)), replayedNeed.Size)
	restartedServer.Close()
	require.NoError(t, restartedServer.WaitAllClosed(time.Second))
}

func workspaceV2BlobResumeServer(
	t *testing.T,
	env *testutil.WorkspaceEnv,
	blobConfig *config.WorkspaceConfig,
	authenticate workspaceV2Authenticator,
) (*WorkspaceV2Server, *httptest.Server) {
	t.Helper()
	blobStore := service.NewWorkspaceBlobStore(env.WorkspaceRepo, blobConfig)
	server, httpServer := newWorkspaceV2HTTPTestServer(t, authenticate)
	server.syncService = service.NewWorkspaceSyncService(env.WorkspaceRepo, blobStore)
	server.blobStore = blobStore
	return server, httpServer
}

func workspaceV2BlobResumeConfig(t *testing.T, root string) *config.WorkspaceConfig {
	t.Helper()
	cfg := &config.WorkspaceConfig{
		BlobPath: root, MaxPaths: 50_000, MaxBytes: dto.WorkspaceMaxBlobBytes,
		EventRetention: "30d", EventMaxPerWorkspace: 100_000,
		BlobGCGrace: "1h", StagingTTL: "1h", PruneBatchSize: 500,
		MaxWorkspacesPerUser: config.WorkspaceMaxPerUser,
	}
	require.NoError(t, cfg.Validate())
	return cfg
}

func workspaceV2BlobResumeRejected(t *testing.T, frame []byte) dto.WorkspaceMutationRejectedMessage {
	t.Helper()
	require.Equal(t, string(dto.WorkspaceActionMutationRejected), workspaceV2Action(frame))
	var envelope dto.WorkspaceV2Response[dto.WorkspaceMutationRejectedMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(frame), &envelope))
	require.True(t, envelope.Status)
	require.NotNil(t, envelope.RequestID)
	require.NotNil(t, envelope.Data)
	require.Equal(t, dto.WorkspaceMutationRejectBlobRequired, envelope.Data.Reason)
	return *envelope.Data
}

func workspaceV2BlobResumeNeed(t *testing.T, frame []byte) dto.WorkspaceBlobNeedUploadPush {
	t.Helper()
	require.Equal(t, string(dto.WorkspaceActionBlobNeed), workspaceV2Action(frame))
	var envelope dto.WorkspaceV2Response[dto.WorkspaceBlobNeedUploadPush]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(frame), &envelope))
	require.True(t, envelope.Status)
	require.Nil(t, envelope.RequestID)
	require.NotNil(t, envelope.Data)
	return *envelope.Data
}
