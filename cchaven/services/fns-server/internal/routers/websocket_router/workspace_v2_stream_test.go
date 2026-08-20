package websocket_router

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/google/uuid"
	"github.com/haierkeys/fast-note-sync-service/internal/config"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/middleware"
	"github.com/haierkeys/fast-note-sync-service/internal/service"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
	"github.com/lxzan/gws"
	"github.com/stretchr/testify/require"
	"go.uber.org/zap"
	"go.uber.org/zap/zaptest/observer"
)

const workspaceV2StreamClientID = "10000000-0000-4000-8000-000000000010"

func TestWorkspaceV2HelloReturnsNegotiatedFixedLimits(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	requestID := "10000000-0000-4000-8000-000000000011"
	workspaceV2Send(t, conn, dto.WorkspaceActionHello, requestID, workspaceV2HelloData(workspaceV2StreamClientID))

	response := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionHello), workspaceV2Action(response))
	var envelope dto.WorkspaceV2Response[dto.WorkspaceHelloResponse]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(response), &envelope))
	require.Equal(t, requestID, string(*envelope.RequestID))
	require.True(t, envelope.Status)
	require.NoError(t, envelope.Data.Validate())
	require.Equal(t, uint32(dto.WorkspaceMaxControlFrameBytes), envelope.Data.MaxControlFrameBytes)
	require.Equal(t, uint32(dto.WorkspaceBlobChunkSize), envelope.Data.MaxBinaryChunkBytes)
	require.Equal(t, dto.WorkspaceMaxBlobBytes, envelope.Data.MaxBlobBytes)
	require.Equal(t, uint32(4), envelope.Data.MaxTransfersPerConnection)
	require.Equal(t, uint32(25), envelope.Data.HeartbeatSeconds)
	server.CloseAllConnections()
}

func TestWorkspaceV2RejectsNonHelloBeforeHello(t *testing.T) {
	_, conn, events := newWorkspaceV2StreamConnection(t, nil)
	requestID := "10000000-0000-4000-8000-000000000012"
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, requestID, workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))

	response := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionSubscribe), workspaceV2Action(response))
	var envelope dto.WorkspaceV2Response[struct{}]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(response), &envelope))
	require.False(t, envelope.Status)
	require.Equal(t, dto.WorkspaceErrorInvalidRequest, envelope.Error.Code)
}

func TestWorkspaceV2RejectsSecondHelloAndClientIDChange(t *testing.T) {
	_, conn, events := newWorkspaceV2StreamConnection(t, nil)
	workspaceV2Send(t, conn, dto.WorkspaceActionHello, "10000000-0000-4000-8000-000000000013", workspaceV2HelloData(workspaceV2StreamClientID))
	require.True(t, workspaceV2ResponseStatus(t, workspaceV2Receive(t, events)))
	workspaceV2Send(t, conn, dto.WorkspaceActionHello, "10000000-0000-4000-8000-000000000014", workspaceV2HelloData("20000000-0000-4000-8000-000000000020"))
	response := workspaceV2Receive(t, events)
	require.False(t, workspaceV2ResponseStatus(t, response))
	require.Equal(t, dto.WorkspaceErrorInvalidRequest, workspaceV2ErrorCode(t, response))
}

func TestWorkspaceV2RejectsDuplicateRequestIDOnConnection(t *testing.T) {
	_, conn, events := newWorkspaceV2StreamConnection(t, nil)
	requestID := "10000000-0000-4000-8000-000000000015"
	workspaceV2Send(t, conn, dto.WorkspaceActionHello, requestID, workspaceV2HelloData(workspaceV2StreamClientID))
	require.True(t, workspaceV2ResponseStatus(t, workspaceV2Receive(t, events)))
	workspaceV2Send(t, conn, dto.WorkspaceActionHello, requestID, workspaceV2HelloData(workspaceV2StreamClientID))
	response := workspaceV2Receive(t, events)
	require.False(t, workspaceV2ResponseStatus(t, response))
	require.Equal(t, dto.WorkspaceErrorInvalidRequest, workspaceV2ErrorCode(t, response))
}

func TestWorkspaceV2SubscribeAuthorizesBeforeServiceCall(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	serviceStub := &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	server.syncService = serviceStub
	server.access = NewWorkspaceV2AccessPolicy(configForWorkspaceV2Test(canonicalTempDir(t)))
	workspaceV2Send(t, conn, dto.WorkspaceActionHello, "10000000-0000-4000-8000-000000000016", workspaceV2HelloData(workspaceV2StreamClientID))
	require.True(t, workspaceV2ResponseStatus(t, workspaceV2Receive(t, events)))
	server.access = NewWorkspaceV2AccessPolicy(config.WorkspaceConfig{MaxWorkspacesPerUser: config.WorkspaceMaxPerUser})
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000017", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	response := workspaceV2Receive(t, events)
	require.False(t, workspaceV2ResponseStatus(t, response))
	require.Equal(t, dto.WorkspaceErrorForbidden, workspaceV2ErrorCode(t, response))
	require.Zero(t, serviceStub.subscribeCalls)
}

func TestWorkspaceV2SubscribeWritesSnapshotBeginEntriesEndInUTF8ByteOrder(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	serviceStub := &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	server.syncService = serviceStub
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000018")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000019", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))

	begin := workspaceV2Receive(t, events)
	entry := workspaceV2Receive(t, events)
	end := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionSnapshotBegin), workspaceV2Action(begin))
	require.Equal(t, string(dto.WorkspaceActionSnapshotEntry), workspaceV2Action(entry))
	require.Equal(t, string(dto.WorkspaceActionSnapshotEnd), workspaceV2Action(end))
	var beginEnvelope dto.WorkspaceV2Response[dto.WorkspaceSnapshotBeginMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(begin), &beginEnvelope))
	require.Equal(t, uint32(1), beginEnvelope.Data.EntryCount)
	var endEnvelope dto.WorkspaceV2Response[dto.WorkspaceSnapshotEndMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(end), &endEnvelope))
	require.NoError(t, endEnvelope.Data.ValidateAgainst(*beginEnvelope.Data))
	require.Equal(t, uint32(1), endEnvelope.Data.DeliveredCount)
	require.Equal(t, dto.WorkspaceRevision(3), endEnvelope.Data.FinalRevision)
}

func TestWorkspaceV2SubscribeEmptyIncrementalStillWritesBeginAndEnd(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.syncService = &workspaceV2StreamService{changeSet: &domain.WorkspaceChangeSet{
		Mode: dto.WorkspaceSnapshotIncremental, FromRevision: 3, FinalRevision: 3,
	}}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000020")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000021", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 3))
	begin := workspaceV2Receive(t, events)
	end := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionSnapshotBegin), workspaceV2Action(begin))
	require.Equal(t, string(dto.WorkspaceActionSnapshotEnd), workspaceV2Action(end))
}

func TestWorkspaceV2ConflictOnlyReconnectStreamsAuthoritativeConflictWithoutAck(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000073")
	conflict := workspaceV2TestConflictCreated(workspaceID, "40000000-0000-4000-8000-000000000073", operationID, clientID)
	server.syncService = &workspaceV2StreamService{changeSet: &domain.WorkspaceChangeSet{
		Mode: dto.WorkspaceSnapshotIncremental, FromRevision: 3, FinalRevision: 3, ConflictCount: 1,
		PendingConflicts: &workspaceV2StreamCursor{items: []*dto.WorkspaceConflictCreatedMessage{conflict}},
	}}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000074")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000075", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 3))
	begin := workspaceV2Receive(t, events)
	created := workspaceV2Receive(t, events)
	end := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionSnapshotBegin), workspaceV2Action(begin))
	require.Equal(t, string(dto.WorkspaceActionConflictCreated), workspaceV2Action(created))
	require.Equal(t, string(dto.WorkspaceActionSnapshotEnd), workspaceV2Action(end))
	var beginEnvelope dto.WorkspaceV2Response[dto.WorkspaceSnapshotBeginMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(begin), &beginEnvelope))
	require.Zero(t, beginEnvelope.Data.EventCount)
	require.Equal(t, uint32(1), beginEnvelope.Data.ConflictCount)
	var endEnvelope dto.WorkspaceV2Response[dto.WorkspaceSnapshotEndMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(end), &endEnvelope))
	require.Equal(t, uint32(1), endEnvelope.Data.DeliveredCount)
}

func TestWorkspaceV2AckRejectsSameRevisionAfterConflictOnlyReconnect(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.syncService = &workspaceV2StreamService{changeSet: &domain.WorkspaceChangeSet{Mode: dto.WorkspaceSnapshotIncremental, FromRevision: 3, FinalRevision: 3}}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000076")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000077", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 3))
	workspaceV2Receive(t, events)
	workspaceV2Receive(t, events)
	ack := dto.WorkspaceAckRequest{WorkspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), ClientID: dto.WorkspaceUUID(workspaceV2StreamClientID), Revision: 3}
	workspaceV2Send(t, conn, dto.WorkspaceActionAck, "10000000-0000-4000-8000-000000000078", ack)
	response := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionAck), workspaceV2Action(response))
	require.False(t, workspaceV2ResponseStatus(t, response))
	require.Equal(t, dto.WorkspaceErrorInvalidRequest, workspaceV2ErrorCode(t, response))
}

func TestWorkspaceV2DisconnectUnregistersHubSubscription(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000079")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000080", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}
	require.NoError(t, conn.NetConn().Close())
	select {
	case <-events.closes:
	case <-time.After(time.Second):
		t.Fatal("workspace v2 client did not close")
	}
	key := workspaceV2HubKey{uid: 41, workspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)}
	deadline := time.Now().Add(time.Second)
	for time.Now().Before(deadline) {
		server.hub.mu.Lock()
		remaining := len(server.hub.subscribers[key])
		server.hub.mu.Unlock()
		if remaining == 0 {
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatal("workspace v2 disconnect left a hub subscription")
}

func TestWorkspaceV2SubscribeWritesIncrementalMixedRevisionItemsAndAuthoritativeConflicts(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	conflictRevision, err := dto.ParseWorkspaceConflictRevision("1")
	require.NoError(t, err)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000001")
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	mutation := dto.WorkspaceMutation{
		WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Path: "notes/a.md",
		Kind: dto.WorkspaceMutationMkdir, ContentHash: dto.WorkspaceNullableHash{Present: true},
		Metadata: dto.WorkspaceFileMetadata{},
	}
	state := dto.WorkspacePathState{Path: "notes/a.md", PathRevision: 1, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	resolvedState := state
	resolvedState.PathRevision = 2
	resolved := &dto.WorkspaceConflictResolvedMessage{
		WorkspaceID: workspaceID, ConflictID: "40000000-0000-4000-8000-000000000001", ConflictRevision: conflictRevision,
		OperationID: operationID, Revision: 2, Choice: dto.WorkspaceConflictKeepCurrent, PathState: resolvedState, ResolvedByClientID: clientID,
	}
	server.syncService = &workspaceV2StreamService{changeSet: &domain.WorkspaceChangeSet{
		Mode: dto.WorkspaceSnapshotIncremental, FromRevision: 0, FinalRevision: 2, EventCount: 2,
		RevisionItems: []domain.WorkspaceRevisionItem{
			{Revision: 1, Event: &domain.WorkspaceStoredEvent{Revision: 1, OperationID: operationID, OriginClientID: clientID, Mutation: mutation, PathState: state}},
			{Revision: 2, ConflictResolved: resolved},
		},
	}}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000024")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000025", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	begin := workspaceV2Receive(t, events)
	event := workspaceV2Receive(t, events)
	resolution := workspaceV2Receive(t, events)
	end := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionSnapshotBegin), workspaceV2Action(begin))
	require.Equal(t, string(dto.WorkspaceActionEvent), workspaceV2Action(event))
	require.Equal(t, string(dto.WorkspaceActionConflictResolved), workspaceV2Action(resolution))
	require.Equal(t, string(dto.WorkspaceActionSnapshotEnd), workspaceV2Action(end))
	var beginEnvelope dto.WorkspaceV2Response[dto.WorkspaceSnapshotBeginMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(begin), &beginEnvelope))
	require.Equal(t, uint32(2), beginEnvelope.Data.EventCount)
	var eventEnvelope dto.WorkspaceV2Response[dto.WorkspaceEventMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(event), &eventEnvelope))
	require.Equal(t, uint32(0), eventEnvelope.Data.Index)
	var resolvedEnvelope dto.WorkspaceV2Response[dto.WorkspaceConflictResolvedMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(resolution), &resolvedEnvelope))
	require.Equal(t, dto.WorkspaceRevision(2), resolvedEnvelope.Data.Revision)
	var endEnvelope dto.WorkspaceV2Response[dto.WorkspaceSnapshotEndMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(end), &endEnvelope))
	require.Equal(t, uint32(2), endEnvelope.Data.DeliveredCount)
}

func TestWorkspaceV2SubscribeRejectsIncrementalFinalRevisionGap(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000040")
	mutation := dto.WorkspaceMutation{
		WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Path: "notes/gap.md",
		Kind: dto.WorkspaceMutationMkdir, ContentHash: dto.WorkspaceNullableHash{Present: true}, Metadata: dto.WorkspaceFileMetadata{},
	}
	state := dto.WorkspacePathState{Path: "notes/gap.md", PathRevision: 1, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	server.syncService = &workspaceV2StreamService{changeSet: &domain.WorkspaceChangeSet{
		Mode: dto.WorkspaceSnapshotIncremental, FromRevision: 0, FinalRevision: 2, EventCount: 1,
		RevisionItems: []domain.WorkspaceRevisionItem{{Revision: 1, Event: &domain.WorkspaceStoredEvent{
			Revision: 1, OperationID: operationID, OriginClientID: clientID, Mutation: mutation, PathState: state,
		}}},
	}}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000041")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000042", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 2 {
		workspaceV2Receive(t, events)
	}
	select {
	case <-events.closes:
	case <-time.After(time.Second):
		t.Fatal("invalid incremental final revision was accepted")
	}
}

func TestWorkspaceV2SubscribeHasNoSuccessAckAction(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000022")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000023", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	first := workspaceV2Receive(t, events)
	require.NotEqual(t, string(dto.WorkspaceActionSubscribe), workspaceV2Action(first))
}

func TestWorkspaceV2SubscribeBuffersMutationCommittedDuringServiceRead(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	started := make(chan struct{})
	release := make(chan struct{})
	serviceStub := &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet(), subscribeStarted: started, subscribeRelease: release}
	server.syncService = serviceStub
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000026")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000027", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("subscribe did not enter service")
	}
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000002")
	mutation := dto.WorkspaceMutation{WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Path: "notes/live.md", Kind: dto.WorkspaceMutationMkdir, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	state := dto.WorkspacePathState{Path: "notes/live.md", PathRevision: 4, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	revision := dto.WorkspaceRevision(4)
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{
		kind: workspaceV2LiveEvent, treeRevision: &revision, mutation: &mutation,
		accepted: &dto.WorkspaceMutationAcceptedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Revision: revision, PathState: state},
	})
	close(release)

	require.Equal(t, string(dto.WorkspaceActionSnapshotBegin), workspaceV2Action(workspaceV2Receive(t, events)))
	require.Equal(t, string(dto.WorkspaceActionSnapshotEntry), workspaceV2Action(workspaceV2Receive(t, events)))
	require.Equal(t, string(dto.WorkspaceActionSnapshotEnd), workspaceV2Action(workspaceV2Receive(t, events)))
	live := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionEvent), workspaceV2Action(live))
	var eventEnvelope dto.WorkspaceV2Response[dto.WorkspaceEventMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(live), &eventEnvelope))
	require.Equal(t, dto.WorkspaceRevision(4), eventEnvelope.Data.Revision)
}

func TestWorkspaceV2SubscribeDropsBufferedRevisionAlreadyCoveredByFinalRevision(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	started := make(chan struct{})
	release := make(chan struct{})
	serviceStub := &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet(), subscribeStarted: started, subscribeRelease: release}
	server.syncService = serviceStub
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000028")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000029", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	<-started
	revision := dto.WorkspaceRevision(3)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000003")
	mutation := dto.WorkspaceMutation{WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Path: "notes/covered.md", Kind: dto.WorkspaceMutationMkdir, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	state := dto.WorkspacePathState{Path: "notes/covered.md", PathRevision: 3, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{
		kind: workspaceV2LiveEvent, treeRevision: &revision, mutation: &mutation,
		accepted: &dto.WorkspaceMutationAcceptedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Revision: revision, PathState: state},
	})
	close(release)
	workspaceV2Receive(t, events)
	workspaceV2Receive(t, events)
	workspaceV2Receive(t, events)
	select {
	case extra := <-events.messages:
		t.Fatalf("covered live revision was duplicated: %s", extra)
	case <-time.After(100 * time.Millisecond):
	}
}

func TestWorkspaceV2FlushPreservesContiguousOrderWhenRevisionArrivesLate(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	started := make(chan struct{})
	release := make(chan struct{})
	server.syncService = &workspaceV2StreamService{
		changeSet:        workspaceV2FullChangeSet(),
		subscribeStarted: started,
		subscribeRelease: release,
	}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000116")
	events.messages = make(chan []byte)
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000117", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	<-started

	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	queuedRevision := dto.WorkspaceRevision(5)
	queuedMutation := workspaceV2TestMkdirMutation(workspaceID, clientID, "30000000-0000-4000-8000-000000000116", "notes/queued-5.md")
	queuedState := dto.WorkspacePathState{Path: queuedMutation.Path, PathRevision: queuedRevision, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{
		kind: workspaceV2LiveEvent, treeRevision: &queuedRevision, mutation: &queuedMutation,
		accepted: &dto.WorkspaceMutationAcceptedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: queuedMutation.OperationID, Revision: queuedRevision, PathState: queuedState},
	})
	close(release)

	require.Equal(t, string(dto.WorkspaceActionSnapshotBegin), workspaceV2Action(workspaceV2Receive(t, events)))
	require.Equal(t, string(dto.WorkspaceActionSnapshotEntry), workspaceV2Action(workspaceV2Receive(t, events)))
	require.Equal(t, string(dto.WorkspaceActionSnapshotEnd), workspaceV2Action(workspaceV2Receive(t, events)))

	lateRevision := dto.WorkspaceRevision(4)
	lateMutation := workspaceV2TestMkdirMutation(workspaceID, clientID, "30000000-0000-4000-8000-000000000117", "notes/queued-4.md")
	lateState := dto.WorkspacePathState{Path: lateMutation.Path, PathRevision: lateRevision, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{
		kind: workspaceV2LiveEvent, treeRevision: &lateRevision, mutation: &lateMutation,
		accepted: &dto.WorkspaceMutationAcceptedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: lateMutation.OperationID, Revision: lateRevision, PathState: lateState},
	})

	first := workspaceV2Receive(t, events)
	var firstEnvelope dto.WorkspaceV2Response[dto.WorkspaceEventMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(first), &firstEnvelope))
	require.Equal(t, string(dto.WorkspaceActionEvent), workspaceV2Action(first))
	require.Equal(t, lateRevision, firstEnvelope.Data.Revision)
	second := workspaceV2Receive(t, events)
	var secondEnvelope dto.WorkspaceV2Response[dto.WorkspaceEventMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(second), &secondEnvelope))
	require.Equal(t, string(dto.WorkspaceActionEvent), workspaceV2Action(second))
	require.Equal(t, queuedRevision, secondEnvelope.Data.Revision)
}

func TestWorkspaceV2SubscribeCancelsBufferedConflictCreatedWhenResolvedBeforeEnd(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	started := make(chan struct{})
	release := make(chan struct{})
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet(), subscribeStarted: started, subscribeRelease: release}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000030")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000031", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	<-started

	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	conflictID := dto.WorkspaceUUID("40000000-0000-4000-8000-000000000030")
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000030")
	conflict := workspaceV2TestConflictCreated(workspaceID, conflictID, operationID, clientID)
	revision := dto.WorkspaceRevision(4)
	resolved := &dto.WorkspaceConflictResolvedMessage{
		WorkspaceID: workspaceID, ConflictID: conflictID, ConflictRevision: conflict.ConflictRevision,
		OperationID: operationID, Revision: revision, Choice: dto.WorkspaceConflictDelete,
		PathState:          dto.WorkspacePathState{Path: "notes/live.md", PathRevision: revision, Kind: dto.WorkspaceEntryTombstone, ContentHash: dto.WorkspaceNullableHash{Present: true}, Tombstone: true},
		ResolvedByClientID: clientID,
	}
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{
		kind: workspaceV2LiveConflictCreated, conflictID: conflictID, conflict: conflict,
	})
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{
		kind: workspaceV2LiveConflictResolved, conflictID: conflictID, treeRevision: &revision, resolved: resolved,
	})
	close(release)

	for range 3 {
		workspaceV2Receive(t, events)
	}
	resolvedFrame := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionConflictResolved), workspaceV2Action(resolvedFrame))
	select {
	case extra := <-events.messages:
		require.NotEqual(t, string(dto.WorkspaceActionConflictCreated), workspaceV2Action(extra), "resolved buffered conflict must not recreate Created")
	case <-time.After(100 * time.Millisecond):
	}
}

func TestWorkspaceV2SubscribeUnregistersPreviousWorkspaceBeforeReplacingSubscription(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000032")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000033", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}
	secondWorkspaceID := "50000000-0000-4000-8000-000000000032"
	server.access = NewWorkspaceV2AccessPolicy(config.WorkspaceConfig{
		MaxWorkspacesPerUser: config.WorkspaceMaxPerUser,
		Roots: []config.WorkspaceRootConfig{
			{UID: 41, WorkspaceID: workspaceV2SecurityWorkspaceID, Root: canonicalTempDir(t)},
			{UID: 41, WorkspaceID: secondWorkspaceID, Root: canonicalTempDir(t)},
		},
	})
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000034", workspaceV2SubscribeData(secondWorkspaceID, workspaceV2StreamClientID, 0))
	for range 2 {
		workspaceV2Receive(t, events)
	}

	server.hub.mu.Lock()
	defer server.hub.mu.Unlock()
	oldKey := workspaceV2HubKey{uid: 41, workspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)}
	newKey := workspaceV2HubKey{uid: 41, workspaceID: dto.WorkspaceUUID(secondWorkspaceID)}
	require.Empty(t, server.hub.subscribers[oldKey])
	require.Len(t, server.hub.subscribers[newKey], 1)
}

func TestWorkspaceV2LiveResolutionAdvancesLastDeliveredRevision(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000035")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000036", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}

	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	conflictID := dto.WorkspaceUUID("40000000-0000-4000-8000-000000000035")
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000035")
	conflictRevision, err := dto.ParseWorkspaceConflictRevision("1")
	require.NoError(t, err)
	revision := dto.WorkspaceRevision(4)
	resolved := &dto.WorkspaceConflictResolvedMessage{
		WorkspaceID: workspaceID, ConflictID: conflictID, ConflictRevision: conflictRevision,
		OperationID: operationID, Revision: revision, Choice: dto.WorkspaceConflictDelete,
		PathState:          dto.WorkspacePathState{Path: "notes/live.md", PathRevision: revision, Kind: dto.WorkspaceEntryTombstone, ContentHash: dto.WorkspaceNullableHash{Present: true}, Tombstone: true},
		ResolvedByClientID: clientID,
	}
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{
		kind: workspaceV2LiveConflictResolved, conflictID: conflictID, treeRevision: &revision, resolved: resolved,
	})
	frame := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionConflictResolved), workspaceV2Action(frame))

	server.mu.RLock()
	var connection *workspaceV2Connection
	for _, candidate := range server.connections {
		connection = candidate
		break
	}
	server.mu.RUnlock()
	require.NotNil(t, connection)
	connection.stateMu.RLock()
	lastDelivered := connection.subscription.lastDelivered
	connection.stateMu.RUnlock()
	require.Equal(t, revision, lastDelivered)
}

func TestWorkspaceV2DrainsRevisionQueuedWhileEarlierLiveSendWaits(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000095")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000096", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}

	server.mu.RLock()
	var connection *workspaceV2Connection
	for _, candidate := range server.connections {
		connection = candidate
		break
	}
	server.mu.RUnlock()
	require.NotNil(t, connection)
	connection.stateMu.RLock()
	subscription := connection.subscription
	connection.stateMu.RUnlock()
	require.NotNil(t, subscription)

	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	firstRevision := dto.WorkspaceRevision(4)
	secondRevision := dto.WorkspaceRevision(5)
	firstMutation := workspaceV2TestMkdirMutation(workspaceID, clientID, "30000000-0000-4000-8000-000000000095", "notes/queued-4.md")
	secondMutation := workspaceV2TestMkdirMutation(workspaceID, clientID, "30000000-0000-4000-8000-000000000096", "notes/queued-5.md")
	firstState := dto.WorkspacePathState{Path: firstMutation.Path, PathRevision: firstRevision, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	secondState := dto.WorkspacePathState{Path: secondMutation.Path, PathRevision: secondRevision, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	first := workspaceV2LiveNotification{
		kind: workspaceV2LiveEvent, treeRevision: &firstRevision, mutation: &firstMutation,
		accepted: &dto.WorkspaceMutationAcceptedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: firstMutation.OperationID, Revision: firstRevision, PathState: firstState},
	}
	second := workspaceV2LiveNotification{
		kind: workspaceV2LiveEvent, treeRevision: &secondRevision, mutation: &secondMutation,
		accepted: &dto.WorkspaceMutationAcceptedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: secondMutation.OperationID, Revision: secondRevision, PathState: secondState},
	}

	subscription.dispatchMu.Lock()
	firstPublishStarted := make(chan struct{})
	firstPublishDone := make(chan struct{})
	go func() {
		close(firstPublishStarted)
		server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, first)
		close(firstPublishDone)
	}()
	<-firstPublishStarted
	time.Sleep(20 * time.Millisecond)
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, second)
	subscription.dispatchMu.Unlock()

	firstFrame := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionEvent), workspaceV2Action(firstFrame))
	var firstEnvelope dto.WorkspaceV2Response[dto.WorkspaceEventMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(firstFrame), &firstEnvelope))
	require.Equal(t, firstRevision, firstEnvelope.Data.Revision)
	select {
	case secondFrame := <-events.messages:
		var secondEnvelope dto.WorkspaceV2Response[dto.WorkspaceEventMessage]
		require.NoError(t, json.Unmarshal(workspaceV2Payload(secondFrame), &secondEnvelope))
		require.Equal(t, secondRevision, secondEnvelope.Data.Revision)
	case <-time.After(time.Second):
		t.Fatal("live revision queued during an earlier send was not drained")
	}
	select {
	case <-firstPublishDone:
	case <-time.After(time.Second):
		t.Fatal("first live publish did not complete")
	}
}

func TestWorkspaceV2BuffersLiveConflictBacklogWithBoundedCapacity(t *testing.T) {
	subscription := &workspaceV2Subscription{
		streaming:            true,
		pendingRevisionItems: make(map[dto.WorkspaceRevision][]workspaceV2LiveNotification),
		pendingConflicts:     make(map[dto.WorkspaceUUID]*workspaceV2ConflictBuffer),
	}
	connection := &workspaceV2Connection{subscription: subscription}
	for index := 0; index < workspaceV2LiveBacklogDepth+1; index++ {
		conflictID := dto.WorkspaceUUID(fmt.Sprintf("40000000-0000-4000-8000-%012d", index))
		connection.bufferLiveNotification(workspaceV2LiveNotification{
			kind: workspaceV2LiveConflictCreated, conflictID: conflictID,
		})
	}

	subscriptionState := subscription
	require.True(t, subscriptionState.overflowed)
	require.LessOrEqual(t, len(subscriptionState.pendingConflicts), workspaceV2LiveBacklogDepth)
}

func TestWorkspaceV2FlushCountsNotificationsAddedDuringSend(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000180")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000181", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}

	origin := workspaceV2FindConnection(t, server, dto.WorkspaceUUID(workspaceV2StreamClientID))
	origin.stateMu.RLock()
	subscription := origin.subscription
	origin.stateMu.RUnlock()
	require.NotNil(t, subscription)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	firstRevision := dto.WorkspaceRevision(4)
	secondRevision := dto.WorkspaceRevision(5)
	firstMutation := workspaceV2TestMkdirMutation(workspaceID, clientID, "30000000-0000-4000-8000-000000000180", "notes/flush-4.md")
	secondMutation := workspaceV2TestMkdirMutation(workspaceID, clientID, "30000000-0000-4000-8000-000000000181", "notes/flush-5.md")
	firstState := dto.WorkspacePathState{Path: firstMutation.Path, PathRevision: firstRevision, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	secondState := dto.WorkspacePathState{Path: secondMutation.Path, PathRevision: secondRevision, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	first := workspaceV2LiveNotification{
		kind: workspaceV2LiveEvent, treeRevision: &firstRevision, mutation: &firstMutation,
		accepted: &dto.WorkspaceMutationAcceptedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: firstMutation.OperationID, Revision: firstRevision, PathState: firstState},
	}
	second := workspaceV2LiveNotification{
		kind: workspaceV2LiveEvent, treeRevision: &secondRevision, mutation: &secondMutation,
		accepted: &dto.WorkspaceMutationAcceptedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: secondMutation.OperationID, Revision: secondRevision, PathState: secondState},
	}
	origin.stateMu.Lock()
	subscription.flushing = true
	subscription.pendingRevisionItems[firstRevision] = []workspaceV2LiveNotification{first}
	origin.stateMu.Unlock()

	blocked := make(chan struct{})
	release := make(chan struct{})
	origin.writeMessage = func(opcode gws.Opcode, data []byte) error {
		if opcode == gws.OpcodeText {
			select {
			case <-blocked:
			default:
				close(blocked)
				<-release
			}
		}
		return origin.conn.WriteMessage(opcode, data)
	}
	flushDone := make(chan error, 1)
	go func() {
		flushDone <- origin.flushBufferedLive(subscription, dto.WorkspaceRevision(3), nil)
	}()
	select {
	case <-blocked:
	case <-time.After(time.Second):
		t.Fatal("flush did not block in the real outbound writer")
	}
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, second)
	close(release)
	require.NoError(t, <-flushDone)

	firstFrame := workspaceV2Receive(t, events)
	secondFrame := workspaceV2Receive(t, events)
	var firstEnvelope dto.WorkspaceV2Response[dto.WorkspaceEventMessage]
	var secondEnvelope dto.WorkspaceV2Response[dto.WorkspaceEventMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(firstFrame), &firstEnvelope))
	require.NoError(t, json.Unmarshal(workspaceV2Payload(secondFrame), &secondEnvelope))
	require.Equal(t, string(dto.WorkspaceActionEvent), workspaceV2Action(firstFrame))
	require.Equal(t, string(dto.WorkspaceActionEvent), workspaceV2Action(secondFrame))
	require.Equal(t, firstRevision, firstEnvelope.Data.Revision)
	require.Equal(t, secondRevision, secondEnvelope.Data.Revision)
}

func TestWorkspaceV2IgnoresNotificationForPreviousWorkspace(t *testing.T) {
	current := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	previous := dto.WorkspaceUUID("50000000-0000-4000-8000-000000000099")
	subscription := &workspaceV2Subscription{
		workspaceID:          current,
		pendingRevisionItems: make(map[dto.WorkspaceRevision][]workspaceV2LiveNotification),
		pendingConflicts:     make(map[dto.WorkspaceUUID]*workspaceV2ConflictBuffer),
	}
	connection := &workspaceV2Connection{subscription: subscription}
	connection.bufferLiveNotification(workspaceV2LiveNotification{
		workspaceID: previous, kind: workspaceV2LiveConflictCreated,
		conflictID: "40000000-0000-4000-8000-000000000099",
	})
	require.Empty(t, subscription.pendingConflicts)
	require.Empty(t, subscription.pendingRevisionItems)
}

func TestWorkspaceV2MutationResponsePrecedesOriginEvent(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000050")
	mutation := workspaceV2TestMkdirMutation(workspaceID, clientID, operationID, "notes/mutation.md")
	state := dto.WorkspacePathState{Path: mutation.Path, PathRevision: 4, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	accepted := &dto.WorkspaceMutationAcceptedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Revision: 4, PathState: state}
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		applyMutation: func(context.Context, int64, dto.WorkspaceMutation) (*service.WorkspaceMutationOutcome, error) {
			return &service.WorkspaceMutationOutcome{Accepted: accepted}, nil
		},
	}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000051")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000052", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}
	workspaceV2Send(t, conn, dto.WorkspaceActionMutation, "10000000-0000-4000-8000-000000000053", mutation)
	response := workspaceV2Receive(t, events)
	event := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionMutationAccepted), workspaceV2Action(response))
	require.Equal(t, string(dto.WorkspaceActionEvent), workspaceV2Action(event))
	var responseEnvelope dto.WorkspaceV2Response[dto.WorkspaceMutationAcceptedMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(response), &responseEnvelope))
	require.Equal(t, "10000000-0000-4000-8000-000000000053", string(*responseEnvelope.RequestID))
}

func TestWorkspaceV2CommittedMutationPublishesToPeerWhenOriginWriterFails(t *testing.T) {
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	})
	originConn, originEvents := newWorkspaceV2StreamClient(t, httpServer)
	peerConn, peerEvents := newWorkspaceV2StreamClient(t, httpServer)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	originClientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	peerClientID := dto.WorkspaceUUID("20000000-0000-4000-8000-000000000030")
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000150")
	mutation := workspaceV2TestMkdirMutation(workspaceID, originClientID, operationID, "notes/committed-after-write-failure.md")
	state := dto.WorkspacePathState{Path: mutation.Path, PathRevision: 4, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	accepted := &dto.WorkspaceMutationAcceptedMessage{WorkspaceID: workspaceID, ClientID: originClientID, OperationID: operationID, Revision: 4, PathState: state}
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		applyMutation: func(context.Context, int64, dto.WorkspaceMutation) (*service.WorkspaceMutationOutcome, error) {
			return &service.WorkspaceMutationOutcome{Accepted: accepted}, nil
		},
	}
	workspaceV2Hello(t, originConn, originEvents, "10000000-0000-4000-8000-000000000151")
	workspaceV2Send(t, peerConn, dto.WorkspaceActionHello, "10000000-0000-4000-8000-000000000152", workspaceV2HelloData(string(peerClientID)))
	require.True(t, workspaceV2ResponseStatus(t, workspaceV2Receive(t, peerEvents)))
	workspaceV2Send(t, originConn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000153", workspaceV2SubscribeData(string(workspaceID), string(originClientID), 0))
	workspaceV2Send(t, peerConn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000154", workspaceV2SubscribeData(string(workspaceID), string(peerClientID), 0))
	for range 3 {
		workspaceV2Receive(t, originEvents)
		workspaceV2Receive(t, peerEvents)
	}
	origin := workspaceV2FindConnection(t, server, originClientID)
	origin.writeMessage = func(opcode gws.Opcode, data []byte) error {
		if opcode == gws.OpcodeText && workspaceV2Action(data) == string(dto.WorkspaceActionMutationAccepted) {
			return errors.New("simulated origin response writer failure after commit")
		}
		return origin.conn.WriteMessage(opcode, data)
	}
	workspaceV2Send(t, originConn, dto.WorkspaceActionMutation, "10000000-0000-4000-8000-000000000155", mutation)
	select {
	case frame := <-peerEvents.messages:
		require.Equal(t, string(dto.WorkspaceActionEvent), workspaceV2Action(frame))
		var envelope dto.WorkspaceV2Response[dto.WorkspaceEventMessage]
		require.NoError(t, json.Unmarshal(workspaceV2Payload(frame), &envelope))
		require.Equal(t, dto.WorkspaceRevision(4), envelope.Data.Revision)
	case <-time.After(2 * time.Second):
		t.Fatal("peer did not receive committed event after origin response write failure")
	}
}

func TestWorkspaceV2CommittedConflictPublishesToPeerWhenOriginWriterFails(t *testing.T) {
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	})
	originConn, originEvents := newWorkspaceV2StreamClient(t, httpServer)
	peerConn, peerEvents := newWorkspaceV2StreamClient(t, httpServer)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	originClientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	peerClientID := dto.WorkspaceUUID("20000000-0000-4000-8000-000000000031")
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000156")
	mutation := workspaceV2TestMkdirMutation(workspaceID, originClientID, operationID, "notes/conflict-after-write-failure.md")
	conflict := workspaceV2TestConflictCreated(workspaceID, "40000000-0000-4000-8000-000000000156", operationID, originClientID)
	conflictID := conflict.ConflictID
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		applyMutation: func(context.Context, int64, dto.WorkspaceMutation) (*service.WorkspaceMutationOutcome, error) {
			return &service.WorkspaceMutationOutcome{
				Rejected: &dto.WorkspaceMutationRejectedMessage{WorkspaceID: workspaceID, ClientID: originClientID, OperationID: operationID, Reason: dto.WorkspaceMutationRejectConflictCreated, ConflictID: &conflictID},
				Conflict: conflict,
			}, nil
		},
	}
	workspaceV2Hello(t, originConn, originEvents, "10000000-0000-4000-8000-000000000157")
	workspaceV2Send(t, peerConn, dto.WorkspaceActionHello, "10000000-0000-4000-8000-000000000158", workspaceV2HelloData(string(peerClientID)))
	require.True(t, workspaceV2ResponseStatus(t, workspaceV2Receive(t, peerEvents)))
	workspaceV2Send(t, originConn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000159", workspaceV2SubscribeData(string(workspaceID), string(originClientID), 0))
	workspaceV2Send(t, peerConn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000160", workspaceV2SubscribeData(string(workspaceID), string(peerClientID), 0))
	for range 3 {
		workspaceV2Receive(t, originEvents)
		workspaceV2Receive(t, peerEvents)
	}
	origin := workspaceV2FindConnection(t, server, originClientID)
	origin.writeMessage = func(opcode gws.Opcode, data []byte) error {
		if opcode == gws.OpcodeText && workspaceV2Action(data) == string(dto.WorkspaceActionMutationRejected) {
			return errors.New("simulated origin response writer failure after commit")
		}
		return origin.conn.WriteMessage(opcode, data)
	}
	workspaceV2Send(t, originConn, dto.WorkspaceActionMutation, "10000000-0000-4000-8000-000000000161", mutation)
	select {
	case frame := <-peerEvents.messages:
		require.Equal(t, string(dto.WorkspaceActionConflictCreated), workspaceV2Action(frame))
	case <-time.After(2 * time.Second):
		t.Fatal("peer did not receive committed conflict after origin response write failure")
	}
}

func TestWorkspaceV2CommittedResolutionPublishesToPeerWhenOriginWriterFails(t *testing.T) {
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	})
	originConn, originEvents := newWorkspaceV2StreamClient(t, httpServer)
	peerConn, peerEvents := newWorkspaceV2StreamClient(t, httpServer)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	originClientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	peerClientID := dto.WorkspaceUUID("20000000-0000-4000-8000-000000000032")
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000162")
	conflictID := dto.WorkspaceUUID("40000000-0000-4000-8000-000000000162")
	conflict := workspaceV2TestConflictCreated(workspaceID, conflictID, operationID, originClientID)
	resolved := &dto.WorkspaceConflictResolvedMessage{
		WorkspaceID: workspaceID, ConflictID: conflictID, ConflictRevision: conflict.ConflictRevision, OperationID: operationID,
		Revision: 4, Choice: dto.WorkspaceConflictDelete,
		PathState:          dto.WorkspacePathState{Path: conflict.Path, PathRevision: 4, Kind: dto.WorkspaceEntryTombstone, ContentHash: dto.WorkspaceNullableHash{Present: true}, Tombstone: true},
		ResolvedByClientID: originClientID,
	}
	request := dto.WorkspaceConflictResolvedRequest{
		WorkspaceID: workspaceID, ClientID: originClientID, OperationID: operationID, ConflictID: conflictID,
		ConflictRevision: conflict.ConflictRevision, Choice: dto.WorkspaceConflictDelete, Path: conflict.Path,
		ContentHash: dto.WorkspaceNullableHash{Present: true}, Metadata: dto.WorkspaceFileMetadata{},
	}
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		resolveConflict: func(context.Context, int64, dto.WorkspaceConflictResolvedRequest) (*service.WorkspaceResolveOutcome, error) {
			return &service.WorkspaceResolveOutcome{Resolved: resolved}, nil
		},
	}
	workspaceV2Hello(t, originConn, originEvents, "10000000-0000-4000-8000-000000000163")
	workspaceV2Send(t, peerConn, dto.WorkspaceActionHello, "10000000-0000-4000-8000-000000000164", workspaceV2HelloData(string(peerClientID)))
	require.True(t, workspaceV2ResponseStatus(t, workspaceV2Receive(t, peerEvents)))
	workspaceV2Send(t, originConn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000165", workspaceV2SubscribeData(string(workspaceID), string(originClientID), 0))
	workspaceV2Send(t, peerConn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000166", workspaceV2SubscribeData(string(workspaceID), string(peerClientID), 0))
	for range 3 {
		workspaceV2Receive(t, originEvents)
		workspaceV2Receive(t, peerEvents)
	}
	origin := workspaceV2FindConnection(t, server, originClientID)
	origin.writeMessage = func(opcode gws.Opcode, data []byte) error {
		if opcode == gws.OpcodeText && workspaceV2Action(data) == string(dto.WorkspaceActionConflictResolved) {
			return errors.New("simulated origin response writer failure after commit")
		}
		return origin.conn.WriteMessage(opcode, data)
	}
	workspaceV2Send(t, originConn, dto.WorkspaceActionConflictResolved, "10000000-0000-4000-8000-000000000167", request)
	select {
	case frame := <-peerEvents.messages:
		require.Equal(t, string(dto.WorkspaceActionConflictResolved), workspaceV2Action(frame))
	case <-time.After(2 * time.Second):
		t.Fatal("peer did not receive committed resolution after origin response write failure")
	}
}

func TestWorkspaceV2MutationConflictResponsePrecedesCreatedPush(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000054")
	mutation := workspaceV2TestMkdirMutation(workspaceID, clientID, operationID, "notes/conflict.md")
	conflict := workspaceV2TestConflictCreated(workspaceID, "40000000-0000-4000-8000-000000000054", operationID, clientID)
	conflictID := conflict.ConflictID
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		applyMutation: func(context.Context, int64, dto.WorkspaceMutation) (*service.WorkspaceMutationOutcome, error) {
			return &service.WorkspaceMutationOutcome{
				Rejected: &dto.WorkspaceMutationRejectedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Reason: dto.WorkspaceMutationRejectConflictCreated, ConflictID: &conflictID},
				Conflict: conflict,
			}, nil
		},
	}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000055")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000056", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}
	workspaceV2Send(t, conn, dto.WorkspaceActionMutation, "10000000-0000-4000-8000-000000000057", mutation)
	rejected := workspaceV2Receive(t, events)
	created := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionMutationRejected), workspaceV2Action(rejected))
	require.Equal(t, string(dto.WorkspaceActionConflictCreated), workspaceV2Action(created))
}

func TestWorkspaceV2MutationBlobRequiredResponsePrecedesUploadNeedPush(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000058")
	hash := dto.WorkspaceContentHash("blake3:0000000000000000000000000000000000000000000000000000000000000000")
	mutation := dto.WorkspaceMutation{
		WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Path: "notes/blob.md",
		Kind: dto.WorkspaceMutationUpsertFile, ContentHash: dto.WorkspaceNullableHash{Present: true, Value: &hash},
		Metadata: dto.WorkspaceFileMetadata{Size: 5},
	}
	rejected := &dto.WorkspaceMutationRejectedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Reason: dto.WorkspaceMutationRejectBlobRequired, RequiredHash: &hash}
	need := &dto.WorkspaceBlobNeedUploadPush{WorkspaceID: workspaceID, Direction: dto.WorkspaceBlobUpload, OperationID: operationID, ContentHash: hash, Size: 5}
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		applyMutation: func(context.Context, int64, dto.WorkspaceMutation) (*service.WorkspaceMutationOutcome, error) {
			return &service.WorkspaceMutationOutcome{Rejected: rejected, RequiredUpload: need}, nil
		},
	}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000059")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000060", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}
	workspaceV2Send(t, conn, dto.WorkspaceActionMutation, "10000000-0000-4000-8000-000000000061", mutation)
	rejectedFrame := workspaceV2Receive(t, events)
	needFrame := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionMutationRejected), workspaceV2Action(rejectedFrame))
	require.Equal(t, string(dto.WorkspaceActionBlobNeed), workspaceV2Action(needFrame))
}

func TestWorkspaceV2AckReturnsCorrelatedResponse(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet(), acknowledge: func(context.Context, int64, dto.WorkspaceAckRequest, dto.WorkspaceRevision) error { return nil }}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000058")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000059", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}
	ack := dto.WorkspaceAckRequest{WorkspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), ClientID: dto.WorkspaceUUID(workspaceV2StreamClientID), Revision: 3}
	workspaceV2Send(t, conn, dto.WorkspaceActionAck, "10000000-0000-4000-8000-000000000060", ack)
	response := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionAck), workspaceV2Action(response))
	require.True(t, workspaceV2ResponseStatus(t, response))
}

func TestWorkspaceV2AckAcceptsRevisionSeenOnlyFromLivePush(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		acknowledge: func(context.Context, int64, dto.WorkspaceAckRequest, dto.WorkspaceRevision) error {
			return nil
		},
	}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000081")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000082", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}

	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000081")
	mutation := workspaceV2TestMkdirMutation(workspaceID, clientID, operationID, "notes/live-ack.md")
	revision := dto.WorkspaceRevision(4)
	state := dto.WorkspacePathState{Path: mutation.Path, PathRevision: revision, Kind: dto.WorkspaceEntryDirectory, ContentHash: dto.WorkspaceNullableHash{Present: true}}
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{
		kind: workspaceV2LiveEvent, treeRevision: &revision, mutation: &mutation,
		accepted: &dto.WorkspaceMutationAcceptedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Revision: revision, PathState: state},
	})
	live := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionEvent), workspaceV2Action(live))

	ack := dto.WorkspaceAckRequest{WorkspaceID: workspaceID, ClientID: clientID, Revision: revision}
	workspaceV2Send(t, conn, dto.WorkspaceActionAck, "10000000-0000-4000-8000-000000000083", ack)
	response := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionAck), workspaceV2Action(response))
	require.True(t, workspaceV2ResponseStatus(t, response))
}

func TestWorkspaceV2MutationRequiresCurrentSubscription(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	called := 0
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		applyMutation: func(context.Context, int64, dto.WorkspaceMutation) (*service.WorkspaceMutationOutcome, error) {
			called++
			return nil, errors.New("mutation must not reach service")
		},
	}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000084")
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	mutation := workspaceV2TestMkdirMutation(workspaceID, dto.WorkspaceUUID(workspaceV2StreamClientID), "30000000-0000-4000-8000-000000000084", "notes/not-subscribed.md")
	workspaceV2Send(t, conn, dto.WorkspaceActionMutation, "10000000-0000-4000-8000-000000000085", mutation)
	response := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionMutation), workspaceV2Action(response))
	require.False(t, workspaceV2ResponseStatus(t, response))
	require.Equal(t, dto.WorkspaceErrorInvalidRequest, workspaceV2ErrorCode(t, response))
	require.Zero(t, called)
}

func TestWorkspaceV2MutationBroadcastsConflictToEverySubscriber(t *testing.T) {
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	})
	conn1, events1 := newWorkspaceV2StreamClient(t, httpServer)
	conn2, events2 := newWorkspaceV2StreamClient(t, httpServer)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000086")
	conflict := workspaceV2TestConflictCreated(workspaceID, "40000000-0000-4000-8000-000000000086", operationID, clientID)
	conflictID := conflict.ConflictID
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		applyMutation: func(context.Context, int64, dto.WorkspaceMutation) (*service.WorkspaceMutationOutcome, error) {
			return &service.WorkspaceMutationOutcome{
				Rejected: &dto.WorkspaceMutationRejectedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Reason: dto.WorkspaceMutationRejectConflictCreated, ConflictID: &conflictID},
				Conflict: conflict,
			}, nil
		},
	}
	workspaceV2Hello(t, conn1, events1, "10000000-0000-4000-8000-000000000087")
	workspaceV2Hello(t, conn2, events2, "10000000-0000-4000-8000-000000000088")
	workspaceV2Send(t, conn1, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000089", workspaceV2SubscribeData(string(workspaceID), workspaceV2StreamClientID, 0))
	workspaceV2Send(t, conn2, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000090", workspaceV2SubscribeData(string(workspaceID), workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events1)
		workspaceV2Receive(t, events2)
	}
	workspaceV2Send(t, conn1, dto.WorkspaceActionMutation, "10000000-0000-4000-8000-000000000091", workspaceV2TestMkdirMutation(workspaceID, clientID, operationID, "notes/broadcast-conflict.md"))
	rejected := workspaceV2Receive(t, events1)
	require.Equal(t, string(dto.WorkspaceActionMutationRejected), workspaceV2Action(rejected))
	conflict1 := workspaceV2Receive(t, events1)
	conflict2 := workspaceV2Receive(t, events2)
	require.Equal(t, string(dto.WorkspaceActionConflictCreated), workspaceV2Action(conflict1))
	require.Equal(t, string(dto.WorkspaceActionConflictCreated), workspaceV2Action(conflict2))
}

func TestWorkspaceV2MutationSendsBlobNeedOnlyToInitiator(t *testing.T) {
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	})
	conn1, events1 := newWorkspaceV2StreamClient(t, httpServer)
	conn2, events2 := newWorkspaceV2StreamClient(t, httpServer)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000095")
	hash := dto.WorkspaceContentHash("blake3:0000000000000000000000000000000000000000000000000000000000000000")
	need := &dto.WorkspaceBlobNeedUploadPush{WorkspaceID: workspaceID, Direction: dto.WorkspaceBlobUpload, OperationID: operationID, ContentHash: hash, Size: 5}
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		applyMutation: func(context.Context, int64, dto.WorkspaceMutation) (*service.WorkspaceMutationOutcome, error) {
			return &service.WorkspaceMutationOutcome{
				Rejected:       &dto.WorkspaceMutationRejectedMessage{WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Reason: dto.WorkspaceMutationRejectBlobRequired, RequiredHash: &hash},
				RequiredUpload: need,
			}, nil
		},
	}
	observerClientID := "20000000-0000-4000-8000-000000000020"
	workspaceV2Hello(t, conn1, events1, "10000000-0000-4000-8000-000000000096")
	workspaceV2Send(t, conn2, dto.WorkspaceActionHello, "10000000-0000-4000-8000-000000000097", workspaceV2HelloData(observerClientID))
	require.True(t, workspaceV2ResponseStatus(t, workspaceV2Receive(t, events2)))
	workspaceV2Send(t, conn1, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000098", workspaceV2SubscribeData(string(workspaceID), string(clientID), 0))
	workspaceV2Send(t, conn2, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000099", workspaceV2SubscribeData(string(workspaceID), observerClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events1)
		workspaceV2Receive(t, events2)
	}
	workspaceV2Send(t, conn1, dto.WorkspaceActionMutation, "10000000-0000-4000-8000-000000000100", workspaceV2TestMkdirMutation(workspaceID, clientID, operationID, "notes/broadcast-blob.md"))
	rejected := workspaceV2Receive(t, events1)
	require.Equal(t, string(dto.WorkspaceActionMutationRejected), workspaceV2Action(rejected))
	need1 := workspaceV2Receive(t, events1)
	require.Equal(t, string(dto.WorkspaceActionBlobNeed), workspaceV2Action(need1))
	select {
	case unexpected := <-events2.messages:
		t.Fatalf("operation-scoped BlobNeed leaked to another subscriber as %s", workspaceV2Action(unexpected))
	case <-time.After(100 * time.Millisecond):
	}
}

func TestWorkspaceV2UnmappedServiceErrorIsObservable(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	core, logs := observer.New(zap.ErrorLevel)
	server.logger = zap.New(core)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000105")
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		applyMutation: func(context.Context, int64, dto.WorkspaceMutation) (*service.WorkspaceMutationOutcome, error) {
			return nil, errors.New("tombstoned descendant collision")
		},
	}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000106")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000107", workspaceV2SubscribeData(string(workspaceID), string(clientID), 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}
	requestID := "10000000-0000-4000-8000-000000000108"
	workspaceV2Send(t, conn, dto.WorkspaceActionMutation, requestID, workspaceV2TestMkdirMutation(workspaceID, clientID, operationID, "notes/logged.md"))
	failure := workspaceV2Receive(t, events)
	require.False(t, workspaceV2ResponseStatus(t, failure))
	require.Equal(t, dto.WorkspaceErrorInternal, workspaceV2ErrorCode(t, failure))

	entries := logs.FilterMessage("workspace v2 service request failed").All()
	require.Len(t, entries, 1)
	fields := entries[0].ContextMap()
	require.Equal(t, string(dto.WorkspaceActionMutation), fields["action"])
	require.Equal(t, int64(41), fields["uid"])
	require.Equal(t, requestID, fields["requestId"])
	require.Equal(t, "tombstoned descendant collision", fields["error"])
}

func TestWorkspaceV2ResolveBlobNeedSendsOnlyToInitiator(t *testing.T) {
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	})
	conn1, events1 := newWorkspaceV2StreamClient(t, httpServer)
	conn2, events2 := newWorkspaceV2StreamClient(t, httpServer)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000110")
	conflict := workspaceV2TestConflictCreated(workspaceID, "40000000-0000-4000-8000-000000000110", operationID, clientID)
	hash := dto.WorkspaceContentHash("blake3:0000000000000000000000000000000000000000000000000000000000000000")
	need := &dto.WorkspaceBlobNeedUploadPush{WorkspaceID: workspaceID, Direction: dto.WorkspaceBlobUpload, OperationID: operationID, ContentHash: hash, Size: 5}
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		resolveConflict: func(context.Context, int64, dto.WorkspaceConflictResolvedRequest) (*service.WorkspaceResolveOutcome, error) {
			return nil, &service.WorkspaceServiceError{Code: dto.WorkspaceErrorBlobRequired, RequiredUpload: need}
		},
	}
	initiatorClientID := workspaceV2StreamClientID
	observerClientID := "20000000-0000-4000-8000-000000000020"
	workspaceV2Hello(t, conn1, events1, "10000000-0000-4000-8000-000000000111")
	workspaceV2Send(t, conn2, dto.WorkspaceActionHello, "10000000-0000-4000-8000-000000000112", workspaceV2HelloData(observerClientID))
	require.True(t, workspaceV2ResponseStatus(t, workspaceV2Receive(t, events2)))
	workspaceV2Send(t, conn1, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000113", workspaceV2SubscribeData(string(workspaceID), initiatorClientID, 0))
	workspaceV2Send(t, conn2, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000114", workspaceV2SubscribeData(string(workspaceID), observerClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events1)
		workspaceV2Receive(t, events2)
	}
	resolve := dto.WorkspaceConflictResolvedRequest{
		WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, ConflictID: conflict.ConflictID,
		ConflictRevision: conflict.ConflictRevision, Choice: dto.WorkspaceConflictDelete, Path: conflict.Path,
		ContentHash: dto.WorkspaceNullableHash{Present: true}, Metadata: dto.WorkspaceFileMetadata{},
	}
	workspaceV2Send(t, conn1, dto.WorkspaceActionConflictResolved, "10000000-0000-4000-8000-000000000115", resolve)
	failure := workspaceV2Receive(t, events1)
	require.Equal(t, string(dto.WorkspaceActionConflictResolved), workspaceV2Action(failure))
	require.False(t, workspaceV2ResponseStatus(t, failure))
	need1 := workspaceV2Receive(t, events1)
	require.Equal(t, string(dto.WorkspaceActionBlobNeed), workspaceV2Action(need1))
	select {
	case unexpected := <-events2.messages:
		t.Fatalf("operation-scoped resolve BlobNeed leaked to another subscriber as %s", workspaceV2Action(unexpected))
	case <-time.After(100 * time.Millisecond):
	}
}

func TestWorkspaceV2StaleResolutionBroadcastsRefreshedConflictGeneration(t *testing.T) {
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	})
	conn1, events1 := newWorkspaceV2StreamClient(t, httpServer)
	conn2, events2 := newWorkspaceV2StreamClient(t, httpServer)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000116")
	old := workspaceV2TestConflictCreated(workspaceID, "40000000-0000-4000-8000-000000000116", operationID, clientID)
	refreshed := *old
	refreshed.ConflictRevision, _ = dto.ParseWorkspaceConflictRevision("2")
	refreshed.Current.PathRevision++
	require.NoError(t, refreshed.Validate())
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		resolveConflict: func(context.Context, int64, dto.WorkspaceConflictResolvedRequest) (*service.WorkspaceResolveOutcome, error) {
			return nil, &service.WorkspaceServiceError{
				Code:              dto.WorkspaceErrorConflictRevisionStale,
				RefreshedConflict: &refreshed,
			}
		},
	}
	observerClientID := "20000000-0000-4000-8000-000000000021"
	workspaceV2Hello(t, conn1, events1, "10000000-0000-4000-8000-000000000117")
	workspaceV2Send(t, conn2, dto.WorkspaceActionHello, "10000000-0000-4000-8000-000000000118", workspaceV2HelloData(observerClientID))
	require.True(t, workspaceV2ResponseStatus(t, workspaceV2Receive(t, events2)))
	workspaceV2Send(t, conn1, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000119", workspaceV2SubscribeData(string(workspaceID), string(clientID), 0))
	workspaceV2Send(t, conn2, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000120", workspaceV2SubscribeData(string(workspaceID), observerClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events1)
		workspaceV2Receive(t, events2)
	}
	resolve := dto.WorkspaceConflictResolvedRequest{
		WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, ConflictID: old.ConflictID,
		ConflictRevision: old.ConflictRevision, Choice: dto.WorkspaceConflictDelete, Path: old.Path,
		ContentHash: dto.WorkspaceNullableHash{Present: true}, Metadata: dto.WorkspaceFileMetadata{},
	}
	workspaceV2Send(t, conn1, dto.WorkspaceActionConflictResolved, "10000000-0000-4000-8000-000000000121", resolve)
	failure := workspaceV2Receive(t, events1)
	require.Equal(t, dto.WorkspaceErrorConflictRevisionStale, workspaceV2ErrorCode(t, failure))
	for _, events := range []*workspaceV2TestEvents{events1, events2} {
		push := workspaceV2Receive(t, events)
		require.Equal(t, string(dto.WorkspaceActionConflictCreated), workspaceV2Action(push))
		var body struct {
			Data dto.WorkspaceConflictCreatedMessage `json:"data"`
		}
		require.NoError(t, json.Unmarshal(workspaceV2Payload(push), &body))
		require.Equal(t, refreshed, body.Data)
	}
}

func TestWorkspaceV2SnapshotDeduplicatesBufferedAuthoritativeConflict(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	started := make(chan struct{})
	release := make(chan struct{})
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000092")
	conflict := workspaceV2TestConflictCreated(workspaceID, "40000000-0000-4000-8000-000000000092", operationID, clientID)
	server.syncService = &workspaceV2StreamService{
		changeSet:        &domain.WorkspaceChangeSet{Mode: dto.WorkspaceSnapshotIncremental, FromRevision: 3, FinalRevision: 3, ConflictCount: 1, PendingConflicts: &workspaceV2StreamCursor{items: []*dto.WorkspaceConflictCreatedMessage{conflict}}},
		subscribeStarted: started, subscribeRelease: release,
	}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000093")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000094", workspaceV2SubscribeData(string(workspaceID), workspaceV2StreamClientID, 3))
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("subscribe did not enter service")
	}
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{kind: workspaceV2LiveConflictCreated, conflictID: conflict.ConflictID, conflict: conflict})
	close(release)
	require.Equal(t, string(dto.WorkspaceActionSnapshotBegin), workspaceV2Action(workspaceV2Receive(t, events)))
	require.Equal(t, string(dto.WorkspaceActionConflictCreated), workspaceV2Action(workspaceV2Receive(t, events)))
	require.Equal(t, string(dto.WorkspaceActionSnapshotEnd), workspaceV2Action(workspaceV2Receive(t, events)))
	select {
	case extra := <-events.messages:
		t.Fatalf("authoritative conflict was duplicated as %s", workspaceV2Action(extra))
	case <-time.After(100 * time.Millisecond):
	}
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{kind: workspaceV2LiveConflictCreated, conflictID: conflict.ConflictID, conflict: conflict})
	select {
	case extra := <-events.messages:
		t.Fatalf("late authoritative conflict was duplicated as %s", workspaceV2Action(extra))
	case <-time.After(100 * time.Millisecond):
	}
	refreshed := *conflict
	refreshedRevision, err := dto.ParseWorkspaceConflictRevision("2")
	require.NoError(t, err)
	refreshed.ConflictRevision = refreshedRevision
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{
		kind: workspaceV2LiveConflictCreated, conflictID: refreshed.ConflictID, conflict: &refreshed,
	})
	refreshFrame := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionConflictCreated), workspaceV2Action(refreshFrame))
	var refreshEnvelope dto.WorkspaceV2Response[dto.WorkspaceConflictCreatedMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(refreshFrame), &refreshEnvelope))
	require.Equal(t, refreshed.ConflictRevision, refreshEnvelope.Data.ConflictRevision)
	server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{
		kind: workspaceV2LiveConflictCreated, conflictID: refreshed.ConflictID, conflict: &refreshed,
	})
	select {
	case extra := <-events.messages:
		t.Fatalf("same authoritative conflict revision was duplicated as %s", workspaceV2Action(extra))
	case <-time.After(100 * time.Millisecond):
	}
}

func TestWorkspaceV2RefreshedPendingConflictDoesNotRollbackAcrossSnapshotBoundary(t *testing.T) {
	tests := []struct {
		name       string
		arrivalIDs []int
	}{
		{name: "new_then_old", arrivalIDs: []int{1, 0}},
		{name: "old_then_new", arrivalIDs: []int{0, 1}},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			server, conn, events := newWorkspaceV2StreamConnection(t, nil)
			started := make(chan struct{})
			release := make(chan struct{})
			workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
			clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
			operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000170")
			oldGeneration := *workspaceV2TestConflictCreated(workspaceID, "40000000-0000-4000-8000-000000000170", operationID, clientID)
			oldRevision, err := dto.ParseWorkspaceConflictRevision("9999999999999999999")
			require.NoError(t, err)
			oldGeneration.ConflictRevision = oldRevision
			newGeneration := oldGeneration
			newRevision, err := dto.ParseWorkspaceConflictRevision("1")
			require.NoError(t, err)
			newGeneration.ConflictRevision = newRevision
			server.syncService = &workspaceV2AuthoritativeStreamService{
				workspaceV2StreamService: &workspaceV2StreamService{
					changeSet:        workspaceV2FullChangeSet(),
					subscribeStarted: started,
					subscribeRelease: release,
				},
				pendingConflict: &newGeneration,
			}
			workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000171")
			workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000172", workspaceV2SubscribeData(string(workspaceID), string(clientID), 0))
			select {
			case <-started:
			case <-time.After(time.Second):
				t.Fatal("subscribe did not reach snapshot boundary")
			}
			generations := []*dto.WorkspaceConflictCreatedMessage{&oldGeneration, &newGeneration}
			for _, index := range test.arrivalIDs {
				generation := generations[index]
				server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{
					kind: workspaceV2LiveConflictCreated, conflictID: generation.ConflictID, conflict: generation,
				})
			}
			close(release)
			for range 3 {
				workspaceV2Receive(t, events)
			}
			created := workspaceV2Receive(t, events)
			require.Equal(t, string(dto.WorkspaceActionConflictCreated), workspaceV2Action(created))
			var envelope dto.WorkspaceV2Response[dto.WorkspaceConflictCreatedMessage]
			require.NoError(t, json.Unmarshal(workspaceV2Payload(created), &envelope))
			require.Equal(t, newRevision, envelope.Data.ConflictRevision)

			// A delayed stale publisher must not re-send or roll back the current row.
			server.hub.publish(workspaceV2HubKey{uid: 41, workspaceID: workspaceID}, workspaceV2LiveNotification{
				kind: workspaceV2LiveConflictCreated, conflictID: oldGeneration.ConflictID, conflict: &oldGeneration,
			})
			select {
			case extra := <-events.messages:
				t.Fatalf("stale generation was delivered after authoritative refresh as %s", workspaceV2Action(extra))
			case <-time.After(100 * time.Millisecond):
			}
		})
	}
}

func TestWorkspaceV2ResolveResponsePrecedesResolutionPush(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)
	clientID := dto.WorkspaceUUID(workspaceV2StreamClientID)
	operationID := dto.WorkspaceUUID("30000000-0000-4000-8000-000000000061")
	conflictID := dto.WorkspaceUUID("40000000-0000-4000-8000-000000000061")
	conflict := workspaceV2TestConflictCreated(workspaceID, conflictID, operationID, clientID)
	resolved := &dto.WorkspaceConflictResolvedMessage{
		WorkspaceID: workspaceID, ConflictID: conflictID, ConflictRevision: conflict.ConflictRevision, OperationID: operationID,
		Revision: 4, Choice: dto.WorkspaceConflictDelete,
		PathState:          dto.WorkspacePathState{Path: "notes/live.md", PathRevision: 4, Kind: dto.WorkspaceEntryTombstone, ContentHash: dto.WorkspaceNullableHash{Present: true}, Tombstone: true},
		ResolvedByClientID: clientID,
	}
	server.syncService = &workspaceV2StreamService{
		changeSet: workspaceV2FullChangeSet(),
		resolveConflict: func(context.Context, int64, dto.WorkspaceConflictResolvedRequest) (*service.WorkspaceResolveOutcome, error) {
			return &service.WorkspaceResolveOutcome{Resolved: resolved}, nil
		},
	}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000062")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000063", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}
	resolve := dto.WorkspaceConflictResolvedRequest{
		WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, ConflictID: conflictID,
		ConflictRevision: conflict.ConflictRevision, Choice: dto.WorkspaceConflictDelete, Path: "notes/live.md",
		ContentHash: dto.WorkspaceNullableHash{Present: true}, Metadata: dto.WorkspaceFileMetadata{},
	}
	workspaceV2Send(t, conn, dto.WorkspaceActionConflictResolved, "10000000-0000-4000-8000-000000000064", resolve)
	response := workspaceV2Receive(t, events)
	push := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionConflictResolved), workspaceV2Action(response))
	require.Equal(t, string(dto.WorkspaceActionConflictResolved), workspaceV2Action(push))
	var responseEnvelope dto.WorkspaceV2Response[dto.WorkspaceConflictResolvedMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(response), &responseEnvelope))
	require.NotNil(t, responseEnvelope.RequestID)
	var pushEnvelope dto.WorkspaceV2Response[dto.WorkspaceConflictResolvedMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(push), &pushEnvelope))
	require.Nil(t, pushEnvelope.RequestID)
}

func TestWorkspaceV2UploadStreamsBinaryChunksAndCompletesBlobEnd(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	blobStore := &workspaceV2BlobStoreStub{}
	server.blobStore = blobStore
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000065")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000066", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}

	payload := []byte("hello")
	hash := workspaceV2TestBlobHash(payload)
	transferID := "60000000-0000-4000-8000-000000000065"
	begin := dto.WorkspaceBlobBeginMessage{WorkspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), TransferID: dto.WorkspaceUUID(transferID), Direction: dto.WorkspaceBlobUpload, ContentHash: hash, Size: uint64(len(payload)), ChunkSize: dto.WorkspaceBlobChunkSize, ChunkCount: 1}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000067", begin)
	require.Equal(t, string(dto.WorkspaceActionBlobBegin), workspaceV2Action(workspaceV2Receive(t, events)))
	fullDigest, chunkDigest := dto.ComputeWorkspaceBlobDigest(payload)
	transferUUID := uuid.MustParse(transferID)
	header, err := dto.MarshalWorkspaceBlobHeader(dto.WorkspaceBlobHeader{Direction: dto.WorkspaceBlobUpload, Final: true, TransferID: transferUUID, PayloadLen: uint32(len(payload)), ChunkDigest: chunkDigest})
	require.NoError(t, err)
	frame := append(header[:], payload...)
	require.NoError(t, conn.WriteMessage(gws.OpcodeBinary, frame))
	_ = fullDigest
	end := dto.WorkspaceBlobEndMessage{WorkspaceID: begin.WorkspaceID, TransferID: begin.TransferID, Direction: begin.Direction, ContentHash: begin.ContentHash, Size: begin.Size, ChunkCount: 1}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, "10000000-0000-4000-8000-000000000068", end)
	require.Equal(t, string(dto.WorkspaceActionBlobEnd), workspaceV2Action(workspaceV2Receive(t, events)))
	blobStore.mu.Lock()
	require.Equal(t, [][]byte{payload}, blobStore.puts)
	blobStore.mu.Unlock()
}

func TestWorkspaceV2BinaryChunkFailureReportsBlobEndWithoutClosingConnection(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.blobStore = &workspaceV2BlobStoreStub{}
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000101")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000102", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}

	payload := []byte("hello")
	hash := workspaceV2TestBlobHash(payload)
	transferID := dto.WorkspaceUUID("60000000-0000-4000-8000-000000000101")
	begin := dto.WorkspaceBlobBeginMessage{WorkspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), TransferID: transferID, Direction: dto.WorkspaceBlobUpload, ContentHash: hash, Size: uint64(len(payload)), ChunkSize: dto.WorkspaceBlobChunkSize, ChunkCount: 1}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000103", begin)
	require.Equal(t, string(dto.WorkspaceActionBlobBegin), workspaceV2Action(workspaceV2Receive(t, events)))
	header, err := dto.MarshalWorkspaceBlobHeader(dto.WorkspaceBlobHeader{Direction: dto.WorkspaceBlobUpload, Final: true, TransferID: uuid.MustParse(string(transferID)), PayloadLen: uint32(len(payload))})
	require.NoError(t, err)
	require.NoError(t, conn.WriteMessage(gws.OpcodeBinary, append(header[:], payload...)))

	failure := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionBlobEnd), workspaceV2Action(failure))
	require.False(t, workspaceV2ResponseStatus(t, failure))
	require.Equal(t, dto.WorkspaceErrorBlobHashMismatch, workspaceV2ErrorCode(t, failure))
	var envelope dto.WorkspaceV2Response[struct{}]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(failure), &envelope))
	require.Nil(t, envelope.RequestID)
	select {
	case err := <-events.closes:
		t.Fatalf("binary transfer failure closed the connection: %v", err)
	case <-time.After(100 * time.Millisecond):
	}
}

func TestWorkspaceV2ExpiredTransferReportsBlobEndFailure(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.blobStore = &workspaceV2BlobStoreStub{}
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000104")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000105", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}
	payload := []byte("hello")
	hash := workspaceV2TestBlobHash(payload)
	transferID := dto.WorkspaceUUID("60000000-0000-4000-8000-000000000104")
	begin := dto.WorkspaceBlobBeginMessage{WorkspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), TransferID: transferID, Direction: dto.WorkspaceBlobUpload, ContentHash: hash, Size: uint64(len(payload)), ChunkSize: dto.WorkspaceBlobChunkSize, ChunkCount: 1}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000106", begin)
	require.Equal(t, string(dto.WorkspaceActionBlobBegin), workspaceV2Action(workspaceV2Receive(t, events)))
	server.mu.RLock()
	var owner *workspaceV2Connection
	for _, candidate := range server.connections {
		owner = candidate
		break
	}
	server.mu.RUnlock()
	require.NotNil(t, owner)
	owner.stateMu.RLock()
	transfer := owner.transfers[uuid.MustParse(string(transferID))]
	owner.stateMu.RUnlock()
	require.NotNil(t, transfer)
	transfer.mu.Lock()
	transfer.lastActivity = time.Now().Add(-workspaceV2TransferIdleExpiry)
	transfer.mu.Unlock()
	server.transfers.Expire(time.Now())
	failure := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionBlobEnd), workspaceV2Action(failure))
	require.False(t, workspaceV2ResponseStatus(t, failure))
	require.Equal(t, dto.WorkspaceErrorBlobTransferOutOfOrder, workspaceV2ErrorCode(t, failure))
	var envelope dto.WorkspaceV2Response[struct{}]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(failure), &envelope))
	require.Nil(t, envelope.RequestID)
}

func TestWorkspaceV2StructuralBinaryHeaderClosesConnection(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.blobStore = &workspaceV2BlobStoreStub{}
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000097")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000098", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}

	payload := []byte("hello")
	hash := workspaceV2TestBlobHash(payload)
	transferID := "60000000-0000-4000-8000-000000000097"
	begin := dto.WorkspaceBlobBeginMessage{WorkspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), TransferID: dto.WorkspaceUUID(transferID), Direction: dto.WorkspaceBlobUpload, ContentHash: hash, Size: uint64(len(payload)), ChunkSize: dto.WorkspaceBlobChunkSize, ChunkCount: 1}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobBegin, "10000000-0000-4000-8000-000000000099", begin)
	workspaceV2Receive(t, events)
	_, chunkDigest := dto.ComputeWorkspaceBlobDigest(payload)
	header, err := dto.MarshalWorkspaceBlobHeader(dto.WorkspaceBlobHeader{Direction: dto.WorkspaceBlobUpload, Final: true, TransferID: uuid.MustParse(transferID), PayloadLen: uint32(len(payload)), ChunkDigest: chunkDigest})
	require.NoError(t, err)
	header[0] = 'X'
	require.NoError(t, conn.WriteMessage(gws.OpcodeBinary, append(header[:], payload...)))
	select {
	case <-events.closes:
	case <-time.After(time.Second):
		t.Fatal("structural binary framing error did not close the connection")
	}
}

func TestWorkspaceV2DownloadStreamsBinaryChunksAndAcknowledgesBlobEnd(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	payload := []byte("hello")
	downloadClosed := make(chan struct{})
	blobStore := &workspaceV2BlobStoreStub{download: payload, downloadClosed: downloadClosed}
	server.blobStore = blobStore
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000069")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000070", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}

	hash := workspaceV2TestBlobHash(payload)
	need := dto.WorkspaceBlobNeedDownloadRequest{WorkspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), Direction: dto.WorkspaceBlobDownload, ContentHash: hash, OperationID: dto.WorkspaceNullableUUID{Present: true}, Size: dto.WorkspaceNullableUint64{Present: true}}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobNeed, "10000000-0000-4000-8000-000000000071", need)
	require.Equal(t, string(dto.WorkspaceActionBlobNeed), workspaceV2Action(workspaceV2Receive(t, events)))
	beginFrame := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionBlobBegin), workspaceV2Action(beginFrame))
	binaryFrame := workspaceV2Receive(t, events)
	require.Equal(t, "FNS2", string(binaryFrame[:4]))
	endFrame := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionBlobEnd), workspaceV2Action(endFrame))
	var beginEnvelope dto.WorkspaceV2Response[dto.WorkspaceBlobBeginMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(beginFrame), &beginEnvelope))
	var endEnvelope dto.WorkspaceV2Response[dto.WorkspaceBlobEndMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(endFrame), &endEnvelope))
	require.NotNil(t, endEnvelope.Data)
	transferID, err := uuid.Parse(string(endEnvelope.Data.TransferID))
	require.NoError(t, err)
	owner := workspaceV2FindConnection(t, server, dto.WorkspaceUUID(workspaceV2StreamClientID))
	owner.stateMu.RLock()
	retainedTransfer := owner.transfers[transferID]
	connectionTransferCount := len(owner.transfers)
	owner.stateMu.RUnlock()
	require.NotNil(t, retainedTransfer)
	require.Equal(t, 1, connectionTransferCount)
	server.transfers.mu.Lock()
	_, transferActive := server.transfers.active[retainedTransfer]
	workspaceTransferCount := server.transfers.byWorkspace[workspaceV2HubKey{uid: owner.uid, workspaceID: endEnvelope.Data.WorkspaceID}]
	userTransferCount := server.transfers.byUser[owner.uid]
	activeTransferCount := len(server.transfers.active)
	server.transfers.mu.Unlock()
	require.True(t, transferActive)
	require.Equal(t, 1, workspaceTransferCount)
	require.Equal(t, 1, userTransferCount)
	require.Equal(t, 1, activeTransferCount)
	select {
	case <-downloadClosed:
		t.Fatal("download transfer closed before BlobEnd acknowledgement")
	default:
	}

	ack := endEnvelope.Data
	ackBytes, err := json.Marshal(ack)
	require.NoError(t, err)
	ackRequestID := dto.WorkspaceUUID("10000000-0000-4000-8000-000000000072")
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobEnd, string(ackRequestID), *ack)
	ackFrame := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionBlobEnd), workspaceV2Action(ackFrame))
	var ackEnvelope dto.WorkspaceV2Response[dto.WorkspaceBlobEndMessage]
	require.NoError(t, json.Unmarshal(workspaceV2Payload(ackFrame), &ackEnvelope))
	require.NotNil(t, ackEnvelope.RequestID)
	require.Equal(t, ackRequestID, *ackEnvelope.RequestID)
	require.True(t, ackEnvelope.Status)
	require.Nil(t, ackEnvelope.Error)
	require.NotNil(t, ackEnvelope.Data)
	ackResponseBytes, err := json.Marshal(ackEnvelope.Data)
	require.NoError(t, err)
	require.Equal(t, ackBytes, ackResponseBytes)
	select {
	case <-downloadClosed:
	case <-time.After(time.Second):
		t.Fatal("download transfer was not released after BlobEnd acknowledgement")
	}
	owner.stateMu.RLock()
	_, transferPresent := owner.transfers[transferID]
	connectionTransferCount = len(owner.transfers)
	owner.stateMu.RUnlock()
	server.transfers.mu.Lock()
	_, transferActive = server.transfers.active[retainedTransfer]
	workspaceTransferCount = server.transfers.byWorkspace[workspaceV2HubKey{uid: owner.uid, workspaceID: endEnvelope.Data.WorkspaceID}]
	userTransferCount = server.transfers.byUser[owner.uid]
	activeTransferCount = len(server.transfers.active)
	server.transfers.mu.Unlock()
	require.False(t, transferPresent)
	require.Zero(t, connectionTransferCount)
	require.False(t, transferActive)
	require.Zero(t, workspaceTransferCount)
	require.Zero(t, userTransferCount)
	require.Zero(t, activeTransferCount)
	require.Equal(t, dto.WorkspaceBlobDownload, beginEnvelope.Data.Direction)
}

func TestWorkspaceV2DownloadRejectsFullBlobHashMismatch(t *testing.T) {
	server, conn, events := newWorkspaceV2StreamConnection(t, nil)
	server.blobStore = &workspaceV2BlobStoreStub{download: []byte("tampered")}
	server.syncService = &workspaceV2StreamService{changeSet: workspaceV2FullChangeSet()}
	workspaceV2Hello(t, conn, events, "10000000-0000-4000-8000-000000000107")
	workspaceV2Send(t, conn, dto.WorkspaceActionSubscribe, "10000000-0000-4000-8000-000000000108", workspaceV2SubscribeData(workspaceV2SecurityWorkspaceID, workspaceV2StreamClientID, 0))
	for range 3 {
		workspaceV2Receive(t, events)
	}
	hash := workspaceV2TestBlobHash([]byte("hello"))
	need := dto.WorkspaceBlobNeedDownloadRequest{
		WorkspaceID: dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), Direction: dto.WorkspaceBlobDownload,
		ContentHash: hash, OperationID: dto.WorkspaceNullableUUID{Present: true}, Size: dto.WorkspaceNullableUint64{Present: true},
	}
	workspaceV2Send(t, conn, dto.WorkspaceActionBlobNeed, "10000000-0000-4000-8000-000000000109", need)
	require.Equal(t, string(dto.WorkspaceActionBlobNeed), workspaceV2Action(workspaceV2Receive(t, events)))
	require.Equal(t, string(dto.WorkspaceActionBlobBegin), workspaceV2Action(workspaceV2Receive(t, events)))
	failure := workspaceV2Receive(t, events)
	require.Equal(t, string(dto.WorkspaceActionBlobEnd), workspaceV2Action(failure))
	require.False(t, workspaceV2ResponseStatus(t, failure))
	require.Equal(t, dto.WorkspaceErrorBlobHashMismatch, workspaceV2ErrorCode(t, failure))
	select {
	case unexpected := <-events.messages:
		t.Fatalf("download exposed a binary payload before hash verification: %x", unexpected[:min(len(unexpected), 4)])
	case <-time.After(100 * time.Millisecond):
	}
}

type workspaceV2StreamService struct {
	service.WorkspaceSyncService
	mu               sync.Mutex
	changeSet        *domain.WorkspaceChangeSet
	subscribeCalls   int
	subscribeStarted chan struct{}
	subscribeRelease chan struct{}
	applyMutation    func(context.Context, int64, dto.WorkspaceMutation) (*service.WorkspaceMutationOutcome, error)
	acknowledge      func(context.Context, int64, dto.WorkspaceAckRequest, dto.WorkspaceRevision) error
	resolveConflict  func(context.Context, int64, dto.WorkspaceConflictResolvedRequest) (*service.WorkspaceResolveOutcome, error)
}

type workspaceV2BlobStoreStub struct {
	mu             sync.Mutex
	puts           [][]byte
	download       []byte
	downloadClosed chan struct{}
}

type workspaceV2BlobDownloadReader struct {
	io.Reader
	closed chan struct{}
	once   sync.Once
}

func (r *workspaceV2BlobDownloadReader) Close() error {
	r.once.Do(func() {
		if r.closed != nil {
			close(r.closed)
		}
	})
	return nil
}

func (s *workspaceV2BlobStoreStub) Has(context.Context, int64, dto.WorkspaceContentHash, uint64) (bool, error) {
	return true, nil
}

func (s *workspaceV2BlobStoreStub) Put(_ context.Context, _ int64, _ dto.WorkspaceContentHash, _ uint64, source io.Reader) error {
	data, err := io.ReadAll(source)
	if err != nil {
		return err
	}
	s.mu.Lock()
	s.puts = append(s.puts, data)
	s.mu.Unlock()
	return nil
}

func (s *workspaceV2BlobStoreStub) Open(context.Context, int64, dto.WorkspaceContentHash) (io.ReadCloser, uint64, error) {
	reader := &workspaceV2BlobDownloadReader{Reader: bytes.NewReader(s.download), closed: s.downloadClosed}
	return reader, uint64(len(s.download)), nil
}

func (s *workspaceV2BlobStoreStub) ReconcileAndGC(context.Context, int64, time.Time) error {
	return nil
}

func (s *workspaceV2StreamService) Subscribe(_ context.Context, _ int64, _ dto.WorkspaceSubscribeRequest) (*domain.WorkspaceChangeSet, error) {
	s.mu.Lock()
	s.subscribeCalls++
	started := s.subscribeStarted
	release := s.subscribeRelease
	s.mu.Unlock()
	if started != nil {
		close(started)
		<-release
	}
	return s.changeSet, nil
}

type workspaceV2AuthoritativeStreamService struct {
	*workspaceV2StreamService
	pendingConflict *dto.WorkspaceConflictCreatedMessage
}

func (s *workspaceV2AuthoritativeStreamService) CurrentPendingConflict(context.Context, int64, dto.WorkspaceUUID, dto.WorkspaceUUID) (*dto.WorkspaceConflictCreatedMessage, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.pendingConflict == nil {
		return nil, nil
	}
	copy := *s.pendingConflict
	return &copy, nil
}

func (s *workspaceV2StreamService) ApplyMutation(ctx context.Context, uid int64, mutation dto.WorkspaceMutation) (*service.WorkspaceMutationOutcome, error) {
	if s.applyMutation == nil {
		return nil, errors.New("apply mutation stub is not configured")
	}
	return s.applyMutation(ctx, uid, mutation)
}

func (s *workspaceV2StreamService) Acknowledge(ctx context.Context, uid int64, ack dto.WorkspaceAckRequest, lastDelivered dto.WorkspaceRevision) error {
	if s.acknowledge == nil {
		return errors.New("acknowledge stub is not configured")
	}
	return s.acknowledge(ctx, uid, ack, lastDelivered)
}

func (s *workspaceV2StreamService) ResolveConflict(ctx context.Context, uid int64, request dto.WorkspaceConflictResolvedRequest) (*service.WorkspaceResolveOutcome, error) {
	if s.resolveConflict == nil {
		return nil, errors.New("resolve conflict stub is not configured")
	}
	return s.resolveConflict(ctx, uid, request)
}

var _ service.WorkspaceSyncService = (*workspaceV2StreamService)(nil)

type workspaceV2StreamCursor struct {
	items []*dto.WorkspaceConflictCreatedMessage
	index int
}

func (c *workspaceV2StreamCursor) Next(context.Context) (*dto.WorkspaceConflictCreatedMessage, error) {
	if c.index >= len(c.items) {
		return nil, nil
	}
	item := c.items[c.index]
	c.index++
	return item, nil
}

func (c *workspaceV2StreamCursor) Close() error { return nil }

func workspaceV2FullChangeSet() *domain.WorkspaceChangeSet {
	return &domain.WorkspaceChangeSet{
		Mode: dto.WorkspaceSnapshotFull, FromRevision: 0, FinalRevision: 3,
		Entries: []dto.WorkspacePathState{{
			Path: "notes/a.md", PathRevision: 3, Kind: dto.WorkspaceEntryDirectory,
			ContentHash: dto.WorkspaceNullableHash{Present: true},
		}}, EntryCount: 1,
	}
}

func workspaceV2TestMkdirMutation(workspaceID, clientID, operationID dto.WorkspaceUUID, path dto.WorkspacePath) dto.WorkspaceMutation {
	return dto.WorkspaceMutation{
		WorkspaceID: workspaceID, ClientID: clientID, OperationID: operationID, Path: path,
		Kind: dto.WorkspaceMutationMkdir, ContentHash: dto.WorkspaceNullableHash{Present: true}, Metadata: dto.WorkspaceFileMetadata{},
	}
}

func workspaceV2TestBlobHash(payload []byte) dto.WorkspaceContentHash {
	full, _ := dto.ComputeWorkspaceBlobDigest(payload)
	return dto.WorkspaceContentHash("blake3:" + hex.EncodeToString(full[:]))
}

func workspaceV2TestConflictCreated(workspaceID, conflictID, operationID, clientID dto.WorkspaceUUID) *dto.WorkspaceConflictCreatedMessage {
	conflictRevision, err := dto.ParseWorkspaceConflictRevision("1")
	if err != nil {
		panic(err)
	}
	path := dto.WorkspacePath("notes/live.md")
	tombstone := dto.WorkspaceConflictSide{Path: nil, PathRevision: 0, ContentHash: dto.WorkspaceNullableHash{Present: true}, Tombstone: true}
	hash := dto.WorkspaceContentHash("blake3:0000000000000000000000000000000000000000000000000000000000000000")
	live := dto.WorkspaceConflictSide{Path: &path, PathRevision: 1, ContentHash: dto.WorkspaceNullableHash{Present: true, Value: &hash}, Metadata: dto.WorkspaceFileMetadata{Size: 1}}
	return &dto.WorkspaceConflictCreatedMessage{
		WorkspaceID: workspaceID, ConflictID: conflictID, ConflictRevision: conflictRevision, Path: path,
		Kind: dto.WorkspaceConflictDeleteModify, Ancestor: tombstone, Current: live, Incoming: tombstone,
		CreatedByOperationID: operationID,
	}
}

func workspaceV2FindConnection(t *testing.T, server *WorkspaceV2Server, clientID dto.WorkspaceUUID) *workspaceV2Connection {
	t.Helper()
	server.mu.RLock()
	defer server.mu.RUnlock()
	for _, connection := range server.connections {
		connection.stateMu.RLock()
		matches := connection.helloClientID == clientID
		connection.stateMu.RUnlock()
		if matches {
			return connection
		}
	}
	t.Fatalf("workspace v2 connection for client %s not found", clientID)
	return nil
}

func newWorkspaceV2StreamConnection(t *testing.T, _ *workspaceV2StreamService) (*WorkspaceV2Server, *gws.Conn, *workspaceV2TestEvents) {
	t.Helper()
	server, httpServer := newWorkspaceV2HTTPTestServer(t, func(*gin.Context) (*middleware.AuthenticatedUserToken, *code.Code) {
		return workspaceV2TestIdentity(), nil
	})
	conn, events := newWorkspaceV2StreamClient(t, httpServer)
	return server, conn, events
}

func newWorkspaceV2StreamClient(t *testing.T, httpServer *httptest.Server) (*gws.Conn, *workspaceV2TestEvents) {
	t.Helper()
	events := &workspaceV2TestEvents{messages: make(chan []byte, 32), closes: make(chan error, 1)}
	conn, response, err := gws.NewClient(events, &gws.ClientOption{
		Addr:             "ws" + strings.TrimPrefix(httpServer.URL, "http") + "/api/user/workspace-sync/v2",
		HandshakeTimeout: 3 * time.Second,
		RequestHeader:    http.Header{"Authorization": []string{"Bearer test-token"}},
	})
	require.NoError(t, err)
	require.Equal(t, http.StatusSwitchingProtocols, response.StatusCode)
	if response.Body != nil {
		response.Body.Close()
	}
	go conn.ReadLoop()
	t.Cleanup(func() { _ = conn.NetConn().Close() })
	return conn, events
}

func workspaceV2Hello(t *testing.T, conn *gws.Conn, events *workspaceV2TestEvents, requestID string) {
	t.Helper()
	workspaceV2Send(t, conn, dto.WorkspaceActionHello, requestID, workspaceV2HelloData(workspaceV2StreamClientID))
	require.True(t, workspaceV2ResponseStatus(t, workspaceV2Receive(t, events)))
}

func workspaceV2HelloData(clientID string) dto.WorkspaceHelloRequest {
	return dto.WorkspaceHelloRequest{ProtocolVersion: "2", ClientID: dto.WorkspaceUUID(clientID), ClientVersion: "test", Capabilities: []string{"binary_chunks", "conflicts", "snapshot_v1"}}
}

func workspaceV2SubscribeData(workspaceID, clientID string, revision dto.WorkspaceRevision) dto.WorkspaceSubscribeRequest {
	return dto.WorkspaceSubscribeRequest{WorkspaceID: dto.WorkspaceUUID(workspaceID), ClientID: dto.WorkspaceUUID(clientID), LastAckRevision: revision}
}

func workspaceV2Send(t *testing.T, conn *gws.Conn, action dto.WorkspaceV2Action, requestID string, data any) {
	t.Helper()
	rawData, err := json.Marshal(data)
	require.NoError(t, err)
	envelope, err := json.Marshal(struct {
		RequestID dto.WorkspaceUUID `json:"requestId"`
		Data      json.RawMessage   `json:"data"`
	}{RequestID: dto.WorkspaceUUID(requestID), Data: rawData})
	require.NoError(t, err)
	require.NoError(t, conn.WriteString(string(action)+"|"+string(envelope)))
}

func workspaceV2Receive(t *testing.T, events *workspaceV2TestEvents) []byte {
	t.Helper()
	select {
	case response := <-events.messages:
		return response
	case <-time.After(2 * time.Second):
		t.Fatal("timed out waiting for workspace v2 response")
		return nil
	}
}

func workspaceV2Action(raw []byte) string {
	index := strings.IndexByte(string(raw), '|')
	if index < 0 {
		return ""
	}
	return string(raw[:index])
}

func workspaceV2Payload(raw []byte) []byte {
	index := strings.IndexByte(string(raw), '|')
	if index < 0 {
		return nil
	}
	return raw[index+1:]
}

func workspaceV2ResponseStatus(t *testing.T, raw []byte) bool {
	t.Helper()
	var envelope struct {
		Status bool `json:"status"`
	}
	require.NoError(t, json.Unmarshal(workspaceV2Payload(raw), &envelope))
	return envelope.Status
}

func workspaceV2ErrorCode(t *testing.T, raw []byte) dto.WorkspaceV2ErrorCode {
	t.Helper()
	var envelope struct {
		Error *dto.WorkspaceV2Error `json:"error"`
	}
	require.NoError(t, json.Unmarshal(workspaceV2Payload(raw), &envelope))
	require.NotNil(t, envelope.Error)
	return envelope.Error.Code
}
