package websocket_router

import (
	"context"
	"errors"
	"io"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/lxzan/gws"
)

const (
	workspaceV2InboundQueueDepth         = 8
	workspaceV2OutboundQueueDepth        = 8
	workspaceV2LiveBacklogDepth          = 256
	workspaceV2MaxSeenTransfers          = 1024
	workspaceV2MaxTransfersPerConnection = 4
	workspaceV2MaxTransfersPerWorkspace  = 16
	workspaceV2MaxTransfersPerUser       = 32
	workspaceV2TransferIdleExpiry        = 60 * time.Second
	workspaceV2TransferMaxLifetime       = 30 * time.Minute
	workspaceV2Heartbeat                 = 25 * time.Second
	workspaceV2HeartbeatWait             = 60 * time.Second
	workspaceV2ConnectionLoopCount       = 4
)

var (
	errWorkspaceV2TransferAlreadyActive    = errors.New("workspace transfer identifier already active")
	errWorkspaceV2TransferIdentifierReused = errors.New("workspace transfer identifier reused")
)

type workspaceV2InboundFrame struct {
	opcode gws.Opcode
	data   []byte
}

type workspaceV2OutboundFrame struct {
	opcode gws.Opcode
	data   []byte
	done   chan error
}

type workspaceV2Connection struct {
	server          *WorkspaceV2Server
	conn            *gws.Conn
	ctx             context.Context
	cancel          context.CancelFunc
	closeOnce       sync.Once
	closing         bool
	cleanupComplete bool
	uid             int64
	tokenID         int64
	scope           string
	clientType      string
	clientName      string
	clientVersion   string
	inbound         chan workspaceV2InboundFrame
	outbound        chan workspaceV2OutboundFrame
	helloClientID   dto.WorkspaceUUID
	helloDone       bool
	subscription    *workspaceV2Subscription
	transfers       map[uuid.UUID]*workspaceV2Transfer
	seenTransfers   map[uuid.UUID]struct{}
	seenRequestIDs  map[dto.WorkspaceUUID]struct{}
	stateMu         sync.RWMutex
	writeMessage    func(gws.Opcode, []byte) error
}

type workspaceV2Transfer struct {
	mu               sync.Mutex
	cleanupOnce      sync.Once
	ctx              context.Context
	cancel           context.CancelFunc
	manager          *workspaceV2TransferManager
	owner            *workspaceV2Connection
	lifecycleOwner   *WorkspaceV2Server
	workspaceID      dto.WorkspaceUUID
	transferID       uuid.UUID
	direction        dto.WorkspaceBlobDirection
	contentHash      dto.WorkspaceContentHash
	size             uint64
	chunkSize        uint32
	chunkCount       uint64
	nextChunkIndex   uint64
	nextOffset       uint64
	uploadWriter     *io.PipeWriter
	uploadDone       chan struct{}
	uploadErr        error
	receiptRecorded  bool
	uploadComplete   bool
	download         io.ReadCloser
	downloadComplete bool
	createdAt        time.Time
	lastActivity     time.Time
	uploadInFlight   bool
	closing          bool
	cleanupStarted   bool
}

type workspaceV2TransferManager struct {
	mu          sync.Mutex
	byWorkspace map[workspaceV2HubKey]int
	byUser      map[int64]int
	active      map[*workspaceV2Transfer]struct{}
	byIdentity  map[workspaceV2TransferKey]*workspaceV2Transfer
}

func newWorkspaceV2TransferManager() *workspaceV2TransferManager {
	return &workspaceV2TransferManager{
		byWorkspace: make(map[workspaceV2HubKey]int),
		byUser:      make(map[int64]int),
		active:      make(map[*workspaceV2Transfer]struct{}),
		byIdentity:  make(map[workspaceV2TransferKey]*workspaceV2Transfer),
	}
}

func (m *workspaceV2TransferManager) reserve(c *workspaceV2Connection, transfer *workspaceV2Transfer) error {
	if m == nil || c == nil || transfer == nil {
		return errors.New("workspace transfer reservation is invalid")
	}
	if transfer.transferID == uuid.Nil || transfer.workspaceID == "" {
		return errors.New("workspace transfer identity is invalid")
	}
	c.stateMu.Lock()
	if c.closing || (c.ctx != nil && c.ctx.Err() != nil) {
		c.stateMu.Unlock()
		return context.Canceled
	}
	if _, exists := c.seenTransfers[transfer.transferID]; exists {
		c.stateMu.Unlock()
		return errors.New("workspace transfer identifier reused")
	}
	if len(c.seenTransfers) >= workspaceV2MaxSeenTransfers {
		c.stateMu.Unlock()
		return errors.New("workspace transfer history limit exceeded")
	}
	key := workspaceV2HubKey{uid: c.uid, workspaceID: transfer.workspaceID}
	identityKey := workspaceV2TransferKey{uid: c.uid, transferID: transfer.transferID}
	m.mu.Lock()
	if m.byWorkspace == nil {
		m.byWorkspace = make(map[workspaceV2HubKey]int)
	}
	if m.byUser == nil {
		m.byUser = make(map[int64]int)
	}
	if m.active == nil {
		m.active = make(map[*workspaceV2Transfer]struct{})
	}
	if m.byIdentity == nil {
		m.byIdentity = make(map[workspaceV2TransferKey]*workspaceV2Transfer)
	}
	if existing := m.byIdentity[identityKey]; existing != nil {
		m.mu.Unlock()
		c.stateMu.Unlock()
		if existing.matchesTransferIdentity(transfer) {
			return errWorkspaceV2TransferAlreadyActive
		}
		return errWorkspaceV2TransferIdentifierReused
	}
	// Keep attempted IDs even when a quota rejects the transfer; reconnect is
	// required instead of allowing a client to probe the same ID repeatedly.
	c.seenTransfers[transfer.transferID] = struct{}{}
	if len(c.transfers) >= workspaceV2MaxTransfersPerConnection {
		m.mu.Unlock()
		c.stateMu.Unlock()
		return errors.New("workspace transfer limit exceeded")
	}
	if m.byWorkspace[key] >= workspaceV2MaxTransfersPerWorkspace || m.byUser[c.uid] >= workspaceV2MaxTransfersPerUser {
		m.mu.Unlock()
		c.stateMu.Unlock()
		return errors.New("workspace transfer limit exceeded")
	}
	if transfer.createdAt.IsZero() {
		transfer.createdAt = time.Now().UTC()
	}
	if transfer.lastActivity.IsZero() {
		transfer.lastActivity = transfer.createdAt
	}
	transfer.manager = m
	transfer.owner = c
	m.byWorkspace[key]++
	m.byUser[c.uid]++
	m.active[transfer] = struct{}{}
	m.byIdentity[identityKey] = transfer
	c.transfers[transfer.transferID] = transfer
	m.mu.Unlock()
	c.stateMu.Unlock()
	return nil
}

func (m *workspaceV2TransferManager) release(transfer *workspaceV2Transfer) bool {
	if transfer == nil {
		return false
	}
	released := false
	transfer.cleanupOnce.Do(func() {
		released = true
		owner := transfer.owner
		if owner != nil {
			owner.stateMu.Lock()
			delete(owner.transfers, transfer.transferID)
			owner.stateMu.Unlock()
		}
		manager := transfer.manager
		if manager == nil {
			return
		}
		manager.mu.Lock()
		delete(manager.active, transfer)
		if owner != nil {
			identityKey := workspaceV2TransferKey{uid: owner.uid, transferID: transfer.transferID}
			if manager.byIdentity[identityKey] == transfer {
				delete(manager.byIdentity, identityKey)
			}
			key := workspaceV2HubKey{uid: owner.uid, workspaceID: transfer.workspaceID}
			if manager.byWorkspace[key] > 1 {
				manager.byWorkspace[key]--
			} else {
				delete(manager.byWorkspace, key)
			}
			if manager.byUser[owner.uid] > 1 {
				manager.byUser[owner.uid]--
			} else {
				delete(manager.byUser, owner.uid)
			}
		}
		manager.mu.Unlock()
	})
	if released {
		if owner := transfer.owner; owner != nil {
			owner.finishCleanup()
		}
		if lifecycleOwner := transfer.lifecycleOwner; lifecycleOwner != nil {
			lifecycleOwner.finishTransfer()
		}
	}
	return released
}

func (m *workspaceV2TransferManager) Expire(now time.Time) {
	if m == nil {
		return
	}
	m.mu.Lock()
	active := make([]*workspaceV2Transfer, 0, len(m.active))
	for transfer := range m.active {
		active = append(active, transfer)
	}
	m.mu.Unlock()
	for _, transfer := range active {
		transfer.mu.Lock()
		expired := !transfer.closing && !transfer.uploadInFlight && !transfer.lastActivity.IsZero() &&
			(now.Sub(transfer.lastActivity) >= workspaceV2TransferIdleExpiry || now.Sub(transfer.createdAt) >= workspaceV2TransferMaxLifetime)
		if expired {
			transfer.closing = true
		}
		owner := transfer.owner
		transfer.mu.Unlock()
		if expired && owner != nil {
			if owner.removeWorkspaceV2Transfer(transfer.transferID) && owner.conn != nil && owner.outbound != nil {
				_ = owner.writeFailure(dto.WorkspaceActionBlobEnd, nil, dto.WorkspaceErrorBlobTransferOutOfOrder)
			}
		}
	}
}

func (transfer *workspaceV2Transfer) touch(now time.Time) {
	if transfer == nil {
		return
	}
	transfer.mu.Lock()
	transfer.lastActivity = now
	transfer.mu.Unlock()
}

func (transfer *workspaceV2Transfer) beginCleanup() (*io.PipeWriter, bool, io.ReadCloser, context.CancelFunc, <-chan struct{}, bool) {
	if transfer == nil {
		return nil, false, nil, nil, nil, false
	}
	transfer.mu.Lock()
	defer transfer.mu.Unlock()
	if transfer.cleanupStarted {
		return nil, false, nil, nil, transfer.uploadDone, false
	}
	transfer.cleanupStarted = true
	transfer.closing = true
	return transfer.uploadWriter, transfer.uploadComplete, transfer.download, transfer.cancel, transfer.uploadDone, true
}

func newWorkspaceV2Connection(server *WorkspaceV2Server, conn *gws.Conn, identity workspaceV2ConnectionIdentity) *workspaceV2Connection {
	ctx, cancel := context.WithCancel(server.ctx)
	return &workspaceV2Connection{
		server:         server,
		conn:           conn,
		ctx:            ctx,
		cancel:         cancel,
		uid:            identity.uid,
		tokenID:        identity.tokenID,
		scope:          identity.scope,
		clientType:     identity.clientType,
		clientName:     identity.clientName,
		clientVersion:  identity.clientVersion,
		inbound:        make(chan workspaceV2InboundFrame, workspaceV2InboundQueueDepth),
		outbound:       make(chan workspaceV2OutboundFrame, workspaceV2OutboundQueueDepth),
		transfers:      make(map[uuid.UUID]*workspaceV2Transfer),
		seenTransfers:  make(map[uuid.UUID]struct{}, workspaceV2MaxSeenTransfers),
		seenRequestIDs: make(map[dto.WorkspaceUUID]struct{}),
	}
}

type workspaceV2ConnectionIdentity struct {
	uid           int64
	tokenID       int64
	scope         string
	clientType    string
	clientName    string
	clientVersion string
}

func (c *workspaceV2Connection) start() {
	c.startOwnedLoop(c.writerLoop)
	c.startOwnedLoop(c.processorLoop)
	c.startOwnedLoop(c.heartbeatLoop)
	c.startOwnedLoop(c.conn.ReadLoop)
}

func (c *workspaceV2Connection) startOwnedLoop(loop func()) {
	go func() {
		defer c.server.finishConnectionLoop()
		loop()
	}()
}

func (c *workspaceV2Connection) enqueueInbound(frame workspaceV2InboundFrame) bool {
	select {
	case c.inbound <- frame:
		return true
	case <-c.ctx.Done():
		return false
	}
}

func (c *workspaceV2Connection) send(opcode gws.Opcode, data []byte) error {
	frame := workspaceV2OutboundFrame{opcode: opcode, data: append([]byte(nil), data...), done: make(chan error, 1)}
	select {
	case c.outbound <- frame:
	case <-c.ctx.Done():
		return c.ctx.Err()
	}
	select {
	case err := <-frame.done:
		return err
	case <-c.ctx.Done():
		return c.ctx.Err()
	}
}

func (c *workspaceV2Connection) writerLoop() {
	for {
		select {
		case <-c.ctx.Done():
			return
		case frame := <-c.outbound:
			writeMessage := c.conn.WriteMessage
			if c.writeMessage != nil {
				writeMessage = c.writeMessage
			}
			err := writeMessage(frame.opcode, frame.data)
			frame.done <- err
			if err != nil {
				c.cleanup()
				return
			}
		}
	}
}

func (c *workspaceV2Connection) processorLoop() {
	for {
		select {
		case <-c.ctx.Done():
			return
		case frame := <-c.inbound:
			if frame.opcode == gws.OpcodeBinary {
				if err := c.handleWorkspaceV2BinaryFrame(frame.data); err != nil {
					if !c.reportWorkspaceV2BinaryFailure(frame.data, err) {
						c.closeWithCode(1002, "invalid_binary")
						return
					}
				}
				continue
			}
			if frame.opcode != gws.OpcodeText {
				c.closeWithCode(1002, "binary_not_ready")
				return
			}
			decoded, action, err := decodeWorkspaceV2ControlFrame(frame.data)
			if err != nil {
				var wireErr *workspaceV2WireError
				if !asWorkspaceV2WireError(err, &wireErr) {
					c.closeWithCode(1002, "invalid_frame")
					return
				}
				if wireErr.CloseCode != 0 {
					c.closeWithCode(wireErr.CloseCode, "invalid_frame")
					return
				}
				payload, encodeErr := encodeWorkspaceV2WireError(action, wireErr)
				if encodeErr != nil {
					c.closeWithCode(1011, "internal")
					return
				}
				if sendErr := c.send(gws.OpcodeText, payload); sendErr != nil {
					return
				}
				continue
			}
			if decoded == nil {
				c.closeWithCode(1011, "internal")
				return
			}
			requestStatus := c.registerRequestID(decoded.requestID)
			if requestStatus != workspaceV2RequestAccepted {
				requestID := decoded.requestID
				failureCode := dto.WorkspaceErrorInvalidRequest
				if requestStatus == workspaceV2RequestLimitExceeded {
					failureCode = dto.WorkspaceErrorServerBusy
				}
				if writeErr := c.writeFailure(decoded.action, &requestID, failureCode); writeErr != nil {
					return
				}
				if requestStatus == workspaceV2RequestLimitExceeded {
					c.closeWithCode(1013, "server_busy")
					return
				}
				continue
			}
			if dispatchErr := c.dispatch(decoded); dispatchErr != nil {
				c.closeWithCode(1011, "internal")
				return
			}
		}
	}
}

func (c *workspaceV2Connection) reportWorkspaceV2BinaryFailure(frame []byte, err error) bool {
	if len(frame) <= dto.WorkspaceBlobHeaderSize {
		return false
	}
	var declaredDigest [16]byte
	copy(declaredDigest[:], frame[48:64])
	header, parseErr := dto.UnmarshalWorkspaceBlobHeader(
		frame[:dto.WorkspaceBlobHeaderSize],
		uint32(len(frame)-dto.WorkspaceBlobHeaderSize),
		declaredDigest,
	)
	if parseErr != nil {
		return false
	}
	transfer := c.workspaceV2Transfer(header.TransferID)
	if transfer == nil {
		return false
	}
	if writeErr := c.writeFailure(dto.WorkspaceActionBlobEnd, nil, workspaceV2ErrorCodeForBinaryFrameError(err)); writeErr != nil {
		return false
	}
	c.removeWorkspaceV2Transfer(header.TransferID)
	return true
}

func asWorkspaceV2WireError(err error, target **workspaceV2WireError) bool {
	if err == nil || target == nil {
		return false
	}
	var wireErr *workspaceV2WireError
	if !errors.As(err, &wireErr) {
		return false
	}
	*target = wireErr
	return true
}

func (c *workspaceV2Connection) heartbeatLoop() {
	ticker := time.NewTicker(workspaceV2Heartbeat)
	defer ticker.Stop()
	for {
		select {
		case <-c.ctx.Done():
			return
		case <-ticker.C:
			_ = c.conn.SetReadDeadline(time.Now().Add(workspaceV2HeartbeatWait))
			if err := c.send(gws.OpcodePing, nil); err != nil {
				return
			}
		}
	}
}

func (c *workspaceV2Connection) closeWithCode(code uint16, reason string) {
	c.closeOnce.Do(func() {
		_ = c.conn.WriteClose(code, []byte(reason))
		c.cleanupState()
	})
}

func (c *workspaceV2Connection) cleanup() {
	c.closeOnce.Do(c.cleanupState)
}

func (c *workspaceV2Connection) cleanupState() {
	c.stateMu.Lock()
	c.closing = true
	subscription := c.subscription
	c.subscription = nil
	c.stateMu.Unlock()
	if c.cancel != nil {
		c.cancel()
	}
	if c.conn != nil && c.conn.NetConn() != nil {
		_ = c.conn.NetConn().Close()
	}
	if subscription != nil && c.server != nil && c.server.hub != nil {
		c.server.hub.unregister(c, workspaceV2HubKey{uid: c.uid, workspaceID: subscription.workspaceID})
	}
	c.cleanupTransfers()
	c.finishCleanup()
}

func (c *workspaceV2Connection) finishCleanup() {
	if c == nil {
		return
	}
	c.stateMu.Lock()
	if !c.closing || c.cleanupComplete || len(c.transfers) != 0 {
		c.stateMu.Unlock()
		return
	}
	c.cleanupComplete = true
	c.stateMu.Unlock()
	if c.server != nil {
		c.server.removeConnection(c)
	}
}
