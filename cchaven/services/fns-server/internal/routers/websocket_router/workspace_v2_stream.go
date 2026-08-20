package websocket_router

import (
	"context"
	"errors"
	"sort"
	"sync"

	"github.com/google/uuid"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/service"
	"go.uber.org/zap"
)

const workspaceV2MaxRequestIDsPerConnection = 4096

type workspaceV2RequestStatus uint8

const (
	workspaceV2RequestAccepted workspaceV2RequestStatus = iota + 1
	workspaceV2RequestDuplicate
	workspaceV2RequestLimitExceeded
)

type workspaceV2HubKey struct {
	uid         int64
	workspaceID dto.WorkspaceUUID
}

type workspaceV2LiveKind uint8

const (
	workspaceV2LiveEvent workspaceV2LiveKind = iota + 1
	workspaceV2LiveConflictCreated
	workspaceV2LiveConflictResolved
	workspaceV2LiveBlobNeed
)

type workspaceV2LiveNotification struct {
	workspaceID  dto.WorkspaceUUID
	recipient    *workspaceV2Connection
	kind         workspaceV2LiveKind
	treeRevision *dto.WorkspaceRevision
	conflictID   dto.WorkspaceUUID
	mutation     *dto.WorkspaceMutation
	accepted     *dto.WorkspaceMutationAcceptedMessage
	conflict     *dto.WorkspaceConflictCreatedMessage
	resolved     *dto.WorkspaceConflictResolvedMessage
	blobNeed     *dto.WorkspaceBlobNeedUploadPush
}

type workspaceV2PendingConflictAuthority interface {
	CurrentPendingConflict(context.Context, int64, dto.WorkspaceUUID, dto.WorkspaceUUID) (*dto.WorkspaceConflictCreatedMessage, error)
}

type workspaceV2ConflictGeneration struct {
	conflictID       dto.WorkspaceUUID
	conflictRevision dto.WorkspaceConflictRevision
}

type workspaceV2Subscription struct {
	workspaceID            dto.WorkspaceUUID
	clientID               dto.WorkspaceUUID
	streamID               dto.WorkspaceUUID
	lastAck                dto.WorkspaceRevision
	lastDelivered          dto.WorkspaceRevision
	ackableRevision        dto.WorkspaceRevision
	nextEventIndex         uint32
	streaming              bool
	flushing               bool
	overflowed             bool
	pendingRevisionItems   map[dto.WorkspaceRevision][]workspaceV2LiveNotification
	pendingConflicts       map[dto.WorkspaceUUID]*workspaceV2ConflictBuffer
	pendingOrdered         map[dto.WorkspaceRevision][]workspaceV2LiveNotification
	pendingPushes          []workspaceV2LiveNotification
	authoritativeConflicts map[workspaceV2ConflictGeneration]struct{}
	dispatchMu             sync.Mutex
}

type workspaceV2ConflictBuffer struct {
	created  *workspaceV2LiveNotification
	resolved *workspaceV2LiveNotification
}

type workspaceV2Hub struct {
	mu          sync.Mutex
	subscribers map[workspaceV2HubKey]map[*workspaceV2Connection]struct{}
}

func (c *workspaceV2Connection) registerRequestID(requestID dto.WorkspaceUUID) workspaceV2RequestStatus {
	c.stateMu.Lock()
	defer c.stateMu.Unlock()
	if _, exists := c.seenRequestIDs[requestID]; exists {
		return workspaceV2RequestDuplicate
	}
	if len(c.seenRequestIDs) >= workspaceV2MaxRequestIDsPerConnection {
		return workspaceV2RequestLimitExceeded
	}
	c.seenRequestIDs[requestID] = struct{}{}
	return workspaceV2RequestAccepted
}

func (c *workspaceV2Connection) dispatch(decoded *workspaceV2DecodedRequest) error {
	c.stateMu.RLock()
	helloDone := c.helloDone
	helloClientID := c.helloClientID
	c.stateMu.RUnlock()
	if !helloDone && decoded.action != dto.WorkspaceActionHello {
		return c.writeFailure(decoded.action, &decoded.requestID, dto.WorkspaceErrorInvalidRequest)
	}
	if helloDone && decoded.action == dto.WorkspaceActionHello {
		return c.writeFailure(decoded.action, &decoded.requestID, dto.WorkspaceErrorInvalidRequest)
	}
	switch request := decoded.data.(type) {
	case *dto.WorkspaceHelloRequest:
		return c.handleHello(decoded.requestID, request)
	case *dto.WorkspaceSubscribeRequest:
		if request.ClientID != helloClientID {
			return c.writeFailure(decoded.action, &decoded.requestID, dto.WorkspaceErrorInvalidRequest,
				dto.WorkspaceV2FieldError{Field: "data.clientId", Reason: "client_mismatch"})
		}
		return c.handleSubscribe(decoded.requestID, request)
	case *dto.WorkspaceMutation:
		if request.ClientID != helloClientID {
			return c.writeFailure(decoded.action, &decoded.requestID, dto.WorkspaceErrorInvalidRequest,
				dto.WorkspaceV2FieldError{Field: "data.clientId", Reason: "client_mismatch"})
		}
		return c.handleMutation(decoded.requestID, request)
	case *dto.WorkspaceAckRequest:
		if request.ClientID != helloClientID {
			return c.writeFailure(decoded.action, &decoded.requestID, dto.WorkspaceErrorInvalidRequest,
				dto.WorkspaceV2FieldError{Field: "data.clientId", Reason: "client_mismatch"})
		}
		return c.handleAck(decoded.requestID, request)
	case *dto.WorkspaceConflictResolvedRequest:
		if request.ClientID != helloClientID {
			return c.writeFailure(decoded.action, &decoded.requestID, dto.WorkspaceErrorInvalidRequest,
				dto.WorkspaceV2FieldError{Field: "data.clientId", Reason: "client_mismatch"})
		}
		return c.handleResolveConflict(decoded.requestID, request)
	case *dto.WorkspaceBlobNeedDownloadRequest:
		return c.handleBlobNeedDownload(decoded.requestID, request)
	case *dto.WorkspaceBlobBeginMessage:
		return c.handleBlobBegin(decoded.requestID, request)
	case *dto.WorkspaceBlobEndMessage:
		return c.handleBlobEnd(decoded.requestID, request)
	default:
		return c.writeFailure(decoded.action, &decoded.requestID, dto.WorkspaceErrorInvalidRequest)
	}
}

func (c *workspaceV2Connection) handleHello(requestID dto.WorkspaceUUID, request *dto.WorkspaceHelloRequest) error {
	c.stateMu.RLock()
	alreadyDone := c.helloDone
	c.stateMu.RUnlock()
	if alreadyDone {
		return c.writeFailure(dto.WorkspaceActionHello, &requestID, dto.WorkspaceErrorInvalidRequest)
	}
	if request == nil {
		return c.writeFailure(dto.WorkspaceActionHello, &requestID, dto.WorkspaceErrorInvalidRequest)
	}
	if err := request.Validate(); err != nil {
		return c.writeFailure(dto.WorkspaceActionHello, &requestID, dto.WorkspaceErrorInvalidRequest, workspaceV2FieldFromError(err))
	}
	response := dto.WorkspaceHelloResponse{
		ProtocolVersion:           "2",
		ServerVersion:             c.server.version,
		MaxControlFrameBytes:      dto.WorkspaceMaxControlFrameBytes,
		MaxBinaryChunkBytes:       dto.WorkspaceBlobChunkSize,
		MaxBlobBytes:              dto.WorkspaceMaxBlobBytes,
		MaxTransfersPerConnection: 4,
		HeartbeatSeconds:          25,
	}
	if err := response.Validate(); err != nil {
		return err
	}
	payload, err := encodeWorkspaceV2Success(dto.WorkspaceActionHello, requestID, &response)
	if err != nil {
		return err
	}
	c.stateMu.Lock()
	c.helloClientID = request.ClientID
	c.helloDone = true
	c.stateMu.Unlock()
	return c.sendText(payload)
}

func (c *workspaceV2Connection) handleSubscribe(requestID dto.WorkspaceUUID, request *dto.WorkspaceSubscribeRequest) error {
	if request == nil {
		return c.writeFailure(dto.WorkspaceActionSubscribe, &requestID, dto.WorkspaceErrorInvalidRequest)
	}
	if err := request.Validate(); err != nil {
		return c.writeFailure(dto.WorkspaceActionSubscribe, &requestID, dto.WorkspaceErrorInvalidRequest, workspaceV2FieldFromError(err))
	}
	if c.server.access == nil {
		return c.writeFailure(dto.WorkspaceActionSubscribe, &requestID, dto.WorkspaceErrorForbidden)
	}
	if err := c.server.access.Authorize(c.uid, request.WorkspaceID); err != nil {
		return c.writeFailure(dto.WorkspaceActionSubscribe, &requestID, dto.WorkspaceErrorForbidden)
	}
	if c.server.syncService == nil {
		return c.writeFailure(dto.WorkspaceActionSubscribe, &requestID, dto.WorkspaceErrorInternal)
	}

	c.stateMu.Lock()
	previous := c.subscription
	if previous != nil && previous.streaming {
		c.stateMu.Unlock()
		return c.writeFailure(dto.WorkspaceActionSubscribe, &requestID, dto.WorkspaceErrorInvalidRequest)
	}
	lastDelivered := dto.WorkspaceRevision(0)
	if previous != nil && previous.workspaceID == request.WorkspaceID {
		lastDelivered = previous.lastDelivered
	}
	c.stateMu.Unlock()
	if previous != nil {
		previous.dispatchMu.Lock()
		c.server.hub.unregister(c, workspaceV2HubKey{uid: c.uid, workspaceID: previous.workspaceID})
	}
	streamID := dto.WorkspaceUUID(uuid.NewString())
	subscription := &workspaceV2Subscription{
		workspaceID:            request.WorkspaceID,
		clientID:               request.ClientID,
		streamID:               streamID,
		lastAck:                request.LastAckRevision,
		lastDelivered:          lastDelivered,
		streaming:              true,
		pendingRevisionItems:   make(map[dto.WorkspaceRevision][]workspaceV2LiveNotification),
		pendingConflicts:       make(map[dto.WorkspaceUUID]*workspaceV2ConflictBuffer),
		pendingOrdered:         make(map[dto.WorkspaceRevision][]workspaceV2LiveNotification),
		authoritativeConflicts: make(map[workspaceV2ConflictGeneration]struct{}),
	}
	c.stateMu.Lock()
	c.subscription = subscription
	c.stateMu.Unlock()
	if previous != nil {
		previous.dispatchMu.Unlock()
	}
	c.server.hub.register(c, workspaceV2HubKey{uid: c.uid, workspaceID: request.WorkspaceID})

	changeSet, err := c.server.syncService.Subscribe(c.ctx, c.uid, *request)
	if err != nil {
		c.server.hub.unregister(c, workspaceV2HubKey{uid: c.uid, workspaceID: request.WorkspaceID})
		c.restoreSubscription(previous)
		return c.writeFailure(dto.WorkspaceActionSubscribe, &requestID, workspaceV2ServiceErrorCode(err))
	}
	if err := c.streamChangeSet(requestID, *request, changeSet, subscription); err != nil {
		c.closeWithCode(1011, "internal")
		return err
	}
	return nil
}

func (c *workspaceV2Connection) handleMutation(requestID dto.WorkspaceUUID, mutation *dto.WorkspaceMutation) error {
	if mutation == nil {
		return c.writeFailure(dto.WorkspaceActionMutation, &requestID, dto.WorkspaceErrorInvalidRequest)
	}
	if err := mutation.Validate(); err != nil {
		return c.writeFailure(dto.WorkspaceActionMutation, &requestID, dto.WorkspaceErrorInvalidRequest, workspaceV2FieldFromError(err))
	}
	if _, err := c.subscriptionForWorkspace(mutation.WorkspaceID); err != nil {
		return c.writeFailure(dto.WorkspaceActionMutation, &requestID, workspaceV2ServiceErrorCode(err))
	}
	if err := c.authorizeWorkspacePath(mutation.WorkspaceID, mutation.Path); err != nil {
		return c.writeFailure(dto.WorkspaceActionMutation, &requestID, workspaceV2ServiceErrorCode(err))
	}
	if mutation.Kind == dto.WorkspaceMutationRename && mutation.NewPath != nil {
		if err := c.authorizeWorkspacePath(mutation.WorkspaceID, *mutation.NewPath); err != nil {
			return c.writeFailure(dto.WorkspaceActionMutation, &requestID, workspaceV2ServiceErrorCode(err))
		}
	}
	if c.server.syncService == nil {
		return c.writeFailure(dto.WorkspaceActionMutation, &requestID, dto.WorkspaceErrorInternal)
	}
	outcome, err := c.server.syncService.ApplyMutation(c.ctx, c.uid, *mutation)
	if err != nil {
		return c.writeServiceError(dto.WorkspaceActionMutation, requestID, err)
	}
	if outcome == nil || (outcome.Accepted == nil) == (outcome.Rejected == nil) {
		return errors.New("workspace mutation returned invalid outcome")
	}
	if outcome.Accepted != nil {
		return c.writeMutationAccepted(requestID, *mutation, outcome.Accepted)
	}
	writeErr := c.writeMutationRejected(requestID, outcome.Rejected)
	if outcome.RequiredUpload != nil {
		c.server.hub.publish(workspaceV2HubKey{uid: c.uid, workspaceID: mutation.WorkspaceID}, workspaceV2LiveNotification{
			recipient: c, kind: workspaceV2LiveBlobNeed, blobNeed: outcome.RequiredUpload,
		})
	}
	if outcome.Conflict != nil {
		c.server.hub.publish(workspaceV2HubKey{uid: c.uid, workspaceID: mutation.WorkspaceID}, workspaceV2LiveNotification{
			kind: workspaceV2LiveConflictCreated, conflictID: outcome.Conflict.ConflictID, conflict: outcome.Conflict,
		})
	}
	return writeErr
}

func (c *workspaceV2Connection) handleAck(requestID dto.WorkspaceUUID, request *dto.WorkspaceAckRequest) error {
	if request == nil {
		return c.writeFailure(dto.WorkspaceActionAck, &requestID, dto.WorkspaceErrorInvalidRequest)
	}
	subscription, err := c.subscriptionForWorkspace(request.WorkspaceID)
	if err != nil {
		return c.writeFailure(dto.WorkspaceActionAck, &requestID, workspaceV2ServiceErrorCode(err))
	}
	c.stateMu.RLock()
	lastAck := subscription.lastAck
	lastAckable := subscription.ackableRevision
	c.stateMu.RUnlock()
	if err := request.Validate(lastAck, lastAckable); err != nil {
		return c.writeFailure(dto.WorkspaceActionAck, &requestID, dto.WorkspaceErrorInvalidRequest, workspaceV2FieldFromError(err))
	}
	if err := c.authorizeWorkspaceRequest(request.WorkspaceID); err != nil {
		return c.writeFailure(dto.WorkspaceActionAck, &requestID, workspaceV2ServiceErrorCode(err))
	}
	if c.server.syncService == nil {
		return c.writeFailure(dto.WorkspaceActionAck, &requestID, dto.WorkspaceErrorInternal)
	}
	if err := c.server.syncService.Acknowledge(c.ctx, c.uid, *request, lastAckable); err != nil {
		return c.writeServiceError(dto.WorkspaceActionAck, requestID, err)
	}
	if err := sendWorkspaceV2Success(c, dto.WorkspaceActionAck, requestID, request); err != nil {
		return err
	}
	c.stateMu.Lock()
	if c.subscription == subscription {
		subscription.lastAck = request.Revision
	}
	c.stateMu.Unlock()
	return nil
}

func (c *workspaceV2Connection) handleResolveConflict(requestID dto.WorkspaceUUID, request *dto.WorkspaceConflictResolvedRequest) error {
	if request == nil {
		return c.writeFailure(dto.WorkspaceActionConflictResolved, &requestID, dto.WorkspaceErrorInvalidRequest)
	}
	if err := workspaceV2ValidateResolveRequest(*request); err != nil {
		return c.writeFailure(dto.WorkspaceActionConflictResolved, &requestID, dto.WorkspaceErrorInvalidRequest, workspaceV2FieldFromError(err))
	}
	if _, err := c.subscriptionForWorkspace(request.WorkspaceID); err != nil {
		return c.writeFailure(dto.WorkspaceActionConflictResolved, &requestID, workspaceV2ServiceErrorCode(err))
	}
	if err := c.authorizeWorkspacePath(request.WorkspaceID, request.Path); err != nil {
		return c.writeFailure(dto.WorkspaceActionConflictResolved, &requestID, workspaceV2ServiceErrorCode(err))
	}
	if c.server.syncService == nil {
		return c.writeFailure(dto.WorkspaceActionConflictResolved, &requestID, dto.WorkspaceErrorInternal)
	}
	outcome, err := c.server.syncService.ResolveConflict(c.ctx, c.uid, *request)
	if err != nil {
		return c.writeServiceError(dto.WorkspaceActionConflictResolved, requestID, err)
	}
	if outcome == nil || outcome.Resolved == nil {
		return errors.New("workspace conflict resolution returned empty outcome")
	}
	writeErr := sendWorkspaceV2Success(c, dto.WorkspaceActionConflictResolved, requestID, outcome.Resolved)
	revision := outcome.Resolved.Revision
	c.server.hub.publish(workspaceV2HubKey{uid: c.uid, workspaceID: request.WorkspaceID}, workspaceV2LiveNotification{
		kind: workspaceV2LiveConflictResolved, conflictID: outcome.Resolved.ConflictID,
		treeRevision: &revision, resolved: outcome.Resolved,
	})
	return writeErr
}

func (c *workspaceV2Connection) writeMutationAccepted(requestID dto.WorkspaceUUID, mutation dto.WorkspaceMutation, accepted *dto.WorkspaceMutationAcceptedMessage) error {
	writeErr := sendWorkspaceV2Success(c, dto.WorkspaceActionMutationAccepted, requestID, accepted)
	revision := accepted.Revision
	c.server.hub.publish(workspaceV2HubKey{uid: c.uid, workspaceID: mutation.WorkspaceID}, workspaceV2LiveNotification{
		kind: workspaceV2LiveEvent, treeRevision: &revision, mutation: &mutation, accepted: accepted,
	})
	return writeErr
}

func (c *workspaceV2Connection) writeMutationRejected(requestID dto.WorkspaceUUID, rejected *dto.WorkspaceMutationRejectedMessage) error {
	return sendWorkspaceV2Success(c, dto.WorkspaceActionMutationRejected, requestID, rejected)
}

func sendWorkspaceV2Success[T any](c *workspaceV2Connection, action dto.WorkspaceV2Action, requestID dto.WorkspaceUUID, data *T) error {
	payload, err := encodeWorkspaceV2Success(action, requestID, data)
	if err != nil {
		return err
	}
	return c.sendText(payload)
}

func (c *workspaceV2Connection) subscriptionForWorkspace(workspaceID dto.WorkspaceUUID) (*workspaceV2Subscription, error) {
	c.stateMu.RLock()
	subscription := c.subscription
	c.stateMu.RUnlock()
	if subscription == nil {
		return nil, &service.WorkspaceServiceError{Code: dto.WorkspaceErrorInvalidRequest}
	}
	if subscription.workspaceID != workspaceID {
		return nil, &service.WorkspaceServiceError{Code: dto.WorkspaceErrorForbidden}
	}
	return subscription, nil
}

func (c *workspaceV2Connection) authorizeWorkspaceRequest(workspaceID dto.WorkspaceUUID) error {
	if c.server.access == nil {
		return &service.WorkspaceServiceError{Code: dto.WorkspaceErrorForbidden}
	}
	return c.server.access.Authorize(c.uid, workspaceID)
}

func (c *workspaceV2Connection) authorizeWorkspacePath(workspaceID dto.WorkspaceUUID, path dto.WorkspacePath) error {
	if c.server == nil || c.server.access == nil {
		return &service.WorkspaceServiceError{Code: dto.WorkspaceErrorForbidden}
	}
	return c.server.access.CheckPath(c.uid, workspaceID, path)
}

func (c *workspaceV2Connection) writeServiceError(action dto.WorkspaceV2Action, requestID dto.WorkspaceUUID, err error) error {
	var validationErr *dto.WorkspaceValidationError
	if errors.As(err, &validationErr) {
		return c.writeFailure(action, &requestID, dto.WorkspaceErrorInvalidRequest, workspaceV2FieldFromError(err))
	}
	code := workspaceV2ServiceErrorCode(err)
	if code == dto.WorkspaceErrorInternal && c.server != nil && c.server.logger != nil {
		c.server.logger.Error("workspace v2 service request failed",
			zap.String("action", string(action)),
			zap.Int64("uid", c.uid),
			zap.String("requestId", string(requestID)),
			zap.Error(err),
		)
	}
	writeErr := c.writeFailure(action, &requestID, code)
	var serviceErr *service.WorkspaceServiceError
	if errors.As(err, &serviceErr) && serviceErr.RequiredUpload != nil {
		c.server.hub.publish(workspaceV2HubKey{uid: c.uid, workspaceID: serviceErr.RequiredUpload.WorkspaceID}, workspaceV2LiveNotification{
			recipient: c, kind: workspaceV2LiveBlobNeed, blobNeed: serviceErr.RequiredUpload,
		})
	}
	if errors.As(err, &serviceErr) && serviceErr.RefreshedConflict != nil {
		c.server.hub.publish(workspaceV2HubKey{uid: c.uid, workspaceID: serviceErr.RefreshedConflict.WorkspaceID}, workspaceV2LiveNotification{
			kind:       workspaceV2LiveConflictCreated,
			conflictID: serviceErr.RefreshedConflict.ConflictID,
			conflict:   serviceErr.RefreshedConflict,
		})
	}
	return writeErr
}

func workspaceV2ValidateResolveRequest(request dto.WorkspaceConflictResolvedRequest) error {
	if _, err := dto.ParseWorkspaceUUID("workspaceId", string(request.WorkspaceID)); err != nil {
		return err
	}
	if _, err := dto.ParseWorkspaceUUID("clientId", string(request.ClientID)); err != nil {
		return err
	}
	if _, err := dto.ParseWorkspaceUUID("operationId", string(request.OperationID)); err != nil {
		return err
	}
	if _, err := dto.ParseWorkspaceUUID("conflictId", string(request.ConflictID)); err != nil {
		return err
	}
	if request.ConflictRevision == (dto.WorkspaceConflictRevision{}) {
		return &dto.WorkspaceValidationError{Field: "conflictRevision", Reason: "must_be_positive"}
	}
	if _, err := dto.ParseWorkspacePath(string(request.Path)); err != nil {
		return err
	}
	if !request.ContentHash.Present {
		return &dto.WorkspaceValidationError{Field: "contentHash", Reason: "required_key_missing"}
	}
	if request.ContentHash.Value != nil {
		if _, err := dto.ParseWorkspaceContentHash(string(*request.ContentHash.Value)); err != nil {
			return err
		}
	}
	return nil
}

func (c *workspaceV2Connection) restoreSubscription(previous *workspaceV2Subscription) {
	c.stateMu.Lock()
	c.subscription = previous
	c.stateMu.Unlock()
	if previous != nil {
		c.server.hub.register(c, workspaceV2HubKey{uid: c.uid, workspaceID: previous.workspaceID})
	}
}

func (c *workspaceV2Connection) streamChangeSet(
	requestID dto.WorkspaceUUID,
	request dto.WorkspaceSubscribeRequest,
	changeSet *domain.WorkspaceChangeSet,
	subscription *workspaceV2Subscription,
) error {
	if changeSet == nil {
		return errors.New("workspace subscribe returned nil change set")
	}
	if changeSet.PendingConflicts != nil {
		defer func() { _ = changeSet.PendingConflicts.Close() }()
	}
	subscription.dispatchMu.Lock()
	defer subscription.dispatchMu.Unlock()
	begin := dto.WorkspaceSnapshotBeginMessage{
		WorkspaceID:   request.WorkspaceID,
		StreamID:      subscription.streamID,
		Mode:          changeSet.Mode,
		FromRevision:  changeSet.FromRevision,
		FinalRevision: changeSet.FinalRevision,
		EntryCount:    changeSet.EntryCount,
		EventCount:    changeSet.EventCount,
		ConflictCount: changeSet.ConflictCount,
	}
	if err := begin.Validate(); err != nil {
		return err
	}
	if err := c.sendPush(dto.WorkspaceActionSnapshotBegin, &begin); err != nil {
		return err
	}

	eventIndex := uint32(0)
	switch changeSet.Mode {
	case dto.WorkspaceSnapshotFull:
		if len(changeSet.RevisionItems) != 0 || uint64(len(changeSet.Entries)) != uint64(changeSet.EntryCount) {
			return errors.New("workspace snapshot entry count mismatch")
		}
		var previousPath string
		for index, entry := range changeSet.Entries {
			if index > 0 && string(entry.Path) <= previousPath {
				return errors.New("workspace snapshot paths are not byte ordered")
			}
			previousPath = string(entry.Path)
			message := dto.WorkspaceSnapshotEntryMessage{WorkspaceID: request.WorkspaceID, StreamID: subscription.streamID, Index: uint32(index), Entry: entry}
			if err := c.sendPush(dto.WorkspaceActionSnapshotEntry, &message); err != nil {
				return err
			}
		}
	case dto.WorkspaceSnapshotIncremental:
		if len(changeSet.Entries) != 0 || uint64(len(changeSet.RevisionItems)) != uint64(changeSet.EventCount) {
			return errors.New("workspace revision item count mismatch")
		}
		previousRevision := changeSet.FromRevision
		previousEventIndex := ^uint32(0)
		for _, item := range changeSet.RevisionItems {
			if item.Revision <= previousRevision {
				return errors.New("workspace revisions are not strictly increasing")
			}
			priorRevision := previousRevision
			previousRevision = item.Revision
			switch {
			case item.Event != nil && item.ConflictResolved == nil:
				event := dto.WorkspaceEventMessage{
					WorkspaceID: request.WorkspaceID, StreamID: subscription.streamID, Index: eventIndex,
					Revision: item.Event.Revision, OperationID: item.Event.OperationID, OriginClientID: item.Event.OriginClientID,
					Mutation: item.Event.Mutation, PathState: item.Event.PathState, OldPathState: item.Event.OldPathState, NewPathState: item.Event.NewPathState,
				}
				if err := event.Validate(previousEventIndex, priorRevision); err != nil {
					return err
				}
				if err := c.sendPush(dto.WorkspaceActionEvent, &event); err != nil {
					return err
				}
				previousEventIndex = eventIndex
				eventIndex++
			case item.Event == nil && item.ConflictResolved != nil:
				if item.ConflictResolved.WorkspaceID != request.WorkspaceID {
					return errors.New("workspace conflict resolution workspace mismatch")
				}
				if item.ConflictResolved.Revision != item.Revision {
					return errors.New("workspace conflict resolution revision mismatch")
				}
				if err := c.sendPush(dto.WorkspaceActionConflictResolved, item.ConflictResolved); err != nil {
					return err
				}
			default:
				return errors.New("workspace revision item union is invalid")
			}
		}
		if previousRevision != changeSet.FinalRevision {
			return errors.New("workspace revision items do not reach final revision")
		}
	default:
		return errors.New("workspace snapshot mode is invalid")
	}

	deliveredConflicts, err := c.writeAuthoritativeConflicts(changeSet, request.WorkspaceID)
	if err != nil {
		return err
	}
	deliveredCount, err := checkedWorkspaceV2DeliveredCount(begin)
	if err != nil {
		return err
	}
	end := dto.WorkspaceSnapshotEndMessage{
		WorkspaceID: request.WorkspaceID, StreamID: subscription.streamID, Mode: changeSet.Mode,
		DeliveredCount: deliveredCount, FinalRevision: changeSet.FinalRevision,
	}
	if err := end.ValidateAgainst(begin); err != nil {
		return err
	}
	if err := c.sendPush(dto.WorkspaceActionSnapshotEnd, &end); err != nil {
		return err
	}
	c.stateMu.Lock()
	subscription.lastDelivered = end.FinalRevision
	subscription.ackableRevision = end.FinalRevision
	subscription.nextEventIndex = eventIndex
	subscription.streaming = false
	subscription.flushing = true
	subscription.authoritativeConflicts = make(map[workspaceV2ConflictGeneration]struct{}, len(deliveredConflicts))
	for conflictID, conflictRevision := range deliveredConflicts {
		subscription.authoritativeConflicts[workspaceV2ConflictGeneration{conflictID: conflictID, conflictRevision: conflictRevision}] = struct{}{}
	}
	c.stateMu.Unlock()
	return c.flushBufferedLive(subscription, end.FinalRevision, deliveredConflicts)
}

func (c *workspaceV2Connection) writeAuthoritativeConflicts(changeSet *domain.WorkspaceChangeSet, workspaceID dto.WorkspaceUUID) (map[dto.WorkspaceUUID]dto.WorkspaceConflictRevision, error) {
	delivered := make(map[dto.WorkspaceUUID]dto.WorkspaceConflictRevision)
	if changeSet.ConflictCount == 0 {
		return delivered, nil
	}
	if changeSet.PendingConflicts == nil {
		return nil, errors.New("workspace conflict cursor is nil")
	}
	for index := uint32(0); index < changeSet.ConflictCount; index++ {
		conflict, err := changeSet.PendingConflicts.Next(c.ctx)
		if err != nil {
			return nil, err
		}
		if conflict == nil || conflict.WorkspaceID != workspaceID {
			return nil, errors.New("workspace conflict cursor ended early")
		}
		if err := conflict.Validate(); err != nil {
			return nil, err
		}
		if _, exists := delivered[conflict.ConflictID]; exists {
			return nil, errors.New("workspace conflict cursor returned duplicate row")
		}
		if err := c.sendPush(dto.WorkspaceActionConflictCreated, conflict); err != nil {
			return nil, err
		}
		delivered[conflict.ConflictID] = conflict.ConflictRevision
	}
	extra, err := changeSet.PendingConflicts.Next(c.ctx)
	if err != nil {
		return nil, err
	}
	if extra != nil {
		return nil, errors.New("workspace conflict cursor returned extra rows")
	}
	return delivered, nil
}

func (c *workspaceV2Connection) flushBufferedLive(
	subscription *workspaceV2Subscription,
	finalRevision dto.WorkspaceRevision,
	deliveredConflicts map[dto.WorkspaceUUID]dto.WorkspaceConflictRevision,
) error {
	for {
		c.stateMu.Lock()
		if subscription.pendingOrdered == nil {
			subscription.pendingOrdered = make(map[dto.WorkspaceRevision][]workspaceV2LiveNotification)
		}
		for revision, notifications := range subscription.pendingRevisionItems {
			delete(subscription.pendingRevisionItems, revision)
			if revision > finalRevision && revision > subscription.lastDelivered {
				subscription.pendingOrdered[revision] = append(subscription.pendingOrdered[revision], notifications...)
			}
		}
		conflicts := make([]workspaceV2LiveNotification, 0, len(subscription.pendingConflicts))
		for conflictID, buffer := range subscription.pendingConflicts {
			delete(subscription.pendingConflicts, conflictID)
			if buffer == nil {
				continue
			}
			if buffer.resolved != nil && buffer.resolved.treeRevision == nil {
				subscription.flushing = false
				c.stateMu.Unlock()
				return errors.New("workspace live resolution has no tree revision")
			}
			if buffer.created != nil && buffer.resolved == nil {
				alreadyDelivered := false
				if deliveredRevision, exists := deliveredConflicts[buffer.created.conflictID]; exists && buffer.created.conflict != nil {
					alreadyDelivered = deliveredRevision == buffer.created.conflict.ConflictRevision
				}
				if !alreadyDelivered {
					conflicts = append(conflicts, *buffer.created)
				}
			}
		}
		pushes := append([]workspaceV2LiveNotification(nil), subscription.pendingPushes...)
		subscription.pendingPushes = nil
		overflowed := subscription.overflowed
		subscription.overflowed = false
		ready := make([]workspaceV2LiveNotification, 0)
		if subscription.lastDelivered != ^dto.WorkspaceRevision(0) {
			for next := subscription.lastDelivered + 1; ; next++ {
				notifications, exists := subscription.pendingOrdered[next]
				if !exists {
					break
				}
				delete(subscription.pendingOrdered, next)
				ready = append(ready, notifications...)
				if next == ^dto.WorkspaceRevision(0) {
					break
				}
			}
		}
		missingRevision := len(ready) == 0 && len(subscription.pendingOrdered) != 0
		if missingRevision {
			// Do not let a gap observed during snapshot flush turn into a permanent
			// skip.  Keep the future revisions ordered until the missing revision
			// arrives after the flush transitions to live mode.
			subscription.flushing = false
		}
		c.stateMu.Unlock()

		// A buffered Created is sent before buffered revision items. If the same
		// conflict was resolved before End, its Created was intentionally omitted
		// above and only the revision item remains.
		sort.Slice(conflicts, func(i, j int) bool { return conflicts[i].conflictID < conflicts[j].conflictID })
		for _, notification := range conflicts {
			if err := c.sendLiveNotificationLocked(subscription, notification); err != nil {
				return err
			}
		}
		for _, notification := range ready {
			if err := c.sendLiveNotificationLocked(subscription, notification); err != nil {
				return err
			}
		}
		for _, notification := range pushes {
			if err := c.sendLiveNotificationLocked(subscription, notification); err != nil {
				return err
			}
		}
		if overflowed {
			c.stateMu.Lock()
			subscription.flushing = false
			c.stateMu.Unlock()
			c.closeWithCode(1013, "server_busy")
			return errors.New("workspace live backlog overflowed")
		}
		if missingRevision {
			return nil
		}
		c.stateMu.Lock()
		pending := workspaceV2PendingLiveCount(subscription)
		if pending == 0 {
			subscription.flushing = false
		}
		stillOverflowed := subscription.overflowed
		c.stateMu.Unlock()
		if stillOverflowed {
			c.closeWithCode(1013, "server_busy")
			return errors.New("workspace live backlog overflowed")
		}
		if pending == 0 {
			return nil
		}
	}
}

func workspaceV2PendingLiveCount(subscription *workspaceV2Subscription) int {
	if subscription == nil {
		return 0
	}
	return len(subscription.pendingRevisionItems) + len(subscription.pendingConflicts) + len(subscription.pendingOrdered) + len(subscription.pendingPushes)
}

func (c *workspaceV2Connection) sendLiveNotification(subscription *workspaceV2Subscription, notification workspaceV2LiveNotification) error {
	subscription.dispatchMu.Lock()
	defer subscription.dispatchMu.Unlock()
	return c.sendLiveNotificationLocked(subscription, notification)
}

func (c *workspaceV2Connection) sendLiveNotificationLocked(subscription *workspaceV2Subscription, notification workspaceV2LiveNotification) error {
	c.stateMu.RLock()
	if c.subscription != subscription {
		c.stateMu.RUnlock()
		return nil
	}
	lastDelivered := subscription.lastDelivered
	index := subscription.nextEventIndex
	streamID := subscription.streamID
	c.stateMu.RUnlock()
	if notification.treeRevision != nil && *notification.treeRevision <= lastDelivered {
		return nil
	}
	var revision dto.WorkspaceRevision
	switch notification.kind {
	case workspaceV2LiveConflictCreated:
		if notification.conflict == nil {
			return errors.New("workspace live conflict is nil")
		}
		conflict, shouldSend, err := c.authoritativePendingConflict(notification)
		if err != nil {
			return err
		}
		if !shouldSend {
			return nil
		}
		generation := workspaceV2ConflictGeneration{conflictID: conflict.ConflictID, conflictRevision: conflict.ConflictRevision}
		c.stateMu.RLock()
		_, alreadyAuthoritative := subscription.authoritativeConflicts[generation]
		c.stateMu.RUnlock()
		if alreadyAuthoritative {
			return nil
		}
		if err := c.sendPush(dto.WorkspaceActionConflictCreated, conflict); err != nil {
			return err
		}
		c.stateMu.Lock()
		if c.subscription == subscription {
			for existing := range subscription.authoritativeConflicts {
				if existing.conflictID == generation.conflictID {
					delete(subscription.authoritativeConflicts, existing)
				}
			}
			subscription.authoritativeConflicts[generation] = struct{}{}
		}
		c.stateMu.Unlock()
	case workspaceV2LiveConflictResolved:
		if notification.resolved == nil {
			return errors.New("workspace live resolution is nil")
		}
		revision = notification.resolved.Revision
		if notification.treeRevision == nil || *notification.treeRevision != revision {
			return errors.New("workspace live resolution revision mismatch")
		}
		if err := c.sendPush(dto.WorkspaceActionConflictResolved, notification.resolved); err != nil {
			return err
		}
		c.stateMu.Lock()
		if c.subscription == subscription {
			for generation := range subscription.authoritativeConflicts {
				if generation.conflictID == notification.conflictID {
					delete(subscription.authoritativeConflicts, generation)
				}
			}
		}
		c.stateMu.Unlock()
	case workspaceV2LiveBlobNeed:
		if notification.blobNeed == nil {
			return errors.New("workspace live blob need is nil")
		}
		return c.sendPush(dto.WorkspaceActionBlobNeed, notification.blobNeed)
	case workspaceV2LiveEvent:
		if notification.accepted == nil || notification.treeRevision == nil || notification.mutation == nil {
			return errors.New("workspace live event is incomplete")
		}
		revision = *notification.treeRevision
		if notification.accepted.Revision != revision {
			return errors.New("workspace live event revision mismatch")
		}
		event := dto.WorkspaceEventMessage{
			WorkspaceID: notification.accepted.WorkspaceID, StreamID: streamID, Index: index,
			Revision: revision, OperationID: notification.accepted.OperationID,
			OriginClientID: notification.accepted.ClientID, Mutation: *notification.mutation,
			PathState: notification.accepted.PathState, OldPathState: notification.accepted.OldPathState,
			NewPathState: notification.accepted.NewPathState,
		}
		previousIndex := ^uint32(0)
		if index > 0 {
			previousIndex = index - 1
		}
		if err := event.Validate(previousIndex, lastDelivered); err != nil {
			return err
		}
		if err := c.sendPush(dto.WorkspaceActionEvent, &event); err != nil {
			return err
		}
	default:
		return errors.New("workspace live notification kind is invalid")
	}
	if revision != 0 {
		c.stateMu.Lock()
		if c.subscription == subscription {
			if notification.kind == workspaceV2LiveEvent {
				subscription.nextEventIndex = index + 1
			}
			if revision > subscription.lastDelivered {
				subscription.lastDelivered = revision
				// Live revisions become ackable as soon as they are sent. The
				// snapshot end establishes the initial boundary, but leaving
				// ackableRevision there would reject the client's next ack after
				// a live mutation even though the event was delivered.
				subscription.ackableRevision = revision
			}
		}
		c.stateMu.Unlock()
	}
	return nil
}

func (c *workspaceV2Connection) authoritativePendingConflict(notification workspaceV2LiveNotification) (*dto.WorkspaceConflictCreatedMessage, bool, error) {
	if notification.conflict == nil {
		return nil, false, errors.New("workspace live conflict is nil")
	}
	if c == nil || c.server == nil || c.server.syncService == nil {
		return notification.conflict, true, nil
	}
	authority, ok := c.server.syncService.(workspaceV2PendingConflictAuthority)
	if !ok {
		return notification.conflict, true, nil
	}
	current, err := authority.CurrentPendingConflict(c.ctx, c.uid, notification.conflict.WorkspaceID, notification.conflict.ConflictID)
	if err != nil {
		return nil, false, err
	}
	if current == nil {
		return nil, false, nil
	}
	if current.ConflictID != notification.conflict.ConflictID {
		return nil, false, errors.New("workspace authoritative conflict identity mismatch")
	}
	return current, true, nil
}

func checkedWorkspaceV2DeliveredCount(begin dto.WorkspaceSnapshotBeginMessage) (uint32, error) {
	base := uint64(begin.EntryCount)
	if begin.Mode == dto.WorkspaceSnapshotIncremental {
		base = uint64(begin.EventCount)
	}
	total := base + uint64(begin.ConflictCount)
	if total > uint64(^uint32(0)) {
		return 0, errors.New("workspace delivered count overflow")
	}
	return uint32(total), nil
}

func (c *workspaceV2Connection) sendPush(action dto.WorkspaceV2Action, data any) error {
	var payload []byte
	var err error
	switch value := data.(type) {
	case *dto.WorkspaceSnapshotBeginMessage:
		payload, err = encodeWorkspaceV2Push(action, value)
	case *dto.WorkspaceSnapshotEntryMessage:
		payload, err = encodeWorkspaceV2Push(action, value)
	case *dto.WorkspaceSnapshotEndMessage:
		payload, err = encodeWorkspaceV2Push(action, value)
	case *dto.WorkspaceEventMessage:
		payload, err = encodeWorkspaceV2Push(action, value)
	case *dto.WorkspaceConflictCreatedMessage:
		payload, err = encodeWorkspaceV2Push(action, value)
	case *dto.WorkspaceConflictResolvedMessage:
		payload, err = encodeWorkspaceV2Push(action, value)
	case *dto.WorkspaceBlobNeedUploadPush:
		payload, err = encodeWorkspaceV2Push(action, value)
	case *dto.WorkspaceBlobBeginMessage:
		payload, err = encodeWorkspaceV2Push(action, value)
	case *dto.WorkspaceBlobEndMessage:
		payload, err = encodeWorkspaceV2Push(action, value)
	default:
		return errors.New("workspace push data type is not registered")
	}
	if err != nil {
		return err
	}
	return c.sendText(payload)
}

func (c *workspaceV2Connection) sendText(payload []byte) error {
	return c.send(1, payload)
}

func (c *workspaceV2Connection) writeFailure(action dto.WorkspaceV2Action, requestID *dto.WorkspaceUUID, errorCode dto.WorkspaceV2ErrorCode, fields ...dto.WorkspaceV2FieldError) error {
	payload, err := encodeWorkspaceV2Failure(action, requestID, errorCode, fields...)
	if err != nil {
		return err
	}
	return c.sendText(payload)
}

func (h *workspaceV2Hub) register(connection *workspaceV2Connection, key workspaceV2HubKey) {
	if h == nil || connection == nil {
		return
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.subscribers == nil {
		h.subscribers = make(map[workspaceV2HubKey]map[*workspaceV2Connection]struct{})
	}
	if h.subscribers[key] == nil {
		h.subscribers[key] = make(map[*workspaceV2Connection]struct{})
	}
	h.subscribers[key][connection] = struct{}{}
}

func (h *workspaceV2Hub) unregister(connection *workspaceV2Connection, key workspaceV2HubKey) {
	if h == nil || connection == nil {
		return
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if subscribers := h.subscribers[key]; subscribers != nil {
		delete(subscribers, connection)
		if len(subscribers) == 0 {
			delete(h.subscribers, key)
		}
	}
}

func (h *workspaceV2Hub) publish(key workspaceV2HubKey, notification workspaceV2LiveNotification) {
	if h == nil {
		return
	}
	h.mu.Lock()
	subscribers := make([]*workspaceV2Connection, 0, len(h.subscribers[key]))
	for connection := range h.subscribers[key] {
		subscribers = append(subscribers, connection)
	}
	h.mu.Unlock()
	notification.workspaceID = key.workspaceID
	for _, connection := range subscribers {
		connection.bufferLiveNotification(notification)
	}
}

func (c *workspaceV2Connection) bufferLiveNotification(notification workspaceV2LiveNotification) {
	c.stateMu.Lock()
	subscription := c.subscription
	if subscription == nil {
		c.stateMu.Unlock()
		return
	}
	if notification.workspaceID != "" && notification.workspaceID != subscription.workspaceID {
		c.stateMu.Unlock()
		return
	}
	if notification.recipient != nil && notification.recipient != c {
		c.stateMu.Unlock()
		return
	}
	if notification.kind == workspaceV2LiveConflictCreated && !subscription.streaming && !subscription.flushing {
		if notification.conflict != nil {
			_, alreadyAuthoritative := subscription.authoritativeConflicts[workspaceV2ConflictGeneration{
				conflictID: notification.conflictID, conflictRevision: notification.conflict.ConflictRevision,
			}]
			if alreadyAuthoritative {
				c.stateMu.Unlock()
				return
			}
		}
	}
	if subscription.streaming || subscription.flushing {
		if notification.kind == workspaceV2LiveConflictCreated || notification.kind == workspaceV2LiveConflictResolved {
			if notification.conflictID == "" {
				c.stateMu.Unlock()
				return
			}
			buffer := subscription.pendingConflicts[notification.conflictID]
			if buffer == nil {
				if len(subscription.pendingRevisionItems)+len(subscription.pendingConflicts) >= workspaceV2LiveBacklogDepth {
					subscription.overflowed = true
					c.stateMu.Unlock()
					return
				}
				buffer = &workspaceV2ConflictBuffer{}
				subscription.pendingConflicts[notification.conflictID] = buffer
			}
			if notification.kind == workspaceV2LiveConflictCreated {
				buffer.created = &notification
			} else {
				buffer.resolved = &notification
			}
			if notification.kind == workspaceV2LiveConflictResolved && notification.treeRevision != nil {
				subscription.pendingRevisionItems[*notification.treeRevision] = append(subscription.pendingRevisionItems[*notification.treeRevision], notification)
			}
			c.stateMu.Unlock()
			return
		}
		if notification.kind == workspaceV2LiveBlobNeed {
			if len(subscription.pendingRevisionItems)+len(subscription.pendingConflicts)+len(subscription.pendingPushes) >= workspaceV2LiveBacklogDepth {
				subscription.overflowed = true
				c.stateMu.Unlock()
				return
			}
			subscription.pendingPushes = append(subscription.pendingPushes, notification)
			c.stateMu.Unlock()
			return
		}
		if notification.treeRevision != nil {
			if _, exists := subscription.pendingRevisionItems[*notification.treeRevision]; !exists &&
				len(subscription.pendingRevisionItems)+len(subscription.pendingConflicts) >= workspaceV2LiveBacklogDepth {
				subscription.overflowed = true
				c.stateMu.Unlock()
				return
			}
			subscription.pendingRevisionItems[*notification.treeRevision] = append(subscription.pendingRevisionItems[*notification.treeRevision], notification)
		}
		c.stateMu.Unlock()
		return
	}
	if notification.treeRevision != nil {
		lastDelivered := subscription.lastDelivered
		revision := *notification.treeRevision
		if revision <= lastDelivered {
			c.stateMu.Unlock()
			return
		}
		if subscription.pendingOrdered == nil {
			subscription.pendingOrdered = make(map[dto.WorkspaceRevision][]workspaceV2LiveNotification)
		}
		if lastDelivered == ^dto.WorkspaceRevision(0) || revision > lastDelivered+1 {
			if len(subscription.pendingOrdered) >= workspaceV2LiveBacklogDepth {
				subscription.overflowed = true
				c.stateMu.Unlock()
				c.closeWithCode(1013, "server_busy")
				return
			}
			subscription.pendingOrdered[revision] = append(subscription.pendingOrdered[revision], notification)
			c.stateMu.Unlock()
			return
		}
		ready := []workspaceV2LiveNotification{notification}
		for next := revision + 1; next > revision; next++ {
			queued := subscription.pendingOrdered[next]
			if len(queued) == 0 {
				break
			}
			delete(subscription.pendingOrdered, next)
			ready = append(ready, queued...)
		}
		c.stateMu.Unlock()
		if err := c.sendOrderedLiveNotifications(subscription, ready); err != nil {
			c.closeWithCode(1011, "internal")
		}
		return
	}
	c.stateMu.Unlock()
	if err := c.sendLiveNotification(subscription, notification); err != nil {
		c.closeWithCode(1011, "internal")
	}
}

func (c *workspaceV2Connection) sendOrderedLiveNotifications(
	subscription *workspaceV2Subscription,
	notifications []workspaceV2LiveNotification,
) error {
	subscription.dispatchMu.Lock()
	defer subscription.dispatchMu.Unlock()
	return c.sendOrderedLiveNotificationsLocked(subscription, notifications)
}

func (c *workspaceV2Connection) sendOrderedLiveNotificationsLocked(
	subscription *workspaceV2Subscription,
	notifications []workspaceV2LiveNotification,
) error {
	for _, notification := range notifications {
		if err := c.sendLiveNotificationLocked(subscription, notification); err != nil {
			return err
		}
	}
	for {
		c.stateMu.Lock()
		if c.subscription != subscription || subscription.lastDelivered == ^dto.WorkspaceRevision(0) {
			c.stateMu.Unlock()
			return nil
		}
		nextRevision := subscription.lastDelivered + 1
		queued, exists := subscription.pendingOrdered[nextRevision]
		if exists {
			delete(subscription.pendingOrdered, nextRevision)
		}
		c.stateMu.Unlock()
		if !exists {
			return nil
		}
		for _, notification := range queued {
			if err := c.sendLiveNotificationLocked(subscription, notification); err != nil {
				return err
			}
		}
	}
}

func workspaceV2FieldFromError(err error) dto.WorkspaceV2FieldError {
	var validationErr *dto.WorkspaceValidationError
	if errors.As(err, &validationErr) {
		return dto.WorkspaceV2FieldError{Field: validationErr.Field, Reason: validationErr.Reason}
	}
	return dto.WorkspaceV2FieldError{Field: "data", Reason: "invalid"}
}

func workspaceV2ServiceErrorCode(err error) dto.WorkspaceV2ErrorCode {
	var accessErr *WorkspaceV2AccessError
	if errors.As(err, &accessErr) {
		switch accessErr.Code {
		case "forbidden":
			return dto.WorkspaceErrorForbidden
		case "invalid_path":
			return dto.WorkspaceErrorInvalidPath
		}
	}
	var serviceErr *service.WorkspaceServiceError
	if errors.As(err, &serviceErr) && serviceErr.Code != "" {
		return serviceErr.Code
	}
	return dto.WorkspaceErrorInternal
}
