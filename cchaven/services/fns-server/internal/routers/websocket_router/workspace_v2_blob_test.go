package websocket_router

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"runtime"
	"sync"
	"testing"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/google/uuid"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/middleware"
	"github.com/haierkeys/fast-note-sync-service/internal/service"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
	"github.com/lxzan/gws"
	"github.com/stretchr/testify/require"
)

func TestWorkspaceV2TransferLimitsAcrossConnectionWorkspaceAndUser(t *testing.T) {
	manager := newWorkspaceV2TransferManager()
	workspaceA := dto.WorkspaceUUID("10000000-0000-4000-8000-000000000001")
	workspaceB := dto.WorkspaceUUID("20000000-0000-4000-8000-000000000002")
	connections := make([]*workspaceV2Connection, 0, 9)
	for index := 0; index < 8; index++ {
		connections = append(connections, &workspaceV2Connection{
			uid:           41,
			transfers:     make(map[uuid.UUID]*workspaceV2Transfer),
			seenTransfers: make(map[uuid.UUID]struct{}),
		})
	}
	for index := 0; index < workspaceV2MaxTransfersPerWorkspace; index++ {
		connection := connections[index%len(connections)]
		connection.uid = 41
		transfer := workspaceV2TestTransfer(connection, workspaceA, time.Now())
		require.NoError(t, manager.reserve(connection, transfer))
	}
	require.Error(t, manager.reserve(connections[0], workspaceV2TestTransfer(connections[0], workspaceA, time.Now())))

	for index := 0; index < workspaceV2MaxTransfersPerUser-workspaceV2MaxTransfersPerWorkspace; index++ {
		connection := connections[index%len(connections)]
		transfer := workspaceV2TestTransfer(connection, workspaceB, time.Now())
		require.NoError(t, manager.reserve(connection, transfer))
	}
	require.Error(t, manager.reserve(connections[0], workspaceV2TestTransfer(connections[0], workspaceB, time.Now())))
}

func TestWorkspaceV2TransferExpiryReleasesReservation(t *testing.T) {
	manager := newWorkspaceV2TransferManager()
	connection := &workspaceV2Connection{uid: 41, transfers: make(map[uuid.UUID]*workspaceV2Transfer), seenTransfers: make(map[uuid.UUID]struct{})}
	created := time.Date(2026, 8, 7, 0, 0, 0, 0, time.UTC)
	transfer := workspaceV2TestTransfer(connection, dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), created)
	require.NoError(t, manager.reserve(connection, transfer))
	manager.Expire(created.Add(workspaceV2TransferIdleExpiry))
	require.Empty(t, connection.transfers)
	manager.mu.Lock()
	require.Empty(t, manager.active)
	require.Empty(t, manager.byWorkspace)
	require.Empty(t, manager.byUser)
	require.Empty(t, manager.byIdentity)
	manager.mu.Unlock()
}

func TestWorkspaceV2CleanupCancelsBlockedFinalUploadChunkWithoutEarlyRelease(t *testing.T) {
	store := &workspaceV2BlockingUploadStore{started: make(chan struct{}), release: make(chan struct{})}
	serverCtx, serverCancel := context.WithCancel(context.Background())
	defer serverCancel()
	server := &WorkspaceV2Server{ctx: serverCtx, transfers: newWorkspaceV2TransferManager(), blobStore: store}
	connectionCtx, connectionCancel := context.WithCancel(serverCtx)
	defer connectionCancel()
	connection := &workspaceV2Connection{
		server:        server,
		ctx:           connectionCtx,
		cancel:        connectionCancel,
		uid:           41,
		transfers:     make(map[uuid.UUID]*workspaceV2Transfer),
		seenTransfers: make(map[uuid.UUID]struct{}),
	}
	payload := []byte("final")
	transferID := uuid.MustParse("60000000-0000-4000-8000-000000000200")
	hash := workspaceV2TestBlobHash(payload)
	reader, writer := io.Pipe()
	transferCtx, transferCancel := context.WithCancel(connectionCtx)
	defer transferCancel()
	transfer := &workspaceV2Transfer{
		ctx: transferCtx, cancel: transferCancel,
		workspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), transferID: transferID,
		direction: dto.WorkspaceBlobUpload, contentHash: hash, size: uint64(len(payload)), chunkCount: 1,
		uploadWriter: writer, uploadDone: make(chan struct{}),
	}
	require.NoError(t, server.transfers.reserve(connection, transfer))
	_, chunkDigest := dto.ComputeWorkspaceBlobDigest(payload)
	header, err := dto.MarshalWorkspaceBlobHeader(dto.WorkspaceBlobHeader{
		Direction: dto.WorkspaceBlobUpload, Final: true, TransferID: transferID,
		PayloadLen: uint32(len(payload)), ChunkDigest: chunkDigest,
	})
	require.NoError(t, err)
	chunkDone := make(chan error, 1)
	go func() { chunkDone <- connection.handleWorkspaceV2BinaryFrame(append(header[:], payload...)) }()
	for range 100 {
		runtime.Gosched()
	}
	go connection.runWorkspaceV2Upload(transfer, reader)
	select {
	case <-store.started:
	case <-time.After(time.Second):
		t.Fatal("upload store did not start")
	}

	cleanupDone := make(chan bool, 1)
	go func() { cleanupDone <- connection.removeWorkspaceV2Transfer(transferID) }()
	select {
	case <-cleanupDone:
	case <-time.After(time.Second):
		t.Fatal("cleanup initiation blocked behind the final upload chunk")
	}
	select {
	case err := <-chunkDone:
		require.Error(t, err)
	case <-time.After(time.Second):
		t.Fatal("cleanup did not cancel the blocked final upload chunk")
	}
	workspaceV2RequireTransferReleased(t, server, connection, transfer.workspaceID, transferID)
	close(store.release)
}

func TestWorkspaceV2BlobEndBeforeUploadTerminatesTransferAndRejectsReuse(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.blobStore = &workspaceV2BlobStoreStub{}
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000201")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000202", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}

	payload := []byte("hello")
	transferID := dto.WorkspaceUUID("60000000-0000-4000-8000-000000000201")
	hash := workspaceV2TestBlobHash(payload)
	begin := dto.WorkspaceBlobBeginMessage{
		WorkspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), TransferID: transferID,
		Direction: dto.WorkspaceBlobUpload, ContentHash: hash, Size: uint64(len(payload)),
		ChunkSize: dto.WorkspaceBlobChunkSize, ChunkCount: 1,
	}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000203", begin)
	require.Equal(t, string(dto.WorkspaceActionBlobBegin), workspaceV2Action(workspaceV2Receive(t, events)))

	end := dto.WorkspaceBlobEndMessage{
		WorkspaceID: begin.WorkspaceID, TransferID: begin.TransferID, Direction: begin.Direction,
		ContentHash: begin.ContentHash, Size: begin.Size, ChunkCount: begin.ChunkCount,
	}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000204", end)
	failure := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionBlobEnd), workspaceV2Action(failure))
	require.False(t, workspaceV2ResponseStatus(t, failure))
	require.Equal(t, dto.WorkspaceErrorBlobTransferOutOfOrder, workspaceV2ErrorCode(t, failure))

	owner := workspaceV2FindConnection(t, server, dto.WorkspaceUUID(workspaceV2StreamClientID))
	workspaceV2RequireTransferReleased(t, server, owner, begin.WorkspaceID, uuid.MustParse(string(transferID)))

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000205", begin)
	reuseFailure := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionBlobBegin), workspaceV2Action(reuseFailure))
	require.False(t, workspaceV2ResponseStatus(t, reuseFailure))
	require.Equal(t, dto.WorkspaceErrorBlobTransferOutOfOrder, workspaceV2ErrorCode(t, reuseFailure))
	workspaceV2RequireTransferReleased(t, server, owner, begin.WorkspaceID, uuid.MustParse(string(transferID)))

	_, chunkDigest := dto.ComputeWorkspaceBlobDigest(payload)
	header, err := dto.MarshalWorkspaceBlobHeader(dto.WorkspaceBlobHeader{
		Direction: dto.WorkspaceBlobUpload, Final: true, TransferID: uuid.MustParse(string(transferID)),
		PayloadLen: uint32(len(payload)), ChunkDigest: chunkDigest,
	})
	require.NoError(t, err)
	require.NoError(t, conn.WriteMessage(gws.OpcodeBinary, append(header[:], payload...)))
	select {
	case <-events.closes:
	case <-time.After(time.Second):
		t.Fatal("binary chunk reused a failed upload transfer")
	}
}

func TestWorkspaceV2UploadIntegrityFailureReleasesTransferAndRejectsReuse(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.blobStore = &workspaceV2UploadErrorStore{putErr: errors.New("workspace blob hash mismatch")}
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000211")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000212", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}

	payload := []byte("hello")
	transferID := dto.WorkspaceUUID("60000000-0000-4000-8000-000000000211")
	hash := workspaceV2TestBlobHash(payload)
	begin := dto.WorkspaceBlobBeginMessage{
		WorkspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), TransferID: transferID,
		Direction: dto.WorkspaceBlobUpload, ContentHash: hash, Size: uint64(len(payload)),
		ChunkSize: dto.WorkspaceBlobChunkSize, ChunkCount: 1,
	}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000213", begin)
	require.Equal(t, string(dto.WorkspaceActionBlobBegin), workspaceV2Action(workspaceV2Receive(t, events)))
	_, chunkDigest := dto.ComputeWorkspaceBlobDigest(payload)
	header, err := dto.MarshalWorkspaceBlobHeader(dto.WorkspaceBlobHeader{
		Direction: dto.WorkspaceBlobUpload, Final: true, TransferID: uuid.MustParse(string(transferID)),
		PayloadLen: uint32(len(payload)), ChunkDigest: chunkDigest,
	})
	require.NoError(t, err)
	require.NoError(t, conn.WriteMessage(gws.OpcodeBinary, append(header[:], payload...)))

	end := dto.WorkspaceBlobEndMessage{
		WorkspaceID: begin.WorkspaceID, TransferID: begin.TransferID, Direction: begin.Direction,
		ContentHash: begin.ContentHash, Size: begin.Size, ChunkCount: begin.ChunkCount,
	}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000214", end)
	failure := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionBlobEnd), workspaceV2Action(failure))
	require.False(t, workspaceV2ResponseStatus(t, failure))
	require.Equal(t, dto.WorkspaceErrorBlobHashMismatch, workspaceV2ErrorCode(t, failure))

	owner := workspaceV2FindConnection(t, server, dto.WorkspaceUUID(workspaceV2StreamClientID))
	workspaceV2RequireTransferReleased(t, server, owner, begin.WorkspaceID, uuid.MustParse(string(transferID)))
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000215", begin)
	reuseFailure := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionBlobBegin), workspaceV2Action(reuseFailure))
	require.False(t, workspaceV2ResponseStatus(t, reuseFailure))
	require.Equal(t, dto.WorkspaceErrorBlobTransferOutOfOrder, workspaceV2ErrorCode(t, reuseFailure))
	workspaceV2RequireTransferReleased(t, server, owner, begin.WorkspaceID, uuid.MustParse(string(transferID)))
}

func TestWorkspaceV2BlobBeginDuplicateExactRetryIsIdempotent(t *testing.T) {
	store := &workspaceV2CountingUploadStore{started: make(chan struct{})}
	server, conn, events, owner := workspaceV2PrepareBlobSession(t, store)
	payload := []byte("hello")
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000220", payload)
	transferID := uuid.MustParse(string(begin.TransferID))

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000220", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000220", begin)
	select {
	case <-store.started:
	case <-time.After(time.Second):
		t.Fatal("upload store did not start")
	}
	active := owner.workspaceV2Transfer(transferID)
	require.NotNil(t, active)
	workspaceV2RequireExactTransferSlots(t, server, owner, active)

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000221", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000221", begin)
	require.Same(t, active, owner.workspaceV2Transfer(transferID))
	require.Equal(t, 1, store.callCount())
	workspaceV2RequireExactTransferSlots(t, server, owner, active)
}

func TestWorkspaceV2ActiveTransferIDChangedTupleRejectedAcrossLiveConnections(t *testing.T) {
	store := &workspaceV2CountingUploadStore{started: make(chan struct{})}
	server, connect := workspaceV2PrepareMultiConnectionBlobServer(t, store)
	connA, eventsA, ownerA := connect(
		"30000000-0000-4000-8000-000000000220",
		"10000000-0000-4000-8000-000000000260",
		"10000000-0000-4000-8000-000000000261",
	)
	connB, eventsB, ownerB := connect(
		"30000000-0000-4000-8000-000000000221",
		"10000000-0000-4000-8000-000000000262",
		"10000000-0000-4000-8000-000000000263",
	)
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000260", []byte("first live upload"))
	transferID := uuid.MustParse(string(begin.TransferID))
	changed := begin
	changed.ContentHash = workspaceV2TestBlobHash([]byte("changed live upload"))
	changed.Size = uint64(len("changed live upload"))
	frameA := workspaceV2RequestFrame(t, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000264", begin)
	frameB := workspaceV2RequestFrame(t, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000265", changed)
	start := make(chan struct{})
	writeErrors := make(chan error, 2)
	go func() {
		<-start
		writeErrors <- connA.WriteString(frameA)
	}()
	go func() {
		<-start
		writeErrors <- connB.WriteString(frameB)
	}()
	close(start)
	require.NoError(t, <-writeErrors)
	require.NoError(t, <-writeErrors)
	responseA := workspaceV2Receive(t, eventsA)
	responseB := workspaceV2Receive(t, eventsB)
	require.NotEqual(t, workspaceV2ResponseStatus(t, responseA), workspaceV2ResponseStatus(t, responseB))

	var active *workspaceV2Transfer
	var activeOwner *workspaceV2Connection
	if workspaceV2ResponseStatus(t, responseA) {
		workspaceV2RequireBlobBeginSuccess(t, responseA, "10000000-0000-4000-8000-000000000264", begin)
		workspaceV2RequireFailureRequest(t, responseB, dto.WorkspaceActionBlobBegin,
			"10000000-0000-4000-8000-000000000265", dto.WorkspaceErrorBlobTransferOutOfOrder)
		activeOwner = ownerA
	} else {
		workspaceV2RequireFailureRequest(t, responseA, dto.WorkspaceActionBlobBegin,
			"10000000-0000-4000-8000-000000000264", dto.WorkspaceErrorBlobTransferOutOfOrder)
		workspaceV2RequireBlobBeginSuccess(t, responseB, "10000000-0000-4000-8000-000000000265", changed)
		activeOwner = ownerB
	}
	select {
	case <-store.started:
	case <-time.After(time.Second):
		t.Fatal("winning live upload did not reach the blob store")
	}
	active = activeOwner.workspaceV2Transfer(transferID)
	require.NotNil(t, active)
	require.Equal(t, 1, store.callCount())
	if activeOwner == ownerA {
		require.Nil(t, ownerB.workspaceV2Transfer(transferID))
	} else {
		require.Nil(t, ownerA.workspaceV2Transfer(transferID))
	}
	workspaceV2RequireExactTransferSlots(t, server, activeOwner, active)
}

func TestWorkspaceV2ActiveTransferIDExactTupleRejectsNonOwningLiveConnection(t *testing.T) {
	store := &workspaceV2CountingUploadStore{started: make(chan struct{})}
	server, connect := workspaceV2PrepareMultiConnectionBlobServer(t, store)
	connA, eventsA, ownerA := connect(
		"30000000-0000-4000-8000-000000000223",
		"10000000-0000-4000-8000-000000000266",
		"10000000-0000-4000-8000-000000000267",
	)
	connB, eventsB, ownerB := connect(
		"30000000-0000-4000-8000-000000000224",
		"10000000-0000-4000-8000-000000000268",
		"10000000-0000-4000-8000-000000000269",
	)
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000261", []byte("same live upload"))
	transferID := uuid.MustParse(string(begin.TransferID))
	frameA := workspaceV2RequestFrame(t, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000270", begin)
	frameB := workspaceV2RequestFrame(t, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000271", begin)
	start := make(chan struct{})
	writeErrors := make(chan error, 2)
	go func() {
		<-start
		writeErrors <- connA.WriteString(frameA)
	}()
	go func() {
		<-start
		writeErrors <- connB.WriteString(frameB)
	}()
	close(start)
	require.NoError(t, <-writeErrors)
	require.NoError(t, <-writeErrors)
	responseA := workspaceV2Receive(t, eventsA)
	responseB := workspaceV2Receive(t, eventsB)
	select {
	case <-store.started:
	case <-time.After(time.Second):
		t.Fatal("winning exact upload did not reach the blob store")
	}
	activeA := ownerA.workspaceV2Transfer(transferID)
	activeB := ownerB.workspaceV2Transfer(transferID)
	require.NotEqual(t, activeA != nil, activeB != nil)
	activeOwner, active := ownerA, activeA
	winnerConn, winnerResponse := connA, responseA
	winnerRequestID := "10000000-0000-4000-8000-000000000270"
	loserConn, loserEvents, loserResponse := connB, eventsB, responseB
	loserRequestID := "10000000-0000-4000-8000-000000000271"
	if active == nil {
		activeOwner, active = ownerB, activeB
		winnerConn, winnerResponse = connB, responseB
		winnerRequestID = "10000000-0000-4000-8000-000000000271"
		loserConn, loserEvents, loserResponse = connA, eventsA, responseA
		loserRequestID = "10000000-0000-4000-8000-000000000270"
	}
	workspaceV2RequireBlobBeginSuccess(t, winnerResponse, winnerRequestID, begin)
	workspaceV2RequireFailureRequest(t, loserResponse, dto.WorkspaceActionBlobBegin, loserRequestID, dto.WorkspaceErrorBlobTransferOutOfOrder)
	workspaceV2RequireExactTransferSlots(t, server, activeOwner, active)
	workspaceV2SendUploadChunk(t, loserConn, transferID, []byte("same live upload"))
	select {
	case received := <-loserEvents.closes:
		var closeErr *gws.CloseError
		require.True(t, errors.As(received, &closeErr), "loser close error: %v", received)
		require.Equal(t, uint16(1002), closeErr.Code)
		require.Equal(t, "invalid_binary", string(closeErr.Reason))
	case <-time.After(time.Second):
		t.Fatal("non-owning connection binary chunk was not rejected")
	}
	require.Same(t, active, activeOwner.workspaceV2Transfer(transferID))
	workspaceV2RequireExactTransferSlots(t, server, activeOwner, active)
	require.Equal(t, 1, store.callCount())

	payload := []byte("same live upload")
	workspaceV2SendUploadChunk(t, winnerConn, transferID, payload)
	end := workspaceV2EndForBegin(begin)
	workspaceV2Send(t, winnerConn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000280", end)
	var winnerEvents *workspaceV2TestEvents
	if winnerConn == connA {
		winnerEvents = eventsA
	} else {
		winnerEvents = eventsB
	}
	workspaceV2RequireBlobEndSuccess(t, workspaceV2Receive(t, winnerEvents), "10000000-0000-4000-8000-000000000280", end)
	workspaceV2RequireExactTransferSlots(t, server, activeOwner, nil)
	workspaceV2RequireCompletedReceipt(t, activeOwner, end)
	require.Equal(t, 1, store.callCount())
}

func TestWorkspaceV2BlobBeginDuplicateMismatchPreservesTransfer(t *testing.T) {
	store := &workspaceV2CountingUploadStore{started: make(chan struct{})}
	server, conn, events, owner := workspaceV2PrepareBlobSession(t, store)
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000222", []byte("hello"))
	transferID := uuid.MustParse(string(begin.TransferID))

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000222", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000222", begin)
	select {
	case <-store.started:
	case <-time.After(time.Second):
		t.Fatal("upload store did not start")
	}
	active := owner.workspaceV2Transfer(transferID)
	require.NotNil(t, active)
	mismatch := begin
	mismatch.ContentHash = workspaceV2TestBlobHash([]byte("other"))

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000223", mismatch)
	failure := workspaceV2Receive(t, events)
	workspaceV2RequireFailureRequest(t, failure, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000223", dto.WorkspaceErrorBlobTransferOutOfOrder)
	require.Same(t, active, owner.workspaceV2Transfer(transferID))
	require.Equal(t, 1, store.callCount())
	workspaceV2RequireExactTransferSlots(t, server, owner, active)
}

func TestWorkspaceV2BlobMismatchedEndPreservesTransfer(t *testing.T) {
	store := &workspaceV2CountingUploadStore{started: make(chan struct{})}
	server, conn, events, owner := workspaceV2PrepareBlobSession(t, store)
	payload := []byte("hello")
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000224", payload)
	transferID := uuid.MustParse(string(begin.TransferID))

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000224", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000224", begin)
	active := owner.workspaceV2Transfer(transferID)
	require.NotNil(t, active)
	mismatch := workspaceV2EndForBegin(begin)
	mismatch.ContentHash = workspaceV2TestBlobHash([]byte("other"))

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000225", mismatch)
	failure := workspaceV2Receive(t, events)
	workspaceV2RequireFailureRequest(t, failure, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000225", dto.WorkspaceErrorBlobTransferOutOfOrder)
	require.Same(t, active, owner.workspaceV2Transfer(transferID))
	workspaceV2RequireExactTransferSlots(t, server, owner, active)

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000226", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000226", begin)
	require.Equal(t, 1, store.callCount())
	workspaceV2SendUploadChunk(t, conn, transferID, payload)
	end := workspaceV2EndForBegin(begin)
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000227", end)
	workspaceV2RequireBlobEndSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000227", end)
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)
}

func TestWorkspaceV2BlobEndInvalidSequenceCancelsTransferStore(t *testing.T) {
	store := &workspaceV2ContextOnlyUploadStore{started: make(chan struct{}), canceled: make(chan struct{})}
	server, conn, events, owner := workspaceV2PrepareBlobSession(t, store)
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000236", []byte("cancel"))
	transferID := uuid.MustParse(string(begin.TransferID))
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000236", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000236", begin)
	select {
	case <-store.started:
	case <-time.After(time.Second):
		t.Fatal("upload store did not start")
	}
	active := owner.workspaceV2Transfer(transferID)
	require.NotNil(t, active)
	workspaceV2RequireExactTransferSlots(t, server, owner, active)

	end := workspaceV2EndForBegin(begin)
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000237", end)
	workspaceV2RequireFailureRequest(t, workspaceV2Receive(t, events), dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000237", dto.WorkspaceErrorBlobTransferOutOfOrder)
	select {
	case <-store.canceled:
	case <-time.After(time.Second):
		t.Fatal("invalid upload teardown did not cancel the store operation")
	}
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)
}

func TestWorkspaceV2UploadBlobEndAckAwaitsDurableCompletionAndDuplicateEndIsIdempotent(t *testing.T) {
	store := &workspaceV2DurableUploadStore{awaitingCommit: make(chan struct{}), releaseCommit: make(chan struct{})}
	server, conn, events, owner := workspaceV2PrepareBlobSession(t, store)
	payload := []byte("durable")
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000227", payload)
	end := workspaceV2EndForBegin(begin)
	transferID := uuid.MustParse(string(begin.TransferID))

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000227", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000227", begin)
	workspaceV2SendUploadChunk(t, conn, transferID, payload)
	select {
	case <-store.awaitingCommit:
	case <-time.After(time.Second):
		t.Fatal("upload store did not reach durable completion gate")
	}
	active := owner.workspaceV2Transfer(transferID)
	require.NotNil(t, active)

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000228", end)
	select {
	case response := <-events.messages:
		t.Fatalf("BlobEnd responded before durable completion: %s", response)
	case <-time.After(50 * time.Millisecond):
	}
	workspaceV2RequireExactTransferSlots(t, server, owner, active)
	written := make(chan []byte, 1)
	releaseWriter := make(chan struct{})
	owner.writeMessage = func(opcode gws.Opcode, data []byte) error {
		if opcode == gws.OpcodeText && workspaceV2Action(data) == string(dto.WorkspaceActionBlobEnd) {
			written <- append([]byte(nil), data...)
			<-releaseWriter
		}
		return owner.conn.WriteMessage(opcode, data)
	}
	close(store.releaseCommit)
	select {
	case response := <-written:
		workspaceV2RequireBlobEndSuccess(t, response, "10000000-0000-4000-8000-000000000228", end)
	case <-time.After(time.Second):
		t.Fatal("durable BlobEnd response writer was not reached")
	}
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)
	workspaceV2RequireCompletedReceipt(t, owner, end)
	close(releaseWriter)
	workspaceV2RequireBlobEndSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000228", end)
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)
	workspaceV2RequireCompletedReceipt(t, owner, end)

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000229", end)
	workspaceV2RequireBlobEndSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000229", end)
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)
}

func TestWorkspaceV2UploadBlobEndResponseLossReplaysAfterReconnect(t *testing.T) {
	store := &workspaceV2CountingUploadStore{started: make(chan struct{})}
	server, connect := workspaceV2PrepareReconnectableBlobServer(t, store)
	conn, events, owner := connect(
		"10000000-0000-4000-8000-000000000238",
		"10000000-0000-4000-8000-000000000239",
	)
	payload := []byte("upload response loss")
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000238", payload)
	end := workspaceV2EndForBegin(begin)
	transferID := uuid.MustParse(string(begin.TransferID))

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000240", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000240", begin)
	workspaceV2SendUploadChunk(t, conn, transferID, payload)
	written, releaseWriter := workspaceV2LoseBlobEndResponse(owner)
	requestID := "10000000-0000-4000-8000-000000000241"
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, requestID, end)
	select {
	case response := <-written:
		workspaceV2RequireBlobEndSuccess(t, response, requestID, end)
	case <-time.After(time.Second):
		t.Fatal("upload BlobEnd response writer was not reached")
	}
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)
	workspaceV2RequireCompletedReceipt(t, owner, end)
	require.NoError(t, conn.NetConn().Close())
	close(releaseWriter)
	workspaceV2RequireConnectionRemoved(t, server, owner)

	replayConn, replayEvents, replayOwner := connect(
		"10000000-0000-4000-8000-000000000242",
		"",
	)
	unauthorizedRequestID := "10000000-0000-4000-8000-000000000243"
	workspaceV2Send(t, replayConn, dto.WorkspaceActionBlobEnd, unauthorizedRequestID, end)
	workspaceV2RequireFailureRequest(t, workspaceV2Receive(t, replayEvents), dto.WorkspaceActionBlobEnd, unauthorizedRequestID, dto.WorkspaceErrorInvalidRequest)
	workspaceV2Send(t, replayConn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000244", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, replayEvents)
	}

	reusedBegin := workspaceV2UploadBegin(string(begin.TransferID), []byte("changed upload"))
	workspaceV2Send(t, replayConn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000245", reusedBegin)
	workspaceV2RequireFailureRequest(t, workspaceV2Receive(t, replayEvents), dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000245", dto.WorkspaceErrorBlobTransferOutOfOrder)
	require.Equal(t, 1, store.callCount())

	replayRequestID := "10000000-0000-4000-8000-000000000254"
	workspaceV2Send(t, replayConn, dto.WorkspaceActionBlobEnd, replayRequestID, end)
	workspaceV2RequireBlobEndSuccess(t, workspaceV2Receive(t, replayEvents), replayRequestID, end)
	workspaceV2RequireExactTransferSlots(t, server, replayOwner, nil)
	require.Equal(t, 1, store.callCount())

	mismatch := end
	mismatch.ContentHash = workspaceV2TestBlobHash([]byte("changed upload"))
	mismatchRequestID := "10000000-0000-4000-8000-000000000255"
	workspaceV2Send(t, replayConn, dto.WorkspaceActionBlobEnd, mismatchRequestID, mismatch)
	workspaceV2RequireFailureRequest(t, workspaceV2Receive(t, replayEvents), dto.WorkspaceActionBlobEnd, mismatchRequestID, dto.WorkspaceErrorBlobTransferOutOfOrder)
	require.Equal(t, 1, store.callCount())
}

func TestWorkspaceV2UploadCommitReceiptSurvivesConcurrentCleanup(t *testing.T) {
	store := &workspaceV2CleanupWinningUploadStore{
		committed:     make(chan struct{}),
		returnSuccess: make(chan struct{}),
	}
	server, connect := workspaceV2PrepareMultiConnectionBlobServer(t, store)
	conn, events, owner := connect(
		"30000000-0000-4000-8000-000000000225",
		"10000000-0000-4000-8000-000000000272",
		"10000000-0000-4000-8000-000000000273",
	)
	payload := []byte("cleanup after durable commit")
	begin := workspaceV2UploadBegin("60000000-0000-4000-8000-000000000262", payload)
	end := workspaceV2EndForBegin(begin)
	transferID := uuid.MustParse(string(begin.TransferID))

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000274", begin)
	workspaceV2RequireBlobBeginSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000274", begin)
	workspaceV2SendUploadChunk(t, conn, transferID, payload)
	select {
	case <-store.committed:
	case <-time.After(time.Second):
		t.Fatal("upload did not reach the durable commit gate")
	}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000275", end)

	cleanupDone := make(chan struct{})
	go func() {
		server.CloseAllConnections()
		close(cleanupDone)
	}()
	select {
	case <-owner.ctx.Done():
	case <-time.After(time.Second):
		t.Fatal("connection cleanup did not cancel the upload context")
	}
	close(store.returnSuccess)
	select {
	case <-cleanupDone:
	case <-time.After(time.Second):
		t.Fatal("connection cleanup did not finish after Put returned success")
	}
	workspaceV2RequireConnectionRemoved(t, server, owner)
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)

	replayConn, replayEvents, replayOwner := connect(
		"30000000-0000-4000-8000-000000000226",
		"10000000-0000-4000-8000-000000000276",
		"10000000-0000-4000-8000-000000000277",
	)
	workspaceV2Send(t, replayConn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000278", end)
	workspaceV2RequireBlobEndSuccess(t, workspaceV2Receive(t, replayEvents), "10000000-0000-4000-8000-000000000278", end)
	workspaceV2RequireExactTransferSlots(t, server, replayOwner, nil)

	changed := end
	changed.ContentHash = workspaceV2TestBlobHash([]byte("changed after cleanup"))
	workspaceV2Send(t, replayConn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000279", changed)
	workspaceV2RequireFailureRequest(t, workspaceV2Receive(t, replayEvents), dto.WorkspaceActionBlobEnd,
		"10000000-0000-4000-8000-000000000279", dto.WorkspaceErrorBlobTransferOutOfOrder)
	require.Equal(t, 1, store.callCount())
}

func TestWorkspaceV2DownloadBlobEndAckDuplicateEndAndMismatchedReceipt(t *testing.T) {
	payload := []byte("download")
	server, conn, events, owner := workspaceV2PrepareBlobSession(t, &workspaceV2BlobStoreStub{download: payload})
	end, active := workspaceV2StartDownload(t, conn, events, owner, payload, "10000000-0000-4000-8000-000000000230")
	workspaceV2RequireExactTransferSlots(t, server, owner, active)

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000231", end)
	workspaceV2RequireBlobEndSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000231", end)
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)
	workspaceV2RequireCompletedReceipt(t, owner, end)

	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000232", end)
	workspaceV2RequireBlobEndSuccess(t, workspaceV2Receive(t, events), "10000000-0000-4000-8000-000000000232", end)
	mismatch := end
	mismatch.ContentHash = workspaceV2TestBlobHash([]byte("mismatch"))
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000233", mismatch)
	workspaceV2RequireFailureRequest(t, workspaceV2Receive(t, events), dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000233", dto.WorkspaceErrorBlobTransferOutOfOrder)
	workspaceV2RequireCompletedReceipt(t, owner, end)
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)
}

func TestWorkspaceV2DownloadBlobEndAckSlotReleasePrecedesResponseLoss(t *testing.T) {
	payload := []byte("response loss")
	server, conn, events, owner := workspaceV2PrepareBlobSession(t, &workspaceV2BlobStoreStub{download: payload})
	end, active := workspaceV2StartDownload(t, conn, events, owner, payload, "10000000-0000-4000-8000-000000000234")
	workspaceV2RequireExactTransferSlots(t, server, owner, active)

	written := make(chan []byte, 1)
	releaseWriter := make(chan struct{})
	owner.writeMessage = func(opcode gws.Opcode, data []byte) error {
		if opcode == gws.OpcodeText && workspaceV2Action(data) == string(dto.WorkspaceActionBlobEnd) {
			written <- append([]byte(nil), data...)
			<-releaseWriter
			return errors.New("simulated BlobEnd response loss")
		}
		return owner.conn.WriteMessage(opcode, data)
	}
	requestID := "10000000-0000-4000-8000-000000000235"
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, requestID, end)
	var lostResponse []byte
	select {
	case lostResponse = <-written:
	case <-time.After(time.Second):
		t.Fatal("BlobEnd response writer was not reached")
	}
	workspaceV2RequireBlobEndSuccess(t, lostResponse, requestID, end)
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)
	workspaceV2RequireCompletedReceipt(t, owner, end)
	close(releaseWriter)
	require.Eventually(t, func() bool {
		server.mu.RLock()
		_, present := server.connections[owner.conn]
		server.mu.RUnlock()
		return !present
	}, time.Second, 10*time.Millisecond)
	workspaceV2RequireCompletedReceipt(t, owner, end)
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)
}

func TestWorkspaceV2DownloadBlobEndResponseLossReplaysAfterReconnect(t *testing.T) {
	payload := []byte("download response loss replay")
	store := &workspaceV2BlobStoreStub{download: payload}
	server, connect := workspaceV2PrepareReconnectableBlobServer(t, store)
	conn, events, owner := connect(
		"10000000-0000-4000-8000-000000000246",
		"10000000-0000-4000-8000-000000000247",
	)
	end, active := workspaceV2StartDownload(t, conn, events, owner, payload, "10000000-0000-4000-8000-000000000248")
	workspaceV2RequireExactTransferSlots(t, server, owner, active)

	written, releaseWriter := workspaceV2LoseBlobEndResponse(owner)
	requestID := "10000000-0000-4000-8000-000000000249"
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, requestID, end)
	select {
	case response := <-written:
		workspaceV2RequireBlobEndSuccess(t, response, requestID, end)
	case <-time.After(time.Second):
		t.Fatal("download BlobEnd response writer was not reached")
	}
	workspaceV2RequireExactTransferSlots(t, server, owner, nil)
	workspaceV2RequireCompletedReceipt(t, owner, end)
	require.NoError(t, conn.NetConn().Close())
	close(releaseWriter)
	workspaceV2RequireConnectionRemoved(t, server, owner)

	replayConn, replayEvents, replayOwner := connect(
		"10000000-0000-4000-8000-000000000250",
		"10000000-0000-4000-8000-000000000251",
	)
	replayRequestID := "10000000-0000-4000-8000-000000000252"
	workspaceV2Send(t, replayConn, dto.WorkspaceActionBlobEnd, replayRequestID, end)
	workspaceV2RequireBlobEndSuccess(t, workspaceV2Receive(t, replayEvents), replayRequestID, end)
	workspaceV2RequireExactTransferSlots(t, server, replayOwner, nil)

	mismatch := end
	mismatch.ContentHash = workspaceV2TestBlobHash([]byte("changed download"))
	mismatchRequestID := "10000000-0000-4000-8000-000000000253"
	workspaceV2Send(t, replayConn, dto.WorkspaceActionBlobEnd, mismatchRequestID, mismatch)
	workspaceV2RequireFailureRequest(t, workspaceV2Receive(t, replayEvents), dto.WorkspaceActionBlobEnd, mismatchRequestID, dto.WorkspaceErrorBlobTransferOutOfOrder)
}

func TestWorkspaceV2CompletedTransferReceiptSurvivesConnectionCleanup(t *testing.T) {
	server := &WorkspaceV2Server{}
	connection := &workspaceV2Connection{
		server:        server,
		uid:           41,
		transfers:     make(map[uuid.UUID]*workspaceV2Transfer),
		seenTransfers: make(map[uuid.UUID]struct{}),
	}
	end := dto.WorkspaceBlobEndMessage{
		WorkspaceID: "10000000-0000-4000-8000-000000000001",
		TransferID:  dto.WorkspaceUUID(uuid.NewString()),
		Direction:   dto.WorkspaceBlobUpload,
		ContentHash: workspaceV2TestBlobHash(nil),
		Size:        0,
		ChunkCount:  0,
	}
	require.True(t, connection.recordWorkspaceV2CompletedTransfer(end))
	workspaceV2RequireCompletedReceipt(t, connection, end)

	connection.cleanupTransfers()
	workspaceV2RequireCompletedReceipt(t, connection, end)
}

func TestWorkspaceV2CompletedTransferRegistryIsGloballyBoundedAndExpires(t *testing.T) {
	registry := &workspaceV2CompletedTransferRegistry{}
	now := time.Date(2026, 8, 9, 0, 0, 0, 0, time.UTC)
	var firstKey workspaceV2TransferKey
	var newestKey workspaceV2TransferKey
	var newest dto.WorkspaceBlobEndMessage
	for index := range workspaceV2MaxCompletedTransferReceipts + 1 {
		uid := int64(41 + index%2)
		transferID := uuid.New()
		end := dto.WorkspaceBlobEndMessage{
			WorkspaceID: "10000000-0000-4000-8000-000000000001",
			TransferID:  dto.WorkspaceUUID(transferID.String()),
			Direction:   dto.WorkspaceBlobUpload,
			ContentHash: workspaceV2TestBlobHash(nil),
			Size:        0,
			ChunkCount:  0,
		}
		key := workspaceV2TransferKey{uid: uid, transferID: transferID}
		if index == 0 {
			firstKey = key
		}
		newestKey = key
		newest = end
		require.True(t, registry.record(uid, end, now.Add(time.Duration(index))))
	}

	registry.mu.Lock()
	require.Len(t, registry.receipts, workspaceV2MaxCompletedTransferReceipts)
	require.Len(t, registry.order, workspaceV2MaxCompletedTransferReceipts)
	_, oldestPresent := registry.receipts[firstKey]
	registry.mu.Unlock()
	require.False(t, oldestPresent)
	receipt, present := registry.completed(newestKey.uid, newestKey.transferID, now.Add(time.Second))
	require.True(t, present)
	require.True(t, receipt.matches(newest))
	_, wrongUserPresent := registry.completed(newestKey.uid+100, newestKey.transferID, now.Add(time.Second))
	require.False(t, wrongUserPresent)

	registry.Expire(now.Add(workspaceV2CompletedTransferReceiptTTL + time.Second))
	registry.mu.Lock()
	require.Empty(t, registry.receipts)
	require.Empty(t, registry.order)
	registry.mu.Unlock()
}

func TestWorkspaceV2CompletedTransferRegistryRejectsEveryTupleMismatchWithoutMutation(t *testing.T) {
	registry := &workspaceV2CompletedTransferRegistry{}
	now := time.Date(2026, 8, 9, 0, 0, 0, 0, time.UTC)
	transferID := uuid.New()
	end := dto.WorkspaceBlobEndMessage{
		WorkspaceID: "10000000-0000-4000-8000-000000000001",
		TransferID:  dto.WorkspaceUUID(transferID.String()),
		Direction:   dto.WorkspaceBlobUpload,
		ContentHash: workspaceV2TestBlobHash([]byte("complete tuple")),
		Size:        14,
		ChunkCount:  1,
	}
	require.True(t, registry.record(41, end, now))

	mismatches := []dto.WorkspaceBlobEndMessage{end, end, end, end, end}
	mismatches[0].WorkspaceID = "10000000-0000-4000-8000-000000000002"
	mismatches[1].Direction = dto.WorkspaceBlobDownload
	mismatches[2].ContentHash = workspaceV2TestBlobHash([]byte("changed tuple"))
	mismatches[3].Size++
	mismatches[4].ChunkCount++
	for _, mismatch := range mismatches {
		require.False(t, registry.record(41, mismatch, now.Add(time.Second)))
		receipt, present := registry.completed(41, transferID, now.Add(time.Second))
		require.True(t, present)
		require.True(t, receipt.matches(end))
		require.False(t, receipt.matches(mismatch))
	}
}

func TestWorkspaceV2CompletedTransferRegistryIsConcurrencySafe(t *testing.T) {
	registry := &workspaceV2CompletedTransferRegistry{}
	now := time.Date(2026, 8, 9, 0, 0, 0, 0, time.UTC)
	var wg sync.WaitGroup
	for worker := range 16 {
		wg.Add(1)
		go func(worker int) {
			defer wg.Done()
			for index := range workspaceV2MaxCompletedTransferReceipts / 4 {
				transferID := uuid.New()
				end := dto.WorkspaceBlobEndMessage{
					WorkspaceID: "10000000-0000-4000-8000-000000000001",
					TransferID:  dto.WorkspaceUUID(transferID.String()),
					Direction:   dto.WorkspaceBlobUpload,
					ContentHash: workspaceV2TestBlobHash(nil),
					Size:        0,
					ChunkCount:  0,
				}
				if !registry.record(int64(41+worker%2), end, now) {
					t.Errorf("record completed transfer for worker %d", worker)
					return
				}
				registry.completed(int64(41+worker%2), transferID, now)
				if index%64 == 0 {
					registry.Expire(now.Add(-time.Second))
				}
			}
		}(worker)
	}
	wg.Wait()
	registry.mu.Lock()
	require.LessOrEqual(t, len(registry.receipts), workspaceV2MaxCompletedTransferReceipts)
	require.LessOrEqual(t, len(registry.order), workspaceV2MaxCompletedTransferReceipts)
	registry.mu.Unlock()
}

func workspaceV2RequireTransferReleased(t *testing.T, server *WorkspaceV2Server, connection *workspaceV2Connection, workspaceID dto.WorkspaceUUID, transferID uuid.UUID) {
	t.Helper()
	require.Eventually(t, func() bool {
		connection.stateMu.RLock()
		_, present := connection.transfers[transferID]
		connection.stateMu.RUnlock()
		server.transfers.mu.Lock()
		workspaceCount := server.transfers.byWorkspace[workspaceV2HubKey{uid: connection.uid, workspaceID: workspaceID}]
		userCount := server.transfers.byUser[connection.uid]
		activeCount := len(server.transfers.active)
		server.transfers.mu.Unlock()
		return !present && workspaceCount == 0 && userCount == 0 && activeCount == 0
	}, 750*time.Millisecond, 10*time.Millisecond)
}

func workspaceV2PrepareBlobSession(t *testing.T, store service.WorkspaceBlobStore) (*WorkspaceV2Server, *gws.Conn, *workspaceV2TestEvents, *workspaceV2Connection) {
	t.Helper()
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.blobStore = store
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000218")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000219", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}
	owner := workspaceV2FindConnection(t, server, dto.WorkspaceUUID(workspaceV2StreamClientID))
	return server, conn, events, owner
}

func workspaceV2PrepareReconnectableBlobServer(t *testing.T, store service.WorkspaceBlobStore) (*WorkspaceV2Server, func(string, string) (*gws.Conn, *workspaceV2TestEvents, *workspaceV2Connection)) {
	t.Helper()
	server, connectClient := workspaceV2PrepareMultiConnectionBlobServer(t, store)
	connect := func(helloRequestID, subscribeRequestID string) (*gws.Conn, *workspaceV2TestEvents, *workspaceV2Connection) {
		return connectClient(workspaceV2StreamClientID, helloRequestID, subscribeRequestID)
	}
	return server, connect
}

func workspaceV2PrepareMultiConnectionBlobServer(t *testing.T, store service.WorkspaceBlobStore) (*WorkspaceV2Server, func(string, string, string) (*gws.Conn, *workspaceV2TestEvents, *workspaceV2Connection)) {
	t.Helper()
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	})
	t.Cleanup(server.Close)
	server.blobStore = store
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	connect := func(clientID, helloRequestID, subscribeRequestID string) (*gws.Conn, *workspaceV2TestEvents, *workspaceV2Connection) {
		conn, events := newWorkspaceV2StreamClient(t, httpServer)
		workspaceV2Send(t, conn, dto.WorkspaceActionHello, helloRequestID, workspaceV2HelloData(clientID))
		require.True(t, workspaceV2ResponseStatus(t, workspaceV2Receive(t, events)))
		if subscribeRequestID != "" {
			workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, subscribeRequestID, workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, clientID, 0))
			for range 3 {
				workspaceV2Receive(t, events)
			}
		}
		owner := workspaceV2FindConnection(t, server, dto.WorkspaceUUID(clientID))
		return conn, events, owner
	}
	return server, connect
}

func workspaceV2LoseBlobEndResponse(owner *workspaceV2Connection) (<-chan []byte, chan<- struct{}) {
	written := make(chan []byte, 1)
	releaseWriter := make(chan struct{})
	owner.writeMessage = func(opcode gws.Opcode, data []byte) error {
		if opcode == gws.OpcodeText && workspaceV2Action(data) == string(dto.WorkspaceActionBlobEnd) {
			written <- append([]byte(nil), data...)
			<-releaseWriter
			return errors.New("simulated BlobEnd response loss")
		}
		return owner.conn.WriteMessage(opcode, data)
	}
	return written, releaseWriter
}

func workspaceV2RequireConnectionRemoved(t *testing.T, server *WorkspaceV2Server, owner *workspaceV2Connection) {
	t.Helper()
	require.Eventually(t, func() bool {
		server.mu.RLock()
		_, present := server.connections[owner.conn]
		server.mu.RUnlock()
		return !present
	}, time.Second, 10*time.Millisecond)
}

func workspaceV2UploadBegin(transferID string, payload []byte) dto.WorkspaceBlobBeginMessage {
	return dto.WorkspaceBlobBeginMessage{
		WorkspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), TransferID: dto.WorkspaceUUID(transferID),
		Direction: dto.WorkspaceBlobUpload, ContentHash: workspaceV2TestBlobHash(payload), Size: uint64(len(payload)),
		ChunkSize: dto.WorkspaceBlobChunkSize, ChunkCount: workspaceV2BlobChunkCount(uint64(len(payload))),
	}
}

func workspaceV2EndForBegin(begin dto.WorkspaceBlobBeginMessage) dto.WorkspaceBlobEndMessage {
	return dto.WorkspaceBlobEndMessage{
		WorkspaceID: begin.WorkspaceID, TransferID: begin.TransferID, Direction: begin.Direction,
		ContentHash: begin.ContentHash, Size: begin.Size, ChunkCount: begin.ChunkCount,
	}
}

func workspaceV2RequestFrame(t *testing.T, action dto.WorkspaceV2Action, requestID string, data any) string {
	t.Helper()
	rawData, err := json.Marshal(data)
	require.NoError(t, err)
	envelope, err := json.Marshal(struct {
		RequestID dto.WorkspaceUUID `json:"requestId"`
		Data      json.RawMessage   `json:"data"`
	}{RequestID: dto.WorkspaceUUID(requestID), Data: rawData})
	require.NoError(t, err)
	return string(action) + "|" + string(envelope)
}

func workspaceV2SendUploadChunk(t *testing.T, conn *gws.Conn, transferID uuid.UUID, payload []byte) {
	t.Helper()
	_, digest := dto.ComputeWorkspaceBlobDigest(payload)
	header, err := dto.MarshalWorkspaceBlobHeader(dto.WorkspaceBlobHeader{
		Direction: dto.WorkspaceBlobUpload, Final: true, TransferID: transferID,
		PayloadLen: uint32(len(payload)), ChunkDigest: digest,
	})
	require.NoError(t, err)
	require.NoError(t, conn.WriteMessage(gws.OpcodeBinary, append(header[:], payload...)))
}

func workspaceV2StartDownload(t *testing.T, conn *gws.Conn, events *workspaceV2TestEvents, owner *workspaceV2Connection, payload []byte, requestID string) (dto.WorkspaceBlobEndMessage, *workspaceV2Transfer) {
	t.Helper()
	need := dto.WorkspaceBlobNeedDownloadRequest{
		WorkspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), Direction: dto.WorkspaceBlobDownload,
		ContentHash: workspaceV2TestBlobHash(payload), OperationID: dto.WorkspaceNullableUUID{Present: true}, Size: dto.WorkspaceNullableUint64{Present: true},
	}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobNeed, requestID, need)
	require.Equal(t, string(dto.WorkspaceActionBlobNeed), workspaceV2Action(workspaceV2Receive(t, events)))
	require.Equal(t, string(dto.WorkspaceActionBlobBegin), workspaceV2Action(workspaceV2Receive(t, events)))
	binaryFrame := workspaceV2Receive(t, events)
	require.Equal(t, "FNS2", string(binaryFrame[:4]))
	endFrame := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionBlobEnd), workspaceV2Action(endFrame))
	var envelope dto.WorkspaceV2Response[dto.WorkspaceBlobEndMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(endFrame), &envelope))
	require.NotNil(t, envelope.Data)
	transferID := uuid.MustParse(string(envelope.Data.TransferID))
	active := owner.workspaceV2Transfer(transferID)
	require.NotNil(t, active)
	return *envelope.Data, active
}

func workspaceV2RequireBlobBeginSuccess(t *testing.T, frame []byte, requestID string, expected dto.WorkspaceBlobBeginMessage) {
	t.Helper()
	require.Equal(t, string(dto.WorkspaceActionBlobBegin), workspaceV2Action(frame))
	var envelope dto.WorkspaceV2Response[dto.WorkspaceBlobBeginMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(frame), &envelope))
	require.True(t, envelope.Status)
	require.Nil(t, envelope.Error)
	require.NotNil(t, envelope.RequestID)
	require.Equal(t, dto.WorkspaceUUID(requestID), *envelope.RequestID)
	require.NotNil(t, envelope.Data)
	require.Equal(t, expected, *envelope.Data)
}

func workspaceV2RequireBlobEndSuccess(t *testing.T, frame []byte, requestID string, expected dto.WorkspaceBlobEndMessage) {
	t.Helper()
	require.Equal(t, string(dto.WorkspaceActionBlobEnd), workspaceV2Action(frame))
	var envelope dto.WorkspaceV2Response[dto.WorkspaceBlobEndMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(frame), &envelope))
	require.True(t, envelope.Status)
	require.Nil(t, envelope.Error)
	require.NotNil(t, envelope.RequestID)
	require.Equal(t, dto.WorkspaceUUID(requestID), *envelope.RequestID)
	require.NotNil(t, envelope.Data)
	require.Equal(t, expected, *envelope.Data)
}

func workspaceV2RequireFailureRequest(t *testing.T, frame []byte, action dto.WorkspaceV2Action, requestID string, code dto.WorkspaceV2ErrorCode) {
	t.Helper()
	require.Equal(t, string(action), workspaceV2Action(frame))
	var envelope dto.WorkspaceV2Response[struct{}]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(frame), &envelope))
	require.False(t, envelope.Status)
	require.NotNil(t, envelope.RequestID)
	require.Equal(t, dto.WorkspaceUUID(requestID), *envelope.RequestID)
	require.NotNil(t, envelope.Error)
	require.Equal(t, code, envelope.Error.Code)
}

func workspaceV2RequireExactTransferSlots(t *testing.T, server *WorkspaceV2Server, owner *workspaceV2Connection, transfer *workspaceV2Transfer) {
	t.Helper()
	owner.stateMu.RLock()
	connectionTransfers := make(map[uuid.UUID]*workspaceV2Transfer, len(owner.transfers))
	for transferID, active := range owner.transfers {
		connectionTransfers[transferID] = active
	}
	owner.stateMu.RUnlock()
	server.transfers.mu.Lock()
	workspaceSlots := make(map[workspaceV2HubKey]int, len(server.transfers.byWorkspace))
	for key, count := range server.transfers.byWorkspace {
		workspaceSlots[key] = count
	}
	userSlots := make(map[int64]int, len(server.transfers.byUser))
	for uid, count := range server.transfers.byUser {
		userSlots[uid] = count
	}
	activeSlots := make(map[*workspaceV2Transfer]struct{}, len(server.transfers.active))
	for active := range server.transfers.active {
		activeSlots[active] = struct{}{}
	}
	identitySlots := make(map[workspaceV2TransferKey]*workspaceV2Transfer, len(server.transfers.byIdentity))
	for key, active := range server.transfers.byIdentity {
		identitySlots[key] = active
	}
	server.transfers.mu.Unlock()
	if transfer == nil {
		require.Empty(t, connectionTransfers)
		require.Empty(t, workspaceSlots)
		require.Empty(t, userSlots)
		require.Empty(t, activeSlots)
		require.Empty(t, identitySlots)
		return
	}
	require.Equal(t, map[uuid.UUID]*workspaceV2Transfer{transfer.transferID: transfer}, connectionTransfers)
	require.Equal(t, map[workspaceV2HubKey]int{{uid: owner.uid, workspaceID: transfer.workspaceID}: 1}, workspaceSlots)
	require.Equal(t, map[int64]int{owner.uid: 1}, userSlots)
	require.Equal(t, map[*workspaceV2Transfer]struct{}{transfer: {}}, activeSlots)
	require.Equal(t, map[workspaceV2TransferKey]*workspaceV2Transfer{{uid: owner.uid, transferID: transfer.transferID}: transfer}, identitySlots)
}

func workspaceV2RequireCompletedReceipt(t *testing.T, owner *workspaceV2Connection, end dto.WorkspaceBlobEndMessage) {
	t.Helper()
	transferID := uuid.MustParse(string(end.TransferID))
	receipt, present := owner.workspaceV2CompletedTransfer(transferID)
	require.True(t, present)
	require.True(t, receipt.matches(end))
}

type workspaceV2BlockingUploadStore struct {
	started chan struct{}
	release chan struct{}
}

type workspaceV2CountingUploadStore struct {
	workspaceV2BlobStoreStub
	callsMu sync.Mutex
	calls   int
	started chan struct{}
	once    sync.Once
}

func (s *workspaceV2CountingUploadStore) Put(ctx context.Context, uid int64, hash dto.WorkspaceContentHash, size uint64, source io.Reader) error {
	s.callsMu.Lock()
	s.calls++
	s.callsMu.Unlock()
	s.once.Do(func() { close(s.started) })
	return s.workspaceV2BlobStoreStub.Put(ctx, uid, hash, size, source)
}

func (s *workspaceV2CountingUploadStore) callCount() int {
	s.callsMu.Lock()
	defer s.callsMu.Unlock()
	return s.calls
}

type workspaceV2DurableUploadStore struct {
	workspaceV2BlobStoreStub
	awaitingCommit chan struct{}
	releaseCommit  chan struct{}
}

type workspaceV2CleanupWinningUploadStore struct {
	workspaceV2BlobStoreStub
	committed     chan struct{}
	returnSuccess chan struct{}
}

func (s *workspaceV2CleanupWinningUploadStore) Put(_ context.Context, _ int64, _ dto.WorkspaceContentHash, _ uint64, source io.Reader) error {
	data, err := io.ReadAll(source)
	if err != nil {
		return err
	}
	s.mu.Lock()
	s.puts = append(s.puts, data)
	s.mu.Unlock()
	close(s.committed)
	<-s.returnSuccess
	return nil
}

func (s *workspaceV2CleanupWinningUploadStore) callCount() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.puts)
}

type workspaceV2ContextOnlyUploadStore struct {
	workspaceV2BlobStoreStub
	started  chan struct{}
	canceled chan struct{}
}

func (s *workspaceV2ContextOnlyUploadStore) Put(ctx context.Context, _ int64, _ dto.WorkspaceContentHash, _ uint64, _ io.Reader) error {
	close(s.started)
	<-ctx.Done()
	close(s.canceled)
	return ctx.Err()
}

func (s *workspaceV2DurableUploadStore) Put(ctx context.Context, _ int64, _ dto.WorkspaceContentHash, _ uint64, source io.Reader) error {
	if _, err := io.ReadAll(source); err != nil {
		return err
	}
	close(s.awaitingCommit)
	select {
	case <-s.releaseCommit:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

type workspaceV2UploadErrorStore struct {
	workspaceV2BlobStoreStub
	putErr error
}

func (s *workspaceV2UploadErrorStore) Put(_ context.Context, _ int64, _ dto.WorkspaceContentHash, _ uint64, source io.Reader) error {
	if _, err := io.ReadAll(source); err != nil {
		return err
	}
	return s.putErr
}

func (s *workspaceV2BlockingUploadStore) Has(context.Context, int64, dto.WorkspaceContentHash, uint64) (bool, error) {
	return true, nil
}

func (s *workspaceV2BlockingUploadStore) Put(ctx context.Context, _ int64, _ dto.WorkspaceContentHash, _ uint64, source io.Reader) error {
	close(s.started)
	select {
	case <-s.release:
	case <-ctx.Done():
		return ctx.Err()
	}
	_, err := io.ReadAll(source)
	return err
}

func (s *workspaceV2BlockingUploadStore) Open(context.Context, int64, dto.WorkspaceContentHash) (io.ReadCloser, uint64, error) {
	return io.NopCloser(bytes.NewReader(nil)), 0, nil
}

func (s *workspaceV2BlockingUploadStore) ReconcileAndGC(context.Context, int64, time.Time) error {
	return nil
}

func workspaceV2TestTransfer(connection *workspaceV2Connection, workspaceID dto.WorkspaceUUID, now time.Time) *workspaceV2Transfer {
	return &workspaceV2Transfer{
		owner:        connection,
		workspaceID:  workspaceID,
		transferID:   uuid.New(),
		createdAt:    now,
		lastActivity: now,
	}
}
