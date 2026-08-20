package websocket_router

import (
	"context"
	"encoding/hex"
	"errors"
	"io"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/service"
	"github.com/lxzan/gws"
	"github.com/zeebo/blake3"
)

var (
	errWorkspaceV2DownloadHashMismatch = errors.New("workspace download hash mismatch")
	errWorkspaceV2DownloadSizeMismatch = errors.New("workspace download size mismatch")
	errWorkspaceV2ReceiptRegistration  = errors.New("workspace transfer receipt registration failed")
)

const (
	workspaceV2MaxCompletedTransferReceipts = 4096
	workspaceV2CompletedTransferReceiptTTL  = 30 * time.Minute
)

type workspaceV2CompletedTransferReceipt struct {
	workspaceID dto.WorkspaceUUID
	transferID  uuid.UUID
	direction   dto.WorkspaceBlobDirection
	contentHash dto.WorkspaceContentHash
	size        uint64
	chunkCount  uint64
	expiresAt   time.Time
}

type workspaceV2TransferKey struct {
	uid        int64
	transferID uuid.UUID
}

type workspaceV2CompletedTransferRegistry struct {
	mu       sync.Mutex
	receipts map[workspaceV2TransferKey]workspaceV2CompletedTransferReceipt
	order    []workspaceV2TransferKey
}

func newWorkspaceV2CompletedTransferReceipt(end dto.WorkspaceBlobEndMessage, expiresAt time.Time) workspaceV2CompletedTransferReceipt {
	return workspaceV2CompletedTransferReceipt{
		workspaceID: end.WorkspaceID,
		transferID:  uuid.MustParse(string(end.TransferID)),
		direction:   end.Direction,
		contentHash: end.ContentHash,
		size:        end.Size,
		chunkCount:  end.ChunkCount,
		expiresAt:   expiresAt,
	}
}

func (r workspaceV2CompletedTransferReceipt) matches(end dto.WorkspaceBlobEndMessage) bool {
	return r.workspaceID == end.WorkspaceID && r.transferID.String() == string(end.TransferID) &&
		r.direction == end.Direction && r.contentHash == end.ContentHash && r.size == end.Size && r.chunkCount == end.ChunkCount
}

func (r *workspaceV2CompletedTransferRegistry) completed(uid int64, transferID uuid.UUID, now time.Time) (workspaceV2CompletedTransferReceipt, bool) {
	if r == nil || uid <= 0 || transferID == uuid.Nil {
		return workspaceV2CompletedTransferReceipt{}, false
	}
	r.mu.Lock()
	key := workspaceV2TransferKey{uid: uid, transferID: transferID}
	receipt, ok := r.receipts[key]
	if ok && !now.Before(receipt.expiresAt) {
		r.expireLocked(now)
		receipt, ok = r.receipts[key]
	}
	r.mu.Unlock()
	return receipt, ok
}

func (r *workspaceV2CompletedTransferRegistry) record(uid int64, end dto.WorkspaceBlobEndMessage, now time.Time) bool {
	if r == nil || uid <= 0 {
		return false
	}
	transferID, err := uuid.Parse(string(end.TransferID))
	if err != nil {
		return false
	}
	key := workspaceV2TransferKey{uid: uid, transferID: transferID}
	receipt := newWorkspaceV2CompletedTransferReceipt(end, now.Add(workspaceV2CompletedTransferReceiptTTL))
	r.mu.Lock()
	defer r.mu.Unlock()
	if existing, ok := r.receipts[key]; ok {
		if !now.Before(existing.expiresAt) {
			r.expireLocked(now)
		} else {
			return existing.matches(end)
		}
	}
	if r.receipts == nil {
		r.receipts = make(map[workspaceV2TransferKey]workspaceV2CompletedTransferReceipt, workspaceV2MaxCompletedTransferReceipts)
	}
	for len(r.receipts) >= workspaceV2MaxCompletedTransferReceipts && len(r.order) > 0 {
		oldest := r.order[0]
		delete(r.receipts, oldest)
		r.order[0] = workspaceV2TransferKey{}
		r.order = r.order[1:]
	}
	r.receipts[key] = receipt
	r.order = append(r.order, key)
	return true
}

func (r *workspaceV2CompletedTransferRegistry) Expire(now time.Time) {
	if r == nil {
		return
	}
	r.mu.Lock()
	r.expireLocked(now)
	r.mu.Unlock()
}

func (r *workspaceV2CompletedTransferRegistry) expireLocked(now time.Time) {
	kept := r.order[:0]
	for _, key := range r.order {
		receipt, ok := r.receipts[key]
		if !ok {
			continue
		}
		if !now.Before(receipt.expiresAt) {
			delete(r.receipts, key)
			continue
		}
		kept = append(kept, key)
	}
	r.order = kept
}

func (c *workspaceV2Connection) handleBlobBegin(requestID dto.WorkspaceUUID, begin *dto.WorkspaceBlobBeginMessage) error {
	if begin == nil {
		return c.writeFailure(dto.WorkspaceActionBlobBegin, &requestID, dto.WorkspaceErrorInvalidRequest)
	}
	if err := begin.Validate(); err != nil {
		return c.writeFailure(dto.WorkspaceActionBlobBegin, &requestID, dto.WorkspaceErrorInvalidRequest, workspaceV2FieldFromError(err))
	}
	if begin.Direction != dto.WorkspaceBlobUpload {
		return c.writeFailure(dto.WorkspaceActionBlobBegin, &requestID, dto.WorkspaceErrorInvalidRequest,
			dto.WorkspaceV2FieldError{Field: "data.direction", Reason: "must_be_upload"})
	}
	if _, err := c.subscriptionForWorkspace(begin.WorkspaceID); err != nil {
		return c.writeFailure(dto.WorkspaceActionBlobBegin, &requestID, workspaceV2ServiceErrorCode(err))
	}
	if err := c.authorizeWorkspaceRequest(begin.WorkspaceID); err != nil {
		return c.writeFailure(dto.WorkspaceActionBlobBegin, &requestID, workspaceV2ServiceErrorCode(err))
	}
	if c.server.blobStore == nil {
		return c.writeFailure(dto.WorkspaceActionBlobBegin, &requestID, dto.WorkspaceErrorInternal)
	}
	transferID, err := uuid.Parse(string(begin.TransferID))
	if err != nil {
		return c.writeFailure(dto.WorkspaceActionBlobBegin, &requestID, dto.WorkspaceErrorInvalidRequest)
	}
	if active := c.workspaceV2Transfer(transferID); active != nil {
		if !active.matchesBegin(*begin) {
			return c.writeFailure(dto.WorkspaceActionBlobBegin, &requestID, dto.WorkspaceErrorBlobTransferOutOfOrder)
		}
		active.touch(time.Now().UTC())
		return sendWorkspaceV2Success(c, dto.WorkspaceActionBlobBegin, requestID, begin)
	}
	transferCtx, transferCancel := context.WithCancel(c.ctx)
	transfer := &workspaceV2Transfer{
		ctx: transferCtx, cancel: transferCancel,
		workspaceID: begin.WorkspaceID, transferID: transferID, direction: dto.WorkspaceBlobUpload,
		contentHash: begin.ContentHash, size: begin.Size, chunkSize: begin.ChunkSize, chunkCount: begin.ChunkCount,
		uploadDone: make(chan struct{}),
	}
	reader, writer := io.Pipe()
	transfer.uploadWriter = writer
	if err := c.addWorkspaceV2Transfer(transfer); err != nil {
		transferCancel()
		_ = writer.CloseWithError(err)
		_ = reader.CloseWithError(err)
		return c.writeFailure(dto.WorkspaceActionBlobBegin, &requestID, workspaceV2ErrorCodeForTransferError(err))
	}
	go c.runWorkspaceV2Upload(transfer, reader)
	if begin.Size == 0 {
		transfer.mu.Lock()
		closeErr := writer.Close()
		if closeErr == nil {
			transfer.uploadComplete = true
		}
		transfer.mu.Unlock()
		if closeErr != nil {
			return c.writeFailure(dto.WorkspaceActionBlobBegin, &requestID, dto.WorkspaceErrorInternal)
		}
	}
	if err := sendWorkspaceV2Success(c, dto.WorkspaceActionBlobBegin, requestID, begin); err != nil {
		c.removeWorkspaceV2Transfer(transferID)
		_ = writer.CloseWithError(err)
		return err
	}
	return nil
}

func (c *workspaceV2Connection) runWorkspaceV2Upload(transfer *workspaceV2Transfer, reader *io.PipeReader) {
	ctx := transfer.ctx
	if ctx == nil {
		ctx = c.ctx
	}
	err := c.server.blobStore.Put(ctx, c.uid, transfer.contentHash, transfer.size, reader)
	if err != nil {
		_ = reader.CloseWithError(err)
	} else {
		_ = reader.Close()
	}
	// A successful Put is not publishable until its receipt is registered. Cleanup
	// leaves the active identity owned by this worker until that mutation is settled.
	transfer.mu.Lock()
	if err == nil {
		end := transfer.endMessage()
		if c.server == nil || !c.server.completedTransfers.record(c.uid, end, time.Now().UTC()) {
			err = errWorkspaceV2ReceiptRegistration
		} else {
			transfer.receiptRecorded = true
		}
	}
	transfer.uploadErr = err
	close(transfer.uploadDone)
	cleanupStarted := transfer.cleanupStarted
	transfer.mu.Unlock()
	if cleanupStarted {
		c.releaseWorkspaceV2Transfer(transfer)
	}
}

func (c *workspaceV2Connection) handleBlobEnd(requestID dto.WorkspaceUUID, end *dto.WorkspaceBlobEndMessage) error {
	if end == nil {
		return c.writeFailure(dto.WorkspaceActionBlobEnd, &requestID, dto.WorkspaceErrorInvalidRequest)
	}
	if err := end.Validate(); err != nil {
		return c.writeFailure(dto.WorkspaceActionBlobEnd, &requestID, dto.WorkspaceErrorInvalidRequest, workspaceV2FieldFromError(err))
	}
	if _, err := c.subscriptionForWorkspace(end.WorkspaceID); err != nil {
		return c.writeFailure(dto.WorkspaceActionBlobEnd, &requestID, workspaceV2ServiceErrorCode(err))
	}
	if err := c.authorizeWorkspaceRequest(end.WorkspaceID); err != nil {
		return c.writeFailure(dto.WorkspaceActionBlobEnd, &requestID, workspaceV2ServiceErrorCode(err))
	}
	transferID, err := uuid.Parse(string(end.TransferID))
	if err != nil {
		return c.writeFailure(dto.WorkspaceActionBlobEnd, &requestID, dto.WorkspaceErrorInvalidRequest)
	}
	transfer := c.workspaceV2Transfer(transferID)
	if transfer == nil {
		if receipt, ok := c.workspaceV2CompletedTransfer(transferID); ok && receipt.matches(*end) {
			return sendWorkspaceV2Success(c, dto.WorkspaceActionBlobEnd, requestID, end)
		}
		return c.writeFailure(dto.WorkspaceActionBlobEnd, &requestID, dto.WorkspaceErrorBlobTransferOutOfOrder)
	}
	if !transfer.matchesEnd(*end) {
		return c.writeFailure(dto.WorkspaceActionBlobEnd, &requestID, dto.WorkspaceErrorBlobTransferOutOfOrder)
	}
	transfer.mu.Lock()
	direction := transfer.direction
	uploadComplete := transfer.uploadComplete
	downloadComplete := transfer.downloadComplete
	closing := transfer.closing
	transfer.mu.Unlock()
	if closing {
		return c.writeFailure(dto.WorkspaceActionBlobEnd, &requestID, dto.WorkspaceErrorBlobTransferOutOfOrder)
	}
	if direction == dto.WorkspaceBlobUpload {
		transfer.touch(time.Now().UTC())
		if !uploadComplete {
			return c.failWorkspaceV2Transfer(transferID, requestID, dto.WorkspaceErrorBlobTransferOutOfOrder)
		}
		<-transfer.uploadDone
		transfer.mu.Lock()
		uploadErr := transfer.uploadErr
		receiptRecorded := transfer.receiptRecorded
		closing = transfer.closing
		transfer.mu.Unlock()
		if uploadErr != nil {
			return c.failWorkspaceV2Transfer(transferID, requestID, workspaceV2ErrorCodeForTransferError(uploadErr))
		}
		if !receiptRecorded {
			return c.failWorkspaceV2Transfer(transferID, requestID, dto.WorkspaceErrorInternal)
		}
		if closing {
			if err := c.ctx.Err(); err != nil {
				return err
			}
			return c.writeFailure(dto.WorkspaceActionBlobEnd, &requestID, dto.WorkspaceErrorBlobTransferOutOfOrder)
		}
		c.removeWorkspaceV2Transfer(transferID)
		return sendWorkspaceV2Success(c, dto.WorkspaceActionBlobEnd, requestID, end)
	}
	if !downloadComplete {
		return c.failWorkspaceV2Transfer(transferID, requestID, dto.WorkspaceErrorBlobTransferOutOfOrder)
	}
	if !c.recordWorkspaceV2CompletedTransfer(*end) {
		c.removeWorkspaceV2Transfer(transferID)
		if err := c.ctx.Err(); err != nil {
			return err
		}
		return c.writeFailure(dto.WorkspaceActionBlobEnd, &requestID, dto.WorkspaceErrorBlobTransferOutOfOrder)
	}
	c.removeWorkspaceV2Transfer(transferID)
	return sendWorkspaceV2Success(c, dto.WorkspaceActionBlobEnd, requestID, end)
}

func (c *workspaceV2Connection) failWorkspaceV2Transfer(transferID uuid.UUID, requestID dto.WorkspaceUUID, errorCode dto.WorkspaceV2ErrorCode) error {
	c.removeWorkspaceV2Transfer(transferID)
	return c.writeFailure(dto.WorkspaceActionBlobEnd, &requestID, errorCode)
}

func (c *workspaceV2Connection) handleBlobNeedDownload(requestID dto.WorkspaceUUID, need *dto.WorkspaceBlobNeedDownloadRequest) error {
	if need == nil {
		return c.writeFailure(dto.WorkspaceActionBlobNeed, &requestID, dto.WorkspaceErrorInvalidRequest)
	}
	if err := need.Validate(); err != nil {
		return c.writeFailure(dto.WorkspaceActionBlobNeed, &requestID, dto.WorkspaceErrorInvalidRequest, workspaceV2FieldFromError(err))
	}
	if _, err := c.subscriptionForWorkspace(need.WorkspaceID); err != nil {
		return c.writeFailure(dto.WorkspaceActionBlobNeed, &requestID, workspaceV2ServiceErrorCode(err))
	}
	if err := c.authorizeWorkspaceRequest(need.WorkspaceID); err != nil {
		return c.writeFailure(dto.WorkspaceActionBlobNeed, &requestID, workspaceV2ServiceErrorCode(err))
	}
	if c.server.blobStore == nil {
		return c.writeFailure(dto.WorkspaceActionBlobNeed, &requestID, dto.WorkspaceErrorInternal)
	}
	reader, size, err := c.server.blobStore.Open(c.ctx, c.uid, need.ContentHash)
	if err != nil {
		return c.writeFailure(dto.WorkspaceActionBlobNeed, &requestID, workspaceV2ErrorCodeForBlobError(err))
	}
	if reader == nil {
		return c.writeFailure(dto.WorkspaceActionBlobNeed, &requestID, dto.WorkspaceErrorInternal)
	}
	if size > dto.WorkspaceMaxBlobBytes || (need.Size.Present && need.Size.Value != nil && *need.Size.Value != size) {
		_ = reader.Close()
		return c.writeFailure(dto.WorkspaceActionBlobNeed, &requestID, dto.WorkspaceErrorBlobSizeMismatch)
	}
	transferID := uuid.New()
	transferCtx, transferCancel := context.WithCancel(c.ctx)
	transfer := &workspaceV2Transfer{
		ctx: transferCtx, cancel: transferCancel,
		workspaceID: need.WorkspaceID, transferID: transferID, direction: dto.WorkspaceBlobDownload,
		contentHash: need.ContentHash, size: size, chunkSize: dto.WorkspaceBlobChunkSize,
		chunkCount: workspaceV2BlobChunkCount(size), download: reader,
	}
	if err := c.addWorkspaceV2Transfer(transfer); err != nil {
		transferCancel()
		_ = reader.Close()
		return c.writeFailure(dto.WorkspaceActionBlobNeed, &requestID, workspaceV2ErrorCodeForTransferError(err))
	}
	response := &dto.WorkspaceBlobNeedDownloadResponse{
		WorkspaceID: need.WorkspaceID, Direction: dto.WorkspaceBlobDownload,
		OperationID: dto.WorkspaceNullableUUID{Present: true}, ContentHash: need.ContentHash, Size: size,
	}
	if err := sendWorkspaceV2Success(c, dto.WorkspaceActionBlobNeed, requestID, response); err != nil {
		c.removeWorkspaceV2Transfer(transferID)
		return err
	}
	if err := c.streamWorkspaceV2Download(transfer); err != nil {
		c.removeWorkspaceV2Transfer(transferID)
		switch {
		case errors.Is(err, errWorkspaceV2DownloadHashMismatch):
			if writeErr := c.writeFailure(dto.WorkspaceActionBlobEnd, nil, dto.WorkspaceErrorBlobHashMismatch); writeErr != nil {
				return writeErr
			}
			return nil
		case errors.Is(err, errWorkspaceV2DownloadSizeMismatch):
			if writeErr := c.writeFailure(dto.WorkspaceActionBlobEnd, nil, dto.WorkspaceErrorBlobSizeMismatch); writeErr != nil {
				return writeErr
			}
			return nil
		}
		return err
	}
	return nil
}

func (c *workspaceV2Connection) streamWorkspaceV2Download(transfer *workspaceV2Transfer) error {
	begin := &dto.WorkspaceBlobBeginMessage{
		WorkspaceID: transfer.workspaceID, TransferID: dto.WorkspaceUUID(transfer.transferID.String()), Direction: transfer.direction,
		ContentHash: transfer.contentHash, Size: transfer.size, ChunkSize: dto.WorkspaceBlobChunkSize, ChunkCount: transfer.chunkCount,
	}
	if err := c.sendPush(dto.WorkspaceActionBlobBegin, begin); err != nil {
		return err
	}
	spool, err := os.CreateTemp("", "fast-note-workspace-v2-download-*")
	if err != nil {
		return err
	}
	spoolPath := spool.Name()
	defer func() {
		_ = spool.Close()
		_ = os.Remove(spoolPath)
	}()
	hasher := blake3.New()
	limited := io.LimitReader(&workspaceV2ContextReader{
		ctx:    transfer.ctx,
		reader: transfer.download,
		onRead: func() { transfer.touch(time.Now().UTC()) },
	}, int64(transfer.size)+1)
	written, err := io.Copy(io.MultiWriter(spool, hasher), limited)
	if err != nil {
		return err
	}
	if written != int64(transfer.size) {
		return errWorkspaceV2DownloadSizeMismatch
	}
	actualHash := dto.WorkspaceContentHash("blake3:" + hex.EncodeToString(hasher.Sum(nil)))
	if actualHash != transfer.contentHash {
		return errWorkspaceV2DownloadHashMismatch
	}
	if _, err := spool.Seek(0, io.SeekStart); err != nil {
		return err
	}
	remaining := transfer.size
	for index, offset := uint64(0), uint64(0); remaining > 0; index++ {
		chunkSize := uint64(dto.WorkspaceBlobChunkSize)
		if remaining < chunkSize {
			chunkSize = remaining
		}
		payload := make([]byte, chunkSize)
		if _, err := io.ReadFull(spool, payload); err != nil {
			return err
		}
		_, digest := dto.ComputeWorkspaceBlobDigest(payload)
		header, err := dto.MarshalWorkspaceBlobHeader(dto.WorkspaceBlobHeader{
			Direction: dto.WorkspaceBlobDownload, Final: remaining == chunkSize, TransferID: transfer.transferID,
			ChunkIndex: index, Offset: offset, PayloadLen: uint32(len(payload)), ChunkDigest: digest,
		})
		if err != nil {
			return err
		}
		frame := append(header[:], payload...)
		if err := c.send(gws.OpcodeBinary, frame); err != nil {
			return err
		}
		transfer.touch(time.Now().UTC())
		offset += chunkSize
		remaining -= chunkSize
	}
	end := &dto.WorkspaceBlobEndMessage{
		WorkspaceID: transfer.workspaceID, TransferID: dto.WorkspaceUUID(transfer.transferID.String()), Direction: transfer.direction,
		ContentHash: transfer.contentHash, Size: transfer.size, ChunkCount: transfer.chunkCount,
	}
	if err := c.sendPush(dto.WorkspaceActionBlobEnd, end); err != nil {
		return err
	}
	transfer.mu.Lock()
	transfer.downloadComplete = true
	transfer.mu.Unlock()
	return nil
}

type workspaceV2ContextReader struct {
	ctx    context.Context
	reader io.Reader
	onRead func()
}

func (r *workspaceV2ContextReader) Read(p []byte) (int, error) {
	if err := r.ctx.Err(); err != nil {
		return 0, err
	}
	n, err := r.reader.Read(p)
	if n > 0 && r.onRead != nil {
		r.onRead()
	}
	if err == nil {
		if ctxErr := r.ctx.Err(); ctxErr != nil {
			return n, ctxErr
		}
	}
	return n, err
}

func (c *workspaceV2Connection) handleWorkspaceV2BinaryFrame(frame []byte) error {
	if len(frame) <= dto.WorkspaceBlobHeaderSize {
		return errors.New("workspace binary frame is too short")
	}
	transferID, err := uuid.FromBytes(frame[8:24])
	if err != nil {
		return err
	}
	transfer := c.workspaceV2Transfer(transferID)
	if transfer == nil || transfer.direction != dto.WorkspaceBlobUpload {
		return errors.New("workspace upload transfer is unknown")
	}
	transfer.mu.Lock()
	if transfer.closing || transfer.uploadInFlight {
		transfer.mu.Unlock()
		return errors.New("workspace upload transfer is closing")
	}
	payload := frame[dto.WorkspaceBlobHeaderSize:]
	_, digest := dto.ComputeWorkspaceBlobDigest(payload)
	header, err := dto.UnmarshalWorkspaceBlobHeader(frame[:dto.WorkspaceBlobHeaderSize], uint32(len(payload)), digest)
	if err != nil {
		transfer.mu.Unlock()
		return err
	}
	if header.Direction != transfer.direction {
		transfer.mu.Unlock()
		return errors.New("workspace binary direction mismatch")
	}
	if transfer.nextOffset > transfer.size || uint64(len(payload)) > transfer.size-transfer.nextOffset {
		transfer.mu.Unlock()
		return errors.New("workspace binary size exceeds transfer")
	}
	isLast := transfer.nextOffset+uint64(len(payload)) == transfer.size
	if err := header.ValidateSequence(transfer.nextChunkIndex, transfer.nextOffset, isLast); err != nil {
		transfer.mu.Unlock()
		return err
	}
	writer := transfer.uploadWriter
	if writer == nil {
		transfer.mu.Unlock()
		return errors.New("workspace upload writer is unavailable")
	}
	transfer.uploadInFlight = true
	transfer.mu.Unlock()

	_, writeErr := writer.Write(payload)
	transfer.mu.Lock()
	transfer.uploadInFlight = false
	defer transfer.mu.Unlock()
	if writeErr != nil {
		return writeErr
	}
	if transfer.closing {
		return context.Canceled
	}
	transfer.lastActivity = time.Now().UTC()
	transfer.nextChunkIndex++
	transfer.nextOffset += uint64(len(payload))
	if isLast {
		if err := transfer.uploadWriter.Close(); err != nil {
			return err
		}
		transfer.uploadComplete = true
	}
	return nil
}

func (c *workspaceV2Connection) addWorkspaceV2Transfer(transfer *workspaceV2Transfer) error {
	if c.server == nil || c.server.transfers == nil {
		return errors.New("workspace transfer manager unavailable")
	}
	return c.server.reserveWorkspaceV2Transfer(c, transfer)
}

func (s *WorkspaceV2Server) reserveWorkspaceV2Transfer(c *workspaceV2Connection, transfer *workspaceV2Transfer) error {
	if s == nil || s.transfers == nil || c == nil || transfer == nil {
		return errors.New("workspace transfer manager unavailable")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		return context.Canceled
	}
	now := time.Now().UTC()
	key := workspaceV2TransferKey{uid: c.uid, transferID: transfer.transferID}
	s.completedTransfers.mu.Lock()
	if receipt, ok := s.completedTransfers.receipts[key]; ok {
		if !now.Before(receipt.expiresAt) {
			s.completedTransfers.expireLocked(now)
		} else {
			s.completedTransfers.mu.Unlock()
			return errWorkspaceV2TransferIdentifierReused
		}
	}
	transfer.lifecycleOwner = s
	err := s.transfers.reserve(c, transfer)
	s.completedTransfers.mu.Unlock()
	if err != nil {
		transfer.lifecycleOwner = nil
		return err
	}
	s.activeTransfers++
	return err
}

func (c *workspaceV2Connection) workspaceV2Transfer(transferID uuid.UUID) *workspaceV2Transfer {
	c.stateMu.RLock()
	transfer := c.transfers[transferID]
	c.stateMu.RUnlock()
	return transfer
}

func (transfer *workspaceV2Transfer) matchesBegin(begin dto.WorkspaceBlobBeginMessage) bool {
	if transfer == nil {
		return false
	}
	transfer.mu.Lock()
	defer transfer.mu.Unlock()
	return !transfer.closing && transfer.workspaceID == begin.WorkspaceID &&
		transfer.transferID.String() == string(begin.TransferID) && transfer.direction == begin.Direction &&
		transfer.contentHash == begin.ContentHash && transfer.size == begin.Size &&
		transfer.chunkSize == begin.ChunkSize && transfer.chunkCount == begin.ChunkCount
}

func (transfer *workspaceV2Transfer) matchesEnd(end dto.WorkspaceBlobEndMessage) bool {
	if transfer == nil {
		return false
	}
	transfer.mu.Lock()
	defer transfer.mu.Unlock()
	return transfer.workspaceID == end.WorkspaceID && transfer.transferID.String() == string(end.TransferID) &&
		transfer.direction == end.Direction && transfer.contentHash == end.ContentHash &&
		transfer.size == end.Size && transfer.chunkCount == end.ChunkCount
}

func (transfer *workspaceV2Transfer) matchesTransferIdentity(other *workspaceV2Transfer) bool {
	if transfer == nil || other == nil {
		return false
	}
	return transfer.workspaceID == other.workspaceID && transfer.direction == other.direction &&
		transfer.contentHash == other.contentHash && transfer.size == other.size && transfer.chunkCount == other.chunkCount
}

func (transfer *workspaceV2Transfer) endMessage() dto.WorkspaceBlobEndMessage {
	return dto.WorkspaceBlobEndMessage{
		WorkspaceID: transfer.workspaceID,
		TransferID:  dto.WorkspaceUUID(transfer.transferID.String()),
		Direction:   transfer.direction,
		ContentHash: transfer.contentHash,
		Size:        transfer.size,
		ChunkCount:  transfer.chunkCount,
	}
}

func (c *workspaceV2Connection) workspaceV2CompletedTransfer(transferID uuid.UUID) (workspaceV2CompletedTransferReceipt, bool) {
	if c == nil || c.server == nil {
		return workspaceV2CompletedTransferReceipt{}, false
	}
	return c.server.completedTransfers.completed(c.uid, transferID, time.Now().UTC())
}

func (c *workspaceV2Connection) recordWorkspaceV2CompletedTransfer(end dto.WorkspaceBlobEndMessage) bool {
	if c == nil || c.server == nil {
		return false
	}
	return c.server.completedTransfers.record(c.uid, end, time.Now().UTC())
}

func (c *workspaceV2Connection) removeWorkspaceV2Transfer(transferID uuid.UUID) bool {
	c.stateMu.Lock()
	transfer := c.transfers[transferID]
	c.stateMu.Unlock()
	if transfer == nil {
		return false
	}
	uploadWriter, uploadComplete, download, cancel, uploadDone, cleanupStarted := transfer.beginCleanup()
	if cleanupStarted {
		if cancel != nil {
			cancel()
		}
		if uploadWriter != nil && !uploadComplete {
			_ = uploadWriter.CloseWithError(context.Canceled)
		}
		if download != nil {
			_ = download.Close()
		}
	}
	if uploadDone != nil {
		select {
		case <-uploadDone:
		default:
			return true
		}
	}
	c.releaseWorkspaceV2Transfer(transfer)
	return true
}

func (c *workspaceV2Connection) releaseWorkspaceV2Transfer(transfer *workspaceV2Transfer) bool {
	if c == nil || transfer == nil {
		return false
	}
	released := false
	if c.server != nil && c.server.transfers != nil {
		released = c.server.transfers.release(transfer)
	} else if transfer.manager != nil {
		released = transfer.manager.release(transfer)
	} else {
		c.stateMu.Lock()
		delete(c.transfers, transfer.transferID)
		c.stateMu.Unlock()
		released = true
	}
	if released && (c.server == nil || c.server.transfers == nil) {
		c.finishCleanup()
		if lifecycleOwner := transfer.lifecycleOwner; lifecycleOwner != nil {
			lifecycleOwner.finishTransfer()
		}
	}
	return released
}

func (c *workspaceV2Connection) cleanupTransfers() {
	c.stateMu.Lock()
	transferIDs := make([]uuid.UUID, 0, len(c.transfers))
	for transferID := range c.transfers {
		transferIDs = append(transferIDs, transferID)
	}
	c.stateMu.Unlock()
	for _, transferID := range transferIDs {
		c.removeWorkspaceV2Transfer(transferID)
	}
}

func workspaceV2BlobChunkCount(size uint64) uint64 {
	if size == 0 {
		return 0
	}
	return (size-1)/uint64(dto.WorkspaceBlobChunkSize) + 1
}

func workspaceV2ErrorCodeForTransferError(err error) dto.WorkspaceV2ErrorCode {
	if err == nil {
		return dto.WorkspaceErrorInternal
	}
	if strings.Contains(err.Error(), "limit exceeded") {
		return dto.WorkspaceErrorBlobLimitExceeded
	}
	if strings.Contains(err.Error(), "reused") {
		return dto.WorkspaceErrorBlobTransferOutOfOrder
	}
	if errors.Is(err, errWorkspaceV2TransferAlreadyActive) {
		return dto.WorkspaceErrorBlobTransferOutOfOrder
	}
	if strings.Contains(err.Error(), "size mismatch") {
		return dto.WorkspaceErrorBlobSizeMismatch
	}
	if strings.Contains(err.Error(), "hash mismatch") {
		return dto.WorkspaceErrorBlobHashMismatch
	}
	return dto.WorkspaceErrorInternal
}

func workspaceV2ErrorCodeForBinaryFrameError(err error) dto.WorkspaceV2ErrorCode {
	var validationErr *dto.WorkspaceValidationError
	if errors.As(err, &validationErr) {
		switch validationErr.Field {
		case "chunkDigest":
			return dto.WorkspaceErrorBlobHashMismatch
		case "payloadLength":
			if validationErr.Reason == "frame_mismatch" || validationErr.Reason == "limit_exceeded" {
				return dto.WorkspaceErrorBlobSizeMismatch
			}
			return dto.WorkspaceErrorBlobTransferOutOfOrder
		case "chunkIndex", "offset", "final", "direction":
			return dto.WorkspaceErrorBlobTransferOutOfOrder
		}
	}
	if strings.Contains(errString(err), "size exceeds") {
		return dto.WorkspaceErrorBlobSizeMismatch
	}
	if strings.Contains(errString(err), "hash mismatch") || strings.Contains(errString(err), "digest") {
		return dto.WorkspaceErrorBlobHashMismatch
	}
	return dto.WorkspaceErrorBlobTransferOutOfOrder
}

func errString(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func workspaceV2ErrorCodeForBlobError(err error) dto.WorkspaceV2ErrorCode {
	if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
		return dto.WorkspaceErrorBlobNotFound
	}
	if errors.Is(err, service.ErrWorkspaceBlobIntegrity) {
		return dto.WorkspaceErrorInternal
	}
	return workspaceV2ErrorCodeForTransferError(err)
}
