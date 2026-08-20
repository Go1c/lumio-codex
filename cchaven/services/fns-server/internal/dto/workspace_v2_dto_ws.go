package dto

import (
	"bytes"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"reflect"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/google/uuid"
	"github.com/zeebo/blake3"
	"golang.org/x/text/unicode/norm"
)

const (
	WorkspaceBlobHeaderSize              = 64
	WorkspaceMaxControlFrameBytes        = 65_536
	WorkspaceBlobChunkSize               = 1_048_576
	WorkspaceMaxBlobBytes         uint64 = 5_368_709_120
)

type WorkspaceValidationError struct {
	Field  string
	Reason string
}

func (e *WorkspaceValidationError) Error() string {
	return e.Field + ": " + e.Reason
}

type WorkspaceUUID string
type WorkspaceRevision uint64
type WorkspaceConflictRevision struct {
	value uint64
}
type WorkspaceContentHash string
type WorkspacePath string

type WorkspaceNullableHash struct {
	Present bool
	Value   *WorkspaceContentHash
}

type WorkspaceNullableUUID struct {
	Present bool
	Value   *WorkspaceUUID
}

type WorkspaceNullableUint64 struct {
	Present bool
	Value   *uint64
}

type WorkspaceFileMetadata struct {
	Size         uint64 `json:"size"`
	ModifiedAtMS int64  `json:"modifiedAtMs"`
	Executable   bool   `json:"executable"`
}

type WorkspaceEntryKind string

const (
	WorkspaceEntryFile      WorkspaceEntryKind = "file"
	WorkspaceEntryDirectory WorkspaceEntryKind = "directory"
	WorkspaceEntrySymlink   WorkspaceEntryKind = "symlink"
	WorkspaceEntryTombstone WorkspaceEntryKind = "tombstone"
)

type WorkspaceMutationKind string

const (
	WorkspaceMutationUpsertFile    WorkspaceMutationKind = "upsert_file"
	WorkspaceMutationMkdir         WorkspaceMutationKind = "mkdir"
	WorkspaceMutationUpsertSymlink WorkspaceMutationKind = "upsert_symlink"
	WorkspaceMutationDelete        WorkspaceMutationKind = "delete"
	WorkspaceMutationRename        WorkspaceMutationKind = "rename"
)

func ParseWorkspaceRevision(s string) (WorkspaceRevision, error) {
	if s == "" {
		return 0, validationError("revision", "empty")
	}
	v, err := strconv.ParseUint(s, 10, 64)
	if err != nil || strconv.FormatUint(v, 10) != s {
		return 0, validationError("revision", "non_canonical_decimal")
	}
	return WorkspaceRevision(v), nil
}

func (r WorkspaceRevision) MarshalJSON() ([]byte, error) {
	return json.Marshal(strconv.FormatUint(uint64(r), 10))
}

func (r *WorkspaceRevision) UnmarshalJSON(data []byte) error {
	var s string
	if err := json.Unmarshal(data, &s); err != nil {
		return validationError("revision", "must_be_string")
	}
	v, err := ParseWorkspaceRevision(s)
	if err != nil {
		return err
	}
	*r = v
	return nil
}

func ParseWorkspaceConflictRevision(s string) (WorkspaceConflictRevision, error) {
	if s == "" {
		return WorkspaceConflictRevision{}, validationError("conflictRevision", "empty")
	}
	v, err := strconv.ParseUint(s, 10, 64)
	if err != nil || strconv.FormatUint(v, 10) != s {
		return WorkspaceConflictRevision{}, validationError("conflictRevision", "non_canonical_decimal")
	}
	if v == 0 {
		return WorkspaceConflictRevision{}, validationError("conflictRevision", "must_be_positive")
	}
	return WorkspaceConflictRevision{value: v}, nil
}

func (r WorkspaceConflictRevision) MarshalJSON() ([]byte, error) {
	if r.value == 0 {
		return nil, validationError("conflictRevision", "must_be_positive")
	}
	return json.Marshal(strconv.FormatUint(r.value, 10))
}

func (r *WorkspaceConflictRevision) UnmarshalJSON(data []byte) error {
	var s string
	if err := json.Unmarshal(data, &s); err != nil {
		return validationError("conflictRevision", "must_be_string")
	}
	v, err := ParseWorkspaceConflictRevision(s)
	if err != nil {
		return err
	}
	*r = v
	return nil
}

func ParseWorkspaceContentHash(s string) (WorkspaceContentHash, error) {
	if len(s) != len("blake3:")+64 || !strings.HasPrefix(s, "blake3:") {
		return "", validationError("contentHash", "invalid_blake3")
	}
	if _, err := hex.DecodeString(s[len("blake3:"):]); err != nil || strings.ToLower(s) != s {
		return "", validationError("contentHash", "invalid_blake3")
	}
	return WorkspaceContentHash(s), nil
}

func (h WorkspaceContentHash) MarshalJSON() ([]byte, error) {
	if _, err := ParseWorkspaceContentHash(string(h)); err != nil {
		return nil, err
	}
	return json.Marshal(string(h))
}

func (h *WorkspaceContentHash) UnmarshalJSON(data []byte) error {
	var s string
	if err := json.Unmarshal(data, &s); err != nil {
		return validationError("contentHash", "must_be_string")
	}
	v, err := ParseWorkspaceContentHash(s)
	if err != nil {
		return err
	}
	*h = v
	return nil
}

func ParseWorkspacePath(s string) (WorkspacePath, error) {
	fail := func(reason string) (WorkspacePath, error) {
		return "", validationError("path", reason)
	}
	if s == "" || len([]byte(s)) > 4096 || !utf8.ValidString(s) {
		return fail("invalid_length_or_utf8")
	}
	if norm.NFC.String(s) != s {
		return fail("not_nfc")
	}
	if strings.HasPrefix(s, "/") || strings.HasSuffix(s, "/") || strings.Contains(s, "//") || strings.ContainsRune(s, '\\') {
		return fail("not_relative_posix")
	}
	for _, segment := range strings.Split(s, "/") {
		if segment == "" || segment == "." || segment == ".." {
			return fail("invalid_segment")
		}
		if strings.HasSuffix(segment, ".") || strings.HasSuffix(segment, " ") {
			return fail("windows_unsafe_suffix")
		}
		for _, r := range segment {
			if r <= 0x1f || (r >= 0x7f && r <= 0x9f) || strings.ContainsRune(`<>:"|?*`, r) {
				return fail("unsafe_character")
			}
		}
		base := strings.ToUpper(strings.SplitN(segment, ".", 2)[0])
		if base == "CON" || base == "PRN" || base == "AUX" || base == "NUL" ||
			(len(base) == 4 && ((strings.HasPrefix(base, "COM") || strings.HasPrefix(base, "LPT")) && base[3] >= '1' && base[3] <= '9')) {
			return fail("windows_device_name")
		}
	}
	return WorkspacePath(s), nil
}

func (p WorkspacePath) MarshalJSON() ([]byte, error) {
	if _, err := ParseWorkspacePath(string(p)); err != nil {
		return nil, err
	}
	return json.Marshal(string(p))
}

func (p *WorkspacePath) UnmarshalJSON(data []byte) error {
	var s string
	if err := json.Unmarshal(data, &s); err != nil {
		return validationError("path", "must_be_string")
	}
	v, err := ParseWorkspacePath(s)
	if err != nil {
		return err
	}
	*p = v
	return nil
}

func ParseWorkspaceUUID(field, s string) (WorkspaceUUID, error) {
	id, err := uuid.Parse(s)
	if err != nil || id.String() != s {
		return "", validationError(field, "invalid_uuid")
	}
	return WorkspaceUUID(s), nil
}

func (id WorkspaceUUID) MarshalJSON() ([]byte, error) {
	if _, err := ParseWorkspaceUUID("uuid", string(id)); err != nil {
		return nil, err
	}
	return json.Marshal(string(id))
}

func (id *WorkspaceUUID) UnmarshalJSON(data []byte) error {
	var s string
	if err := json.Unmarshal(data, &s); err != nil {
		return validationError("uuid", "must_be_string")
	}
	v, err := ParseWorkspaceUUID("uuid", s)
	if err != nil {
		return err
	}
	*id = v
	return nil
}

func (h *WorkspaceNullableHash) UnmarshalJSON(data []byte) error {
	h.Present = true
	if bytes.Equal(bytes.TrimSpace(data), []byte("null")) {
		h.Value = nil
		return nil
	}
	var value WorkspaceContentHash
	if err := json.Unmarshal(data, &value); err != nil {
		return err
	}
	h.Value = &value
	return nil
}

func (h WorkspaceNullableHash) MarshalJSON() ([]byte, error) {
	if !h.Present {
		return nil, validationError("contentHash", "required_key_missing")
	}
	if h.Value == nil {
		return []byte("null"), nil
	}
	return json.Marshal(*h.Value)
}

func (v *WorkspaceNullableUUID) UnmarshalJSON(data []byte) error {
	v.Present = true
	if bytes.Equal(bytes.TrimSpace(data), []byte("null")) {
		v.Value = nil
		return nil
	}
	var raw string
	if err := json.Unmarshal(data, &raw); err != nil {
		return validationError("uuid", "must_be_uuid_or_null")
	}
	parsed, err := ParseWorkspaceUUID("uuid", raw)
	if err != nil {
		return err
	}
	v.Value = &parsed
	return nil
}

func (v WorkspaceNullableUUID) MarshalJSON() ([]byte, error) {
	if !v.Present {
		return nil, validationError("uuid", "required_key_missing")
	}
	if v.Value == nil {
		return []byte("null"), nil
	}
	return json.Marshal(*v.Value)
}

func (v *WorkspaceNullableUint64) UnmarshalJSON(data []byte) error {
	v.Present = true
	if bytes.Equal(bytes.TrimSpace(data), []byte("null")) {
		v.Value = nil
		return nil
	}
	var parsed uint64
	if err := json.Unmarshal(data, &parsed); err != nil {
		return validationError("uint64", "must_be_uint64_or_null")
	}
	v.Value = &parsed
	return nil
}

func (v WorkspaceNullableUint64) MarshalJSON() ([]byte, error) {
	if !v.Present {
		return nil, validationError("uint64", "required_key_missing")
	}
	if v.Value == nil {
		return []byte("null"), nil
	}
	return json.Marshal(*v.Value)
}

func (m WorkspaceFileMetadata) Validate(kind WorkspaceEntryKind) error {
	if m.Size > WorkspaceMaxBlobBytes {
		return validationError("metadata.size", "limit_exceeded")
	}
	if m.ModifiedAtMS < 0 || m.ModifiedAtMS > 253_402_300_799_999 {
		return validationError("metadata.modifiedAtMs", "out_of_range")
	}
	switch kind {
	case WorkspaceEntryFile, WorkspaceEntrySymlink:
		return nil
	case WorkspaceEntryDirectory, WorkspaceEntryTombstone:
		if m.Size != 0 {
			return validationError("metadata.size", "must_be_zero")
		}
		if m.Executable {
			return validationError("metadata.executable", "must_be_false")
		}
		return nil
	default:
		return validationError("kind", "invalid_enum")
	}
}

func validationError(field, reason string) *WorkspaceValidationError {
	return &WorkspaceValidationError{Field: field, Reason: reason}
}

type WorkspaceV2Request[T any] struct {
	RequestID WorkspaceUUID `json:"requestId"`
	Data      T             `json:"data"`
}

type WorkspaceV2Response[T any] struct {
	RequestID *WorkspaceUUID    `json:"requestId,omitempty"`
	Status    bool              `json:"status"`
	Data      *T                `json:"data,omitempty"`
	Error     *WorkspaceV2Error `json:"error,omitempty"`
}

type WorkspaceV2FieldError struct {
	Field  string `json:"field"`
	Reason string `json:"reason"`
}

type WorkspaceV2Error struct {
	Code      WorkspaceV2ErrorCode    `json:"code"`
	Message   string                  `json:"message"`
	Retryable bool                    `json:"retryable"`
	Fields    []WorkspaceV2FieldError `json:"fields,omitempty"`
}

type WorkspaceV2ErrorCode string

const (
	WorkspaceErrorInvalidFrame           WorkspaceV2ErrorCode = "invalid_frame"
	WorkspaceErrorInvalidJSON            WorkspaceV2ErrorCode = "invalid_json"
	WorkspaceErrorUnknownAction          WorkspaceV2ErrorCode = "unknown_action"
	WorkspaceErrorUnauthenticated        WorkspaceV2ErrorCode = "unauthenticated"
	WorkspaceErrorForbidden              WorkspaceV2ErrorCode = "forbidden"
	WorkspaceErrorInvalidRequest         WorkspaceV2ErrorCode = "invalid_request"
	WorkspaceErrorInvalidRevision        WorkspaceV2ErrorCode = "invalid_revision"
	WorkspaceErrorInvalidHash            WorkspaceV2ErrorCode = "invalid_hash"
	WorkspaceErrorInvalidPath            WorkspaceV2ErrorCode = "invalid_path"
	WorkspaceErrorWorkspaceNotFound      WorkspaceV2ErrorCode = "workspace_not_found"
	WorkspaceErrorWorkspaceLimitExceeded WorkspaceV2ErrorCode = "workspace_limit_exceeded"
	WorkspaceErrorClientNotRegistered    WorkspaceV2ErrorCode = "client_not_registered"
	WorkspaceErrorStaleBaseRevision      WorkspaceV2ErrorCode = "stale_base_revision"
	WorkspaceErrorOperationReused        WorkspaceV2ErrorCode = "operation_reused"
	WorkspaceErrorBlobRequired           WorkspaceV2ErrorCode = "blob_required"
	WorkspaceErrorBlobNotFound           WorkspaceV2ErrorCode = "blob_not_found"
	WorkspaceErrorBlobHashMismatch       WorkspaceV2ErrorCode = "blob_hash_mismatch"
	WorkspaceErrorBlobSizeMismatch       WorkspaceV2ErrorCode = "blob_size_mismatch"
	WorkspaceErrorBlobTransferOutOfOrder WorkspaceV2ErrorCode = "blob_transfer_out_of_order"
	WorkspaceErrorBlobLimitExceeded      WorkspaceV2ErrorCode = "blob_limit_exceeded"
	WorkspaceErrorConflictNotFound       WorkspaceV2ErrorCode = "conflict_not_found"
	WorkspaceErrorConflictRevisionStale  WorkspaceV2ErrorCode = "conflict_revision_stale"
	WorkspaceErrorServerBusy             WorkspaceV2ErrorCode = "server_busy"
	WorkspaceErrorInternal               WorkspaceV2ErrorCode = "internal"
)

var WorkspaceV2ErrorCodes = []WorkspaceV2ErrorCode{
	WorkspaceErrorInvalidFrame, WorkspaceErrorInvalidJSON, WorkspaceErrorUnknownAction,
	WorkspaceErrorUnauthenticated, WorkspaceErrorForbidden, WorkspaceErrorInvalidRequest,
	WorkspaceErrorInvalidRevision, WorkspaceErrorInvalidHash, WorkspaceErrorInvalidPath,
	WorkspaceErrorWorkspaceNotFound, WorkspaceErrorWorkspaceLimitExceeded, WorkspaceErrorClientNotRegistered,
	WorkspaceErrorStaleBaseRevision, WorkspaceErrorOperationReused, WorkspaceErrorBlobRequired, WorkspaceErrorBlobNotFound,
	WorkspaceErrorBlobHashMismatch, WorkspaceErrorBlobSizeMismatch, WorkspaceErrorBlobTransferOutOfOrder,
	WorkspaceErrorBlobLimitExceeded, WorkspaceErrorConflictNotFound, WorkspaceErrorConflictRevisionStale,
	WorkspaceErrorServerBusy, WorkspaceErrorInternal,
}

var workspaceV2ErrorMessages = map[WorkspaceV2ErrorCode]string{
	WorkspaceErrorInvalidFrame: "invalid control frame", WorkspaceErrorInvalidJSON: "invalid JSON payload",
	WorkspaceErrorUnknownAction: "unknown workspace action", WorkspaceErrorUnauthenticated: "authentication required",
	WorkspaceErrorForbidden: "workspace access forbidden", WorkspaceErrorInvalidRequest: "invalid request",
	WorkspaceErrorInvalidRevision: "invalid workspace revision", WorkspaceErrorInvalidHash: "invalid content hash",
	WorkspaceErrorInvalidPath:       "path must be a canonical workspace-relative POSIX path",
	WorkspaceErrorWorkspaceNotFound: "workspace not found", WorkspaceErrorWorkspaceLimitExceeded: "workspace limit exceeded",
	WorkspaceErrorClientNotRegistered: "client not registered", WorkspaceErrorStaleBaseRevision: "base revision is stale",
	WorkspaceErrorOperationReused: "operation identifier was reused", WorkspaceErrorBlobRequired: "blob upload required",
	WorkspaceErrorBlobNotFound:     "blob not found",
	WorkspaceErrorBlobHashMismatch: "blob hash mismatch", WorkspaceErrorBlobSizeMismatch: "blob size mismatch",
	WorkspaceErrorBlobTransferOutOfOrder: "blob transfer is out of order", WorkspaceErrorBlobLimitExceeded: "blob transfer limit exceeded",
	WorkspaceErrorConflictNotFound: "conflict not found", WorkspaceErrorConflictRevisionStale: "conflict revision is stale",
	WorkspaceErrorServerBusy: "server is busy", WorkspaceErrorInternal: "internal server error",
}

func NewWorkspaceV2Error(code WorkspaceV2ErrorCode, fields ...WorkspaceV2FieldError) WorkspaceV2Error {
	message, ok := workspaceV2ErrorMessages[code]
	if !ok {
		message = "internal server error"
		code = WorkspaceErrorInternal
	}
	return WorkspaceV2Error{
		Code: code, Message: message,
		Retryable: code == WorkspaceErrorServerBusy || code == WorkspaceErrorInternal,
		Fields:    fields,
	}
}

type WorkspaceV2Action string

const (
	WorkspaceActionHello            WorkspaceV2Action = "WorkspaceHello"
	WorkspaceActionSubscribe        WorkspaceV2Action = "WorkspaceSubscribe"
	WorkspaceActionSnapshotBegin    WorkspaceV2Action = "WorkspaceSnapshotBegin"
	WorkspaceActionSnapshotEntry    WorkspaceV2Action = "WorkspaceSnapshotEntry"
	WorkspaceActionSnapshotEnd      WorkspaceV2Action = "WorkspaceSnapshotEnd"
	WorkspaceActionMutation         WorkspaceV2Action = "WorkspaceMutation"
	WorkspaceActionMutationAccepted WorkspaceV2Action = "WorkspaceMutationAccepted"
	WorkspaceActionMutationRejected WorkspaceV2Action = "WorkspaceMutationRejected"
	WorkspaceActionEvent            WorkspaceV2Action = "WorkspaceEvent"
	WorkspaceActionAck              WorkspaceV2Action = "WorkspaceAck"
	WorkspaceActionBlobNeed         WorkspaceV2Action = "WorkspaceBlobNeed"
	WorkspaceActionBlobBegin        WorkspaceV2Action = "WorkspaceBlobBegin"
	WorkspaceActionBlobEnd          WorkspaceV2Action = "WorkspaceBlobEnd"
	WorkspaceActionConflictCreated  WorkspaceV2Action = "WorkspaceConflictCreated"
	WorkspaceActionConflictResolved WorkspaceV2Action = "WorkspaceConflictResolved"
)

type WorkspaceV2Flow string

const (
	WorkspaceFlowClientRequest  WorkspaceV2Flow = "client_request"
	WorkspaceFlowServerResponse WorkspaceV2Flow = "server_response"
	WorkspaceFlowServerPush     WorkspaceV2Flow = "server_push"
)

var WorkspaceV2Actions = []WorkspaceV2Action{
	WorkspaceActionHello, WorkspaceActionSubscribe,
	WorkspaceActionSnapshotBegin, WorkspaceActionSnapshotEntry, WorkspaceActionSnapshotEnd,
	WorkspaceActionMutation, WorkspaceActionMutationAccepted, WorkspaceActionMutationRejected,
	WorkspaceActionEvent, WorkspaceActionAck,
	WorkspaceActionBlobNeed, WorkspaceActionBlobBegin, WorkspaceActionBlobEnd,
	WorkspaceActionConflictCreated, WorkspaceActionConflictResolved,
}

func (r WorkspaceV2Response[T]) Validate() error {
	if r.Status {
		if r.Data == nil {
			return validationError("response", "success_requires_data")
		}
		if r.Error != nil {
			return validationError("response", "success_forbids_error")
		}
		return nil
	}
	if r.Error == nil {
		return validationError("response", "error_requires_error")
	}
	if r.Data != nil {
		return validationError("response", "error_forbids_data")
	}
	return nil
}

func DecodeWorkspaceV2Request[T any](frame []byte, dst *WorkspaceV2Request[T]) error {
	if dst == nil || len(frame) > 65_536 {
		return validationError("frame", "invalid_json")
	}
	if err := strictJSONDecode(frame, dst); err != nil {
		return validationError("frame", "invalid_json")
	}
	if _, err := ParseWorkspaceUUID("requestId", string(dst.RequestID)); err != nil {
		return err
	}
	return nil
}

func EncodeWorkspaceV2Response[T any](action WorkspaceV2Action, response WorkspaceV2Response[T]) ([]byte, error) {
	spec, ok := WorkspaceV2ActionSpecs[action]
	if !ok || !workspaceV2ActionExists(action) {
		return nil, validationError("action", "unknown_action")
	}
	if err := response.Validate(); err != nil {
		return nil, err
	}
	if response.Status {
		flow := WorkspaceFlowServerPush
		if response.RequestID != nil {
			flow = WorkspaceFlowServerResponse
		}
		factory, allowed := spec.Flows[flow]
		if !allowed {
			return nil, validationError("flow", "flow_not_allowed")
		}
		if reflect.TypeOf(response.Data) != reflect.TypeOf(factory()) {
			return nil, validationError("data", "type_mismatch")
		}
	}
	return encodeWorkspaceV2Frame(string(action), response)
}

func EncodeWorkspaceV2UnknownActionFailure(receivedAction string, requestID *WorkspaceUUID) ([]byte, error) {
	if !validWorkspaceV2ActionToken(receivedAction) {
		return nil, validationError("action", "invalid_token")
	}
	if workspaceV2ActionExists(WorkspaceV2Action(receivedAction)) {
		return nil, validationError("action", "registered_action")
	}
	errorValue := NewWorkspaceV2Error(WorkspaceErrorUnknownAction)
	return encodeWorkspaceV2Frame(receivedAction, WorkspaceV2Response[struct{}]{
		RequestID: requestID,
		Status:    false,
		Error:     &errorValue,
	})
}

func validWorkspaceV2ActionToken(action string) bool {
	if len(action) == 0 || len(action) > 64 || !workspaceASCIIAlpha(action[0]) {
		return false
	}
	for i := 1; i < len(action); i++ {
		if !workspaceASCIIAlpha(action[i]) && (action[i] < '0' || action[i] > '9') {
			return false
		}
	}
	return true
}

func workspaceASCIIAlpha(value byte) bool {
	return value >= 'A' && value <= 'Z' || value >= 'a' && value <= 'z'
}

func encodeWorkspaceV2Frame(action string, response any) ([]byte, error) {
	payload, err := json.Marshal(response)
	if err != nil {
		return nil, err
	}
	out := make([]byte, 0, len(action)+1+len(payload))
	out = append(out, action...)
	out = append(out, '|')
	out = append(out, payload...)
	if len(out) > 65_536 {
		return nil, validationError("frame", "too_large")
	}
	return out, nil
}

func workspaceV2ActionExists(action WorkspaceV2Action) bool {
	for _, candidate := range WorkspaceV2Actions {
		if candidate == action {
			return true
		}
	}
	return false
}

func strictJSONDecode(data []byte, dst any) error {
	if err := rejectDuplicateJSONKeys(data); err != nil {
		return err
	}
	if dst == nil {
		return fmt.Errorf("JSON destination is nil")
	}
	if err := requireJSONStructKeys(data, reflect.TypeOf(dst)); err != nil {
		return err
	}
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(dst); err != nil {
		return err
	}
	if err := decoder.Decode(&struct{}{}); !errors.Is(err, io.EOF) {
		return fmt.Errorf("trailing JSON data")
	}
	return nil
}

var jsonUnmarshalerType = reflect.TypeOf((*json.Unmarshaler)(nil)).Elem()

func requireJSONStructKeys(data []byte, target reflect.Type) error {
	return requireJSONValue(data, target, false)
}

var (
	jsonRawMessageType          = reflect.TypeOf(json.RawMessage{})
	workspaceNullableHashType   = reflect.TypeOf(WorkspaceNullableHash{})
	workspaceNullableUUIDType   = reflect.TypeOf(WorkspaceNullableUUID{})
	workspaceNullableUint64Type = reflect.TypeOf(WorkspaceNullableUint64{})
)

func requireJSONValue(data []byte, target reflect.Type, allowNull bool) error {
	if bytes.Equal(bytes.TrimSpace(data), []byte("null")) {
		if allowNull || workspaceJSONTypeAllowsNull(target) {
			return nil
		}
		return fmt.Errorf("JSON null is not allowed")
	}
	for target.Kind() == reflect.Pointer {
		if target.Implements(jsonUnmarshalerType) {
			return nil
		}
		target = target.Elem()
	}
	if target.Implements(jsonUnmarshalerType) || (target.Kind() != reflect.Pointer && reflect.PointerTo(target).Implements(jsonUnmarshalerType)) {
		return nil
	}
	switch target.Kind() {
	case reflect.Struct:
		var object map[string]json.RawMessage
		if err := json.Unmarshal(data, &object); err != nil {
			return err
		}
		for i := 0; i < target.NumField(); i++ {
			field := target.Field(i)
			if field.PkgPath != "" {
				continue
			}
			name, optional, ignored := workspaceJSONField(field)
			if ignored {
				continue
			}
			raw, present := object[name]
			if !present {
				if optional {
					continue
				}
				return fmt.Errorf("required JSON key %q missing", name)
			}
			fieldAllowsNull := workspaceJSONTypeAllowsNull(field.Type)
			if field.Type.Kind() == reflect.Pointer {
				fieldAllowsNull = !optional
			}
			if err := requireJSONValue(raw, field.Type, fieldAllowsNull); err != nil {
				return fmt.Errorf("JSON key %q: %w", name, err)
			}
		}
	case reflect.Array, reflect.Slice:
		var elements []json.RawMessage
		if err := json.Unmarshal(data, &elements); err != nil {
			return err
		}
		for i, element := range elements {
			if err := requireJSONValue(element, target.Elem(), false); err != nil {
				return fmt.Errorf("JSON index %d: %w", i, err)
			}
		}
	}
	return nil
}

func workspaceJSONTypeAllowsNull(target reflect.Type) bool {
	return target == jsonRawMessageType || target == workspaceNullableHashType ||
		target == workspaceNullableUUIDType || target == workspaceNullableUint64Type
}

func workspaceJSONField(field reflect.StructField) (name string, optional, ignored bool) {
	tag := field.Tag.Get("json")
	parts := strings.Split(tag, ",")
	if parts[0] == "-" {
		return "", false, true
	}
	name = parts[0]
	if name == "" {
		name = field.Name
	}
	for _, option := range parts[1:] {
		if option == "omitempty" {
			optional = true
			break
		}
	}
	return name, optional, false
}

func rejectDuplicateJSONKeys(data []byte) error {
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	first, err := decoder.Token()
	if err != nil {
		return err
	}
	if first != json.Delim('{') {
		return fmt.Errorf("top-level JSON value must be an object")
	}
	if err := walkJSONObject(decoder); err != nil {
		return err
	}
	if _, err := decoder.Token(); !errors.Is(err, io.EOF) {
		if err == nil {
			return fmt.Errorf("trailing JSON data")
		}
		return err
	}
	return nil
}

func walkJSONObject(decoder *json.Decoder) error {
	seen := make(map[string]struct{})
	for decoder.More() {
		token, err := decoder.Token()
		if err != nil {
			return err
		}
		key, ok := token.(string)
		if !ok {
			return fmt.Errorf("object key is not a string")
		}
		if _, duplicate := seen[key]; duplicate {
			return fmt.Errorf("duplicate object key %q", key)
		}
		seen[key] = struct{}{}
		if err := walkJSONValue(decoder); err != nil {
			return err
		}
	}
	end, err := decoder.Token()
	if err != nil {
		return err
	}
	if end != json.Delim('}') {
		return fmt.Errorf("unterminated object")
	}
	return nil
}

func walkJSONValue(decoder *json.Decoder) error {
	token, err := decoder.Token()
	if err != nil {
		return err
	}
	delim, ok := token.(json.Delim)
	if !ok {
		return nil
	}
	switch delim {
	case '{':
		return walkJSONObject(decoder)
	case '[':
		for decoder.More() {
			if err := walkJSONValue(decoder); err != nil {
				return err
			}
		}
		end, err := decoder.Token()
		if err != nil || end != json.Delim(']') {
			return fmt.Errorf("unterminated array")
		}
		return nil
	default:
		return fmt.Errorf("unexpected delimiter")
	}
}

type WorkspaceHelloRequest struct {
	ProtocolVersion string        `json:"protocolVersion"`
	ClientID        WorkspaceUUID `json:"clientId"`
	ClientVersion   string        `json:"clientVersion"`
	Capabilities    []string      `json:"capabilities"`
}

type WorkspaceHelloResponse struct {
	ProtocolVersion           string `json:"protocolVersion"`
	ServerVersion             string `json:"serverVersion"`
	MaxControlFrameBytes      uint32 `json:"maxControlFrameBytes"`
	MaxBinaryChunkBytes       uint32 `json:"maxBinaryChunkBytes"`
	MaxBlobBytes              uint64 `json:"maxBlobBytes"`
	MaxTransfersPerConnection uint32 `json:"maxTransfersPerConnection"`
	HeartbeatSeconds          uint32 `json:"heartbeatSeconds"`
}

type WorkspaceSubscribeRequest struct {
	WorkspaceID     WorkspaceUUID     `json:"workspaceId"`
	ClientID        WorkspaceUUID     `json:"clientId"`
	LastAckRevision WorkspaceRevision `json:"lastAckRevision"`
}

type WorkspacePathState struct {
	Path         WorkspacePath         `json:"path"`
	PathRevision WorkspaceRevision     `json:"pathRevision"`
	Kind         WorkspaceEntryKind    `json:"kind"`
	ContentHash  WorkspaceNullableHash `json:"contentHash"`
	Metadata     WorkspaceFileMetadata `json:"metadata"`
	Tombstone    bool                  `json:"tombstone"`
}

type WorkspaceSnapshotMode string

const (
	WorkspaceSnapshotFull        WorkspaceSnapshotMode = "snapshot"
	WorkspaceSnapshotIncremental WorkspaceSnapshotMode = "incremental"
)

type WorkspaceSnapshotBeginMessage struct {
	WorkspaceID   WorkspaceUUID         `json:"workspaceId"`
	StreamID      WorkspaceUUID         `json:"streamId"`
	Mode          WorkspaceSnapshotMode `json:"mode"`
	FromRevision  WorkspaceRevision     `json:"fromRevision"`
	FinalRevision WorkspaceRevision     `json:"finalRevision"`
	EntryCount    uint32                `json:"entryCount"`
	EventCount    uint32                `json:"eventCount"`
	ConflictCount uint32                `json:"conflictCount"`
}

type WorkspaceSnapshotEntryMessage struct {
	WorkspaceID WorkspaceUUID      `json:"workspaceId"`
	StreamID    WorkspaceUUID      `json:"streamId"`
	Index       uint32             `json:"index"`
	Entry       WorkspacePathState `json:"entry"`
}

type WorkspaceSnapshotEndMessage struct {
	WorkspaceID    WorkspaceUUID         `json:"workspaceId"`
	StreamID       WorkspaceUUID         `json:"streamId"`
	Mode           WorkspaceSnapshotMode `json:"mode"`
	DeliveredCount uint32                `json:"deliveredCount"`
	FinalRevision  WorkspaceRevision     `json:"finalRevision"`
}

type WorkspaceMutation struct {
	WorkspaceID            WorkspaceUUID         `json:"workspaceId"`
	ClientID               WorkspaceUUID         `json:"clientId"`
	OperationID            WorkspaceUUID         `json:"operationId"`
	Path                   WorkspacePath         `json:"path"`
	BasePathRevision       WorkspaceRevision     `json:"basePathRevision"`
	Kind                   WorkspaceMutationKind `json:"kind"`
	ContentHash            WorkspaceNullableHash `json:"contentHash"`
	Metadata               WorkspaceFileMetadata `json:"metadata"`
	NewPath                *WorkspacePath        `json:"newPath,omitempty"`
	TargetBasePathRevision *WorkspaceRevision    `json:"targetBasePathRevision,omitempty"`
}

type WorkspaceMutationAcceptedMessage struct {
	WorkspaceID  WorkspaceUUID       `json:"workspaceId"`
	ClientID     WorkspaceUUID       `json:"clientId"`
	OperationID  WorkspaceUUID       `json:"operationId"`
	Revision     WorkspaceRevision   `json:"revision"`
	PathState    WorkspacePathState  `json:"pathState"`
	OldPathState *WorkspacePathState `json:"oldPathState,omitempty"`
	NewPathState *WorkspacePathState `json:"newPathState,omitempty"`
}

const (
	WorkspaceMutationRejectStaleBase       = "stale_base_revision"
	WorkspaceMutationRejectOperationReused = "operation_reused"
	WorkspaceMutationRejectBlobRequired    = "blob_required"
	WorkspaceMutationRejectConflictCreated = "conflict_created"
)

type WorkspaceMutationRejectedMessage struct {
	WorkspaceID      WorkspaceUUID         `json:"workspaceId"`
	ClientID         WorkspaceUUID         `json:"clientId"`
	OperationID      WorkspaceUUID         `json:"operationId"`
	Reason           string                `json:"reason"`
	CurrentPathState *WorkspacePathState   `json:"currentPathState"`
	ConflictID       *WorkspaceUUID        `json:"conflictId"`
	RequiredHash     *WorkspaceContentHash `json:"requiredHash"`
}

type WorkspaceEventMessage struct {
	WorkspaceID    WorkspaceUUID       `json:"workspaceId"`
	StreamID       WorkspaceUUID       `json:"streamId"`
	Index          uint32              `json:"index"`
	Revision       WorkspaceRevision   `json:"revision"`
	OperationID    WorkspaceUUID       `json:"operationId"`
	OriginClientID WorkspaceUUID       `json:"originClientId"`
	Mutation       WorkspaceMutation   `json:"mutation"`
	PathState      WorkspacePathState  `json:"pathState"`
	OldPathState   *WorkspacePathState `json:"oldPathState,omitempty"`
	NewPathState   *WorkspacePathState `json:"newPathState,omitempty"`
}

type WorkspaceAckRequest struct {
	WorkspaceID WorkspaceUUID     `json:"workspaceId"`
	ClientID    WorkspaceUUID     `json:"clientId"`
	Revision    WorkspaceRevision `json:"revision"`
}

func (m WorkspaceHelloRequest) Validate() error {
	if m.ProtocolVersion != "2" {
		return validationError("protocolVersion", "unsupported")
	}
	if _, err := ParseWorkspaceUUID("clientId", string(m.ClientID)); err != nil {
		return err
	}
	if m.ClientVersion == "" {
		return validationError("clientVersion", "required")
	}
	want := []string{"binary_chunks", "conflicts", "snapshot_v1"}
	if len(m.Capabilities) != len(want) {
		return validationError("capabilities", "required_set")
	}
	for i := range want {
		if m.Capabilities[i] != want[i] {
			return validationError("capabilities", "required_set")
		}
	}
	return nil
}

func (m WorkspaceHelloResponse) Validate() error {
	if m.ProtocolVersion != "2" || m.ServerVersion == "" {
		return validationError("hello", "invalid_version")
	}
	if m.MaxControlFrameBytes != WorkspaceMaxControlFrameBytes || m.MaxBinaryChunkBytes != WorkspaceBlobChunkSize ||
		m.MaxBlobBytes != WorkspaceMaxBlobBytes || m.MaxTransfersPerConnection != 4 || m.HeartbeatSeconds != 25 {
		return validationError("hello", "invalid_limits")
	}
	return nil
}

func (m WorkspaceSubscribeRequest) Validate() error {
	if _, err := ParseWorkspaceUUID("workspaceId", string(m.WorkspaceID)); err != nil {
		return err
	}
	_, err := ParseWorkspaceUUID("clientId", string(m.ClientID))
	return err
}

func (m WorkspacePathState) Validate() error {
	if _, err := ParseWorkspacePath(string(m.Path)); err != nil {
		return err
	}
	if !m.ContentHash.Present {
		return validationError("contentHash", "required")
	}
	if m.Tombstone != (m.Kind == WorkspaceEntryTombstone) {
		return validationError("tombstone", "kind_mismatch")
	}
	switch m.Kind {
	case WorkspaceEntryFile, WorkspaceEntrySymlink:
		if m.ContentHash.Value == nil {
			return validationError("contentHash", "required_for_kind")
		}
	case WorkspaceEntryDirectory, WorkspaceEntryTombstone:
		if m.ContentHash.Value != nil {
			return validationError("contentHash", "must_be_null_for_kind")
		}
	default:
		return validationError("kind", "invalid_enum")
	}
	if m.ContentHash.Value != nil {
		if _, err := ParseWorkspaceContentHash(string(*m.ContentHash.Value)); err != nil {
			return err
		}
	}
	return m.Metadata.Validate(m.Kind)
}

func (m WorkspaceSnapshotBeginMessage) Validate() error {
	if err := validateWorkspaceAndRelatedID(m.WorkspaceID, "streamId", m.StreamID); err != nil {
		return err
	}
	if m.FinalRevision < m.FromRevision {
		return validationError("finalRevision", "before_from_revision")
	}
	switch m.Mode {
	case WorkspaceSnapshotFull:
		if m.EventCount != 0 {
			return validationError("eventCount", "must_be_zero_for_snapshot")
		}
	case WorkspaceSnapshotIncremental:
		if m.EntryCount != 0 {
			return validationError("entryCount", "must_be_zero_for_incremental")
		}
	default:
		return validationError("mode", "invalid_enum")
	}
	return nil
}

func (m WorkspaceSnapshotEntryMessage) Validate(expectedIndex uint32) error {
	if err := validateWorkspaceAndRelatedID(m.WorkspaceID, "streamId", m.StreamID); err != nil {
		return err
	}
	if m.Index != expectedIndex {
		return validationError("index", "stream_gap")
	}
	return m.Entry.Validate()
}

func (m WorkspaceSnapshotEndMessage) ValidateAgainst(begin WorkspaceSnapshotBeginMessage) error {
	if err := begin.Validate(); err != nil {
		return err
	}
	if m.WorkspaceID != begin.WorkspaceID || m.StreamID != begin.StreamID || m.Mode != begin.Mode || m.FinalRevision != begin.FinalRevision {
		return validationError("snapshotEnd", "begin_mismatch")
	}
	want := begin.EntryCount
	if begin.Mode == WorkspaceSnapshotIncremental {
		want = begin.EventCount
	}
	if begin.ConflictCount > ^uint32(0)-want {
		return validationError("deliveredCount", "count_overflow")
	}
	want += begin.ConflictCount
	if m.DeliveredCount != want {
		return validationError("deliveredCount", "count_mismatch")
	}
	return nil
}

func (m WorkspaceMutation) Validate() error {
	if _, err := ParseWorkspaceUUID("workspaceId", string(m.WorkspaceID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("clientId", string(m.ClientID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("operationId", string(m.OperationID)); err != nil {
		return err
	}
	if _, err := ParseWorkspacePath(string(m.Path)); err != nil {
		return err
	}
	if !m.ContentHash.Present {
		return validationError("contentHash", "required")
	}
	if m.Kind == WorkspaceMutationRename {
		if m.NewPath == nil {
			return validationError("newPath", "required_for_rename")
		}
		if m.TargetBasePathRevision == nil {
			return validationError("targetBasePathRevision", "required_for_rename")
		}
		if _, err := ParseWorkspacePath(string(*m.NewPath)); err != nil {
			return validationError("newPath", "invalid_path")
		}
		if *m.NewPath == m.Path {
			return validationError("newPath", "same_as_path")
		}
		if strings.HasPrefix(string(*m.NewPath), string(m.Path)+"/") {
			return validationError("newPath", "directory_into_child")
		}
	} else if m.NewPath != nil || m.TargetBasePathRevision != nil {
		if m.NewPath != nil {
			return validationError("newPath", "forbidden_for_kind")
		}
		return validationError("targetBasePathRevision", "forbidden_for_kind")
	}

	switch m.Kind {
	case WorkspaceMutationUpsertFile, WorkspaceMutationUpsertSymlink:
		if m.ContentHash.Value == nil {
			return validationError("contentHash", "required_for_kind")
		}
		if _, err := ParseWorkspaceContentHash(string(*m.ContentHash.Value)); err != nil {
			return err
		}
		kind := WorkspaceEntryFile
		if m.Kind == WorkspaceMutationUpsertSymlink {
			kind = WorkspaceEntrySymlink
		}
		return m.Metadata.Validate(kind)
	case WorkspaceMutationMkdir, WorkspaceMutationDelete:
		if m.ContentHash.Value != nil {
			return validationError("contentHash", "must_be_null_for_kind")
		}
		kind := WorkspaceEntryDirectory
		if m.Kind == WorkspaceMutationDelete {
			kind = WorkspaceEntryTombstone
		}
		return m.Metadata.Validate(kind)
	case WorkspaceMutationRename:
		if m.ContentHash.Value != nil {
			if _, err := ParseWorkspaceContentHash(string(*m.ContentHash.Value)); err != nil {
				return err
			}
		}
		return nil
	default:
		return validationError("kind", "invalid_enum")
	}
}

func (m WorkspaceMutationAcceptedMessage) Validate() error {
	if _, err := ParseWorkspaceUUID("workspaceId", string(m.WorkspaceID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("clientId", string(m.ClientID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("operationId", string(m.OperationID)); err != nil {
		return err
	}
	if m.Revision == 0 || m.PathState.PathRevision != m.Revision {
		return validationError("revision", "path_state_mismatch")
	}
	if err := m.PathState.Validate(); err != nil {
		return err
	}
	if (m.OldPathState == nil) != (m.NewPathState == nil) {
		return validationError("pathState", "rename_pair_required")
	}
	if m.OldPathState != nil {
		if err := m.OldPathState.Validate(); err != nil {
			return err
		}
		if err := m.NewPathState.Validate(); err != nil {
			return err
		}
		if m.OldPathState.Path == m.NewPathState.Path {
			return validationError("oldPathState.path", "rename_path_required")
		}
		if m.OldPathState.PathRevision != m.Revision {
			return validationError("oldPathState.pathRevision", "revision_mismatch")
		}
		if m.NewPathState.PathRevision != m.Revision {
			return validationError("newPathState.pathRevision", "revision_mismatch")
		}
		if !workspacePathStateEqual(m.PathState, *m.NewPathState) {
			return validationError("pathState", "new_path_state_mismatch")
		}
	}
	return nil
}

func (m WorkspaceMutationRejectedMessage) Validate() error {
	if _, err := ParseWorkspaceUUID("workspaceId", string(m.WorkspaceID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("clientId", string(m.ClientID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("operationId", string(m.OperationID)); err != nil {
		return err
	}
	if m.CurrentPathState != nil {
		if err := m.CurrentPathState.Validate(); err != nil {
			return err
		}
	}
	switch m.Reason {
	case WorkspaceMutationRejectStaleBase, WorkspaceMutationRejectOperationReused:
		if m.ConflictID != nil || m.RequiredHash != nil {
			return validationError("reason", "payload_mismatch")
		}
	case WorkspaceMutationRejectBlobRequired:
		if m.RequiredHash == nil || m.ConflictID != nil {
			return validationError("requiredHash", "required_for_reason")
		}
		if _, err := ParseWorkspaceContentHash(string(*m.RequiredHash)); err != nil {
			return err
		}
	case WorkspaceMutationRejectConflictCreated:
		if m.ConflictID == nil {
			return validationError("conflictId", "required_for_reason")
		}
		if m.RequiredHash != nil {
			return validationError("requiredHash", "forbidden_for_reason")
		}
	default:
		return validationError("reason", "invalid_enum")
	}
	return nil
}

func (m WorkspaceEventMessage) Validate(previousIndex uint32, previousRevision WorkspaceRevision) error {
	if err := validateWorkspaceAndRelatedID(m.WorkspaceID, "streamId", m.StreamID); err != nil {
		return err
	}
	if m.Index != previousIndex+1 {
		return validationError("index", "stream_gap")
	}
	if m.Revision <= previousRevision {
		return validationError("revision", "not_strictly_increasing")
	}
	if _, err := ParseWorkspaceUUID("operationId", string(m.OperationID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("originClientId", string(m.OriginClientID)); err != nil {
		return err
	}
	if err := m.Mutation.Validate(); err != nil {
		return err
	}
	if m.Mutation.WorkspaceID != m.WorkspaceID || m.Mutation.OperationID != m.OperationID || m.Mutation.ClientID != m.OriginClientID {
		return validationError("mutation", "event_identity_mismatch")
	}
	if m.PathState.PathRevision != m.Revision {
		return validationError("pathState.pathRevision", "revision_mismatch")
	}
	if err := m.PathState.Validate(); err != nil {
		return err
	}
	if m.Mutation.Kind == WorkspaceMutationRename {
		if m.OldPathState == nil || m.NewPathState == nil {
			return validationError("pathState", "rename_pair_required")
		}
		if err := m.OldPathState.Validate(); err != nil {
			return err
		}
		if err := m.NewPathState.Validate(); err != nil {
			return err
		}
		if m.OldPathState.Path != m.Mutation.Path {
			return validationError("oldPathState.path", "mutation_path_mismatch")
		}
		if m.NewPathState.Path != *m.Mutation.NewPath {
			return validationError("newPathState.path", "mutation_new_path_mismatch")
		}
		if m.OldPathState.PathRevision != m.Revision {
			return validationError("oldPathState.pathRevision", "revision_mismatch")
		}
		if m.NewPathState.PathRevision != m.Revision {
			return validationError("newPathState.pathRevision", "revision_mismatch")
		}
		if !workspacePathStateEqual(m.PathState, *m.NewPathState) {
			return validationError("pathState", "new_path_state_mismatch")
		}
		return nil
	}
	if m.OldPathState != nil || m.NewPathState != nil {
		return validationError("pathState", "forbidden_for_kind")
	}
	return nil
}

func (m WorkspaceAckRequest) Validate(previousAck, lastDelivered WorkspaceRevision) error {
	if _, err := ParseWorkspaceUUID("workspaceId", string(m.WorkspaceID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("clientId", string(m.ClientID)); err != nil {
		return err
	}
	if m.Revision <= previousAck {
		return validationError("revision", "ack_regression")
	}
	if m.Revision > lastDelivered {
		return validationError("revision", "ack_overshoot")
	}
	return nil
}

func validateWorkspaceAndRelatedID(workspaceID WorkspaceUUID, field string, id WorkspaceUUID) error {
	if _, err := ParseWorkspaceUUID("workspaceId", string(workspaceID)); err != nil {
		return err
	}
	_, err := ParseWorkspaceUUID(field, string(id))
	return err
}

type WorkspaceBlobDirection string

const (
	WorkspaceBlobUpload   WorkspaceBlobDirection = "upload"
	WorkspaceBlobDownload WorkspaceBlobDirection = "download"
)

type WorkspaceBlobNeedUploadPush struct {
	WorkspaceID WorkspaceUUID          `json:"workspaceId"`
	Direction   WorkspaceBlobDirection `json:"direction"`
	OperationID WorkspaceUUID          `json:"operationId"`
	ContentHash WorkspaceContentHash   `json:"contentHash"`
	Size        uint64                 `json:"size"`
}

type WorkspaceBlobNeedDownloadRequest struct {
	WorkspaceID WorkspaceUUID           `json:"workspaceId"`
	Direction   WorkspaceBlobDirection  `json:"direction"`
	OperationID WorkspaceNullableUUID   `json:"operationId"`
	ContentHash WorkspaceContentHash    `json:"contentHash"`
	Size        WorkspaceNullableUint64 `json:"size"`
}

type WorkspaceBlobNeedDownloadResponse struct {
	WorkspaceID WorkspaceUUID          `json:"workspaceId"`
	Direction   WorkspaceBlobDirection `json:"direction"`
	OperationID WorkspaceNullableUUID  `json:"operationId"`
	ContentHash WorkspaceContentHash   `json:"contentHash"`
	Size        uint64                 `json:"size"`
}

type WorkspaceBlobBeginMessage struct {
	WorkspaceID WorkspaceUUID          `json:"workspaceId"`
	TransferID  WorkspaceUUID          `json:"transferId"`
	Direction   WorkspaceBlobDirection `json:"direction"`
	ContentHash WorkspaceContentHash   `json:"contentHash"`
	Size        uint64                 `json:"size"`
	ChunkSize   uint32                 `json:"chunkSize"`
	ChunkCount  uint64                 `json:"chunkCount"`
}

type WorkspaceBlobEndMessage struct {
	WorkspaceID WorkspaceUUID          `json:"workspaceId"`
	TransferID  WorkspaceUUID          `json:"transferId"`
	Direction   WorkspaceBlobDirection `json:"direction"`
	ContentHash WorkspaceContentHash   `json:"contentHash"`
	Size        uint64                 `json:"size"`
	ChunkCount  uint64                 `json:"chunkCount"`
}

type WorkspaceBlobHeader struct {
	Direction   WorkspaceBlobDirection
	Final       bool
	TransferID  uuid.UUID
	ChunkIndex  uint64
	Offset      uint64
	PayloadLen  uint32
	ChunkDigest [16]byte
}

func MarshalWorkspaceBlobHeader(h WorkspaceBlobHeader) ([WorkspaceBlobHeaderSize]byte, error) {
	var out [WorkspaceBlobHeaderSize]byte
	copy(out[0:4], "FNS2")
	out[4] = 2
	switch h.Direction {
	case WorkspaceBlobUpload:
		out[5] = 1
	case WorkspaceBlobDownload:
		out[5] = 2
	default:
		return out, validationError("direction", "invalid_enum")
	}
	if h.TransferID == uuid.Nil {
		return out, validationError("transferId", "invalid_uuid")
	}
	if h.PayloadLen == 0 {
		return out, validationError("payloadLength", "empty_payload_forbidden")
	}
	if h.PayloadLen > WorkspaceBlobChunkSize {
		return out, validationError("payloadLength", "limit_exceeded")
	}
	if h.Final {
		out[6] = 1
	}
	out[7] = WorkspaceBlobHeaderSize
	copy(out[8:24], h.TransferID[:])
	binary.BigEndian.PutUint64(out[24:32], h.ChunkIndex)
	binary.BigEndian.PutUint64(out[32:40], h.Offset)
	binary.BigEndian.PutUint32(out[40:44], h.PayloadLen)
	copy(out[48:64], h.ChunkDigest[:])
	return out, nil
}

func UnmarshalWorkspaceBlobHeader(data []byte, actualPayloadLen uint32, expectedDigest [16]byte) (WorkspaceBlobHeader, error) {
	var h WorkspaceBlobHeader
	if len(data) != WorkspaceBlobHeaderSize {
		return h, validationError("header", "invalid_length")
	}
	if string(data[0:4]) != "FNS2" {
		return h, validationError("magic", "invalid")
	}
	if data[4] != 2 {
		return h, validationError("version", "invalid")
	}
	switch data[5] {
	case 1:
		h.Direction = WorkspaceBlobUpload
	case 2:
		h.Direction = WorkspaceBlobDownload
	default:
		return h, validationError("direction", "invalid_enum")
	}
	if data[6]&0xfe != 0 {
		return h, validationError("flags", "reserved_bits")
	}
	h.Final = data[6]&1 != 0
	if data[7] != WorkspaceBlobHeaderSize {
		return h, validationError("headerLength", "invalid")
	}
	copy(h.TransferID[:], data[8:24])
	if h.TransferID == uuid.Nil {
		return h, validationError("transferId", "invalid_uuid")
	}
	h.ChunkIndex = binary.BigEndian.Uint64(data[24:32])
	h.Offset = binary.BigEndian.Uint64(data[32:40])
	h.PayloadLen = binary.BigEndian.Uint32(data[40:44])
	if h.PayloadLen == 0 {
		return h, validationError("payloadLength", "empty_payload_forbidden")
	}
	if h.PayloadLen > WorkspaceBlobChunkSize {
		return h, validationError("payloadLength", "limit_exceeded")
	}
	if h.PayloadLen != actualPayloadLen {
		return h, validationError("payloadLength", "frame_mismatch")
	}
	if !bytes.Equal(data[44:48], []byte{0, 0, 0, 0}) {
		return h, validationError("reserved", "non_zero")
	}
	copy(h.ChunkDigest[:], data[48:64])
	if h.ChunkDigest != expectedDigest {
		return h, validationError("chunkDigest", "mismatch")
	}
	return h, nil
}

func (h WorkspaceBlobHeader) ValidateSequence(expectedIndex, expectedOffset uint64, isLast bool) error {
	if h.PayloadLen == 0 {
		return validationError("payloadLength", "empty_payload_forbidden")
	}
	if h.ChunkIndex != expectedIndex {
		return validationError("chunkIndex", "out_of_order")
	}
	if h.Offset != expectedOffset {
		return validationError("offset", "out_of_order")
	}
	if h.Final != isLast {
		return validationError("final", "flag_mismatch")
	}
	if !isLast && h.PayloadLen != WorkspaceBlobChunkSize {
		return validationError("payloadLength", "non_final_must_be_full")
	}
	if h.PayloadLen > WorkspaceBlobChunkSize {
		return validationError("payloadLength", "limit_exceeded")
	}
	return nil
}

func (m WorkspaceBlobNeedUploadPush) Validate() error {
	if m.Direction != WorkspaceBlobUpload {
		return validationError("direction", "must_be_upload")
	}
	if err := validateWorkspaceAndRelatedID(m.WorkspaceID, "operationId", m.OperationID); err != nil {
		return err
	}
	return validateBlobIdentityAndSize(m.ContentHash, m.Size)
}

func (m WorkspaceBlobNeedDownloadRequest) Validate() error {
	if m.Direction != WorkspaceBlobDownload {
		return validationError("direction", "must_be_download")
	}
	if _, err := ParseWorkspaceUUID("workspaceId", string(m.WorkspaceID)); err != nil {
		return err
	}
	if err := validateRequiredNullUUID("operationId", m.OperationID); err != nil {
		return err
	}
	if err := validateRequiredNullUint64("size", m.Size); err != nil {
		return err
	}
	_, err := ParseWorkspaceContentHash(string(m.ContentHash))
	return err
}

func (m WorkspaceBlobNeedDownloadResponse) Validate() error {
	if m.Direction != WorkspaceBlobDownload {
		return validationError("direction", "must_be_download")
	}
	if _, err := ParseWorkspaceUUID("workspaceId", string(m.WorkspaceID)); err != nil {
		return err
	}
	if err := validateRequiredNullUUID("operationId", m.OperationID); err != nil {
		return err
	}
	return validateBlobIdentityAndSize(m.ContentHash, m.Size)
}

func (m WorkspaceBlobBeginMessage) Validate() error {
	if m.Direction != WorkspaceBlobUpload && m.Direction != WorkspaceBlobDownload {
		return validationError("direction", "invalid_enum")
	}
	if err := validateWorkspaceAndRelatedID(m.WorkspaceID, "transferId", m.TransferID); err != nil {
		return err
	}
	if err := validateBlobIdentityAndSize(m.ContentHash, m.Size); err != nil {
		return err
	}
	if m.ChunkSize != WorkspaceBlobChunkSize {
		return validationError("chunkSize", "must_equal_limit")
	}
	if m.ChunkCount != workspaceBlobChunkCount(m.Size) {
		return validationError("chunkCount", "arithmetic_mismatch")
	}
	return nil
}

func (m WorkspaceBlobEndMessage) Validate() error {
	if m.Direction != WorkspaceBlobUpload && m.Direction != WorkspaceBlobDownload {
		return validationError("direction", "invalid_enum")
	}
	if err := validateWorkspaceAndRelatedID(m.WorkspaceID, "transferId", m.TransferID); err != nil {
		return err
	}
	if err := validateBlobIdentityAndSize(m.ContentHash, m.Size); err != nil {
		return err
	}
	if m.ChunkCount != workspaceBlobChunkCount(m.Size) {
		return validationError("chunkCount", "arithmetic_mismatch")
	}
	return nil
}

func ComputeWorkspaceBlobDigest(payload []byte) (full [32]byte, first16 [16]byte) {
	full = blake3.Sum256(payload)
	copy(first16[:], full[:16])
	return full, first16
}

func validateRequiredNullUUID(field string, value WorkspaceNullableUUID) error {
	if !value.Present {
		return validationError(field, "required_key_missing")
	}
	if value.Value != nil {
		return validationError(field, "must_be_null")
	}
	return nil
}

func validateRequiredNullUint64(field string, value WorkspaceNullableUint64) error {
	if !value.Present {
		return validationError(field, "required_key_missing")
	}
	if value.Value != nil {
		return validationError(field, "must_be_null")
	}
	return nil
}

func workspaceBlobChunkCount(size uint64) uint64 {
	if size == 0 {
		return 0
	}
	return (size-1)/uint64(WorkspaceBlobChunkSize) + 1
}

func validateBlobIdentityAndSize(hash WorkspaceContentHash, size uint64) error {
	if _, err := ParseWorkspaceContentHash(string(hash)); err != nil {
		return err
	}
	if size > WorkspaceMaxBlobBytes {
		return validationError("size", "limit_exceeded")
	}
	return nil
}

type WorkspaceConflictKind string
type WorkspaceConflictChoice string

const (
	WorkspaceConflictContent      WorkspaceConflictKind   = "content"
	WorkspaceConflictDeleteModify WorkspaceConflictKind   = "delete_modify"
	WorkspaceConflictRename       WorkspaceConflictKind   = "rename"
	WorkspaceConflictBinary       WorkspaceConflictKind   = "binary"
	WorkspaceConflictKeepCurrent  WorkspaceConflictChoice = "current"
	WorkspaceConflictUseIncoming  WorkspaceConflictChoice = "incoming"
	WorkspaceConflictUseMerged    WorkspaceConflictChoice = "merged"
	WorkspaceConflictDelete       WorkspaceConflictChoice = "delete"
)

type WorkspaceConflictSide struct {
	Path         *WorkspacePath        `json:"path"`
	PathRevision WorkspaceRevision     `json:"pathRevision"`
	ContentHash  WorkspaceNullableHash `json:"contentHash"`
	Metadata     WorkspaceFileMetadata `json:"metadata"`
	Tombstone    bool                  `json:"tombstone"`
}

type WorkspaceConflictCreatedMessage struct {
	WorkspaceID          WorkspaceUUID             `json:"workspaceId"`
	ConflictID           WorkspaceUUID             `json:"conflictId"`
	ConflictRevision     WorkspaceConflictRevision `json:"conflictRevision"`
	Path                 WorkspacePath             `json:"path"`
	Kind                 WorkspaceConflictKind     `json:"kind"`
	Ancestor             WorkspaceConflictSide     `json:"ancestor"`
	Current              WorkspaceConflictSide     `json:"current"`
	Incoming             WorkspaceConflictSide     `json:"incoming"`
	CreatedByOperationID WorkspaceUUID             `json:"createdByOperationId"`
}

type WorkspaceConflictResolvedRequest struct {
	WorkspaceID      WorkspaceUUID             `json:"workspaceId"`
	ClientID         WorkspaceUUID             `json:"clientId"`
	OperationID      WorkspaceUUID             `json:"operationId"`
	ConflictID       WorkspaceUUID             `json:"conflictId"`
	ConflictRevision WorkspaceConflictRevision `json:"conflictRevision"`
	Choice           WorkspaceConflictChoice   `json:"choice"`
	Path             WorkspacePath             `json:"path"`
	ContentHash      WorkspaceNullableHash     `json:"contentHash"`
	Metadata         WorkspaceFileMetadata     `json:"metadata"`
}

type WorkspaceConflictResolvedMessage struct {
	WorkspaceID        WorkspaceUUID             `json:"workspaceId"`
	ConflictID         WorkspaceUUID             `json:"conflictId"`
	ConflictRevision   WorkspaceConflictRevision `json:"conflictRevision"`
	OperationID        WorkspaceUUID             `json:"operationId"`
	Revision           WorkspaceRevision         `json:"revision"`
	Choice             WorkspaceConflictChoice   `json:"choice"`
	PathState          WorkspacePathState        `json:"pathState"`
	ResolvedByClientID WorkspaceUUID             `json:"resolvedByClientId"`
}

func (m WorkspaceConflictCreatedMessage) Validate() error {
	if _, err := ParseWorkspaceUUID("workspaceId", string(m.WorkspaceID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("conflictId", string(m.ConflictID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("createdByOperationId", string(m.CreatedByOperationID)); err != nil {
		return err
	}
	if m.ConflictRevision == (WorkspaceConflictRevision{}) {
		return validationError("conflictRevision", "must_be_positive")
	}
	if _, err := ParseWorkspacePath(string(m.Path)); err != nil {
		return err
	}
	if err := validateWorkspaceConflictSide("ancestor", m.Ancestor); err != nil {
		return err
	}
	if err := validateWorkspaceConflictSide("current", m.Current); err != nil {
		return err
	}
	if err := validateWorkspaceConflictSide("incoming", m.Incoming); err != nil {
		return err
	}
	switch m.Kind {
	case WorkspaceConflictContent, WorkspaceConflictBinary:
		if !workspaceConflictSideIsLiveFileAt(m.Current, m.Path) {
			return validationError("current", "kind_mismatch")
		}
		if !workspaceConflictSideIsLiveFileAt(m.Incoming, m.Path) {
			return validationError("incoming", "kind_mismatch")
		}
	case WorkspaceConflictDeleteModify:
		if m.Current.Tombstone == m.Incoming.Tombstone {
			return validationError("incoming", "kind_mismatch")
		}
		live := m.Current
		if live.Tombstone {
			live = m.Incoming
		}
		if !workspaceConflictSideIsLiveFileAt(live, m.Path) {
			return validationError("incoming", "kind_mismatch")
		}
	case WorkspaceConflictRename:
		if m.Current.Tombstone || m.Incoming.Tombstone || m.Current.Path == nil || m.Incoming.Path == nil {
			return validationError("incoming", "kind_mismatch")
		}
		if *m.Current.Path == *m.Incoming.Path {
			return validationError("incoming.path", "rename_path_required")
		}
	case "":
		return validationError("kind", "invalid_enum")
	default:
		return validationError("kind", "invalid_enum")
	}
	return nil
}

func (m WorkspaceConflictResolvedRequest) ValidateAgainst(created WorkspaceConflictCreatedMessage) error {
	if m.ConflictRevision == (WorkspaceConflictRevision{}) {
		return validationError("conflictRevision", "must_be_positive")
	}
	if m.ConflictRevision != created.ConflictRevision {
		return validationError("conflictRevision", "conflict_revision_stale")
	}
	if err := created.Validate(); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("workspaceId", string(m.WorkspaceID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("clientId", string(m.ClientID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("operationId", string(m.OperationID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("conflictId", string(m.ConflictID)); err != nil {
		return err
	}
	if m.WorkspaceID != created.WorkspaceID || m.ConflictID != created.ConflictID {
		return validationError("conflictId", "conflict_mismatch")
	}
	if _, err := ParseWorkspacePath(string(m.Path)); err != nil {
		return err
	}
	if !m.ContentHash.Present {
		return validationError("contentHash", "required_key_missing")
	}
	switch m.Choice {
	case WorkspaceConflictKeepCurrent:
		return validateWorkspaceConflictSideReplay(m, created.Current)
	case WorkspaceConflictUseIncoming:
		return validateWorkspaceConflictSideReplay(m, created.Incoming)
	case WorkspaceConflictUseMerged:
		if m.Path != created.Path {
			return validationError("path", "conflict_path_mismatch")
		}
		if m.ContentHash.Value == nil {
			return validationError("contentHash", "required_for_merged")
		}
		if _, err := ParseWorkspaceContentHash(string(*m.ContentHash.Value)); err != nil {
			return err
		}
		return m.Metadata.Validate(WorkspaceEntryFile)
	case WorkspaceConflictDelete:
		if m.Path != created.Path {
			return validationError("path", "conflict_path_mismatch")
		}
		if m.ContentHash.Value != nil {
			return validationError("contentHash", "must_be_null_for_delete")
		}
		if m.Metadata.Size != 0 {
			return validationError("metadata.size", "must_be_zero_for_delete")
		}
		if m.Metadata.ModifiedAtMS != 0 {
			return validationError("metadata.modifiedAtMs", "must_be_zero_for_delete")
		}
		if m.Metadata.Executable {
			return validationError("metadata.executable", "must_be_false_for_delete")
		}
		return nil
	default:
		return validationError("choice", "invalid_enum")
	}
}

func (m WorkspaceConflictResolvedMessage) Validate() error {
	if _, err := ParseWorkspaceUUID("workspaceId", string(m.WorkspaceID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("conflictId", string(m.ConflictID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("operationId", string(m.OperationID)); err != nil {
		return err
	}
	if _, err := ParseWorkspaceUUID("resolvedByClientId", string(m.ResolvedByClientID)); err != nil {
		return err
	}
	if m.ConflictRevision == (WorkspaceConflictRevision{}) {
		return validationError("conflictRevision", "must_be_positive")
	}
	if m.Revision == 0 {
		return validationError("revision", "must_be_positive")
	}
	switch m.Choice {
	case WorkspaceConflictKeepCurrent, WorkspaceConflictUseIncoming, WorkspaceConflictUseMerged, WorkspaceConflictDelete:
	default:
		return validationError("choice", "invalid_enum")
	}
	if m.PathState.PathRevision != m.Revision {
		return validationError("pathState.pathRevision", "revision_mismatch")
	}
	return m.PathState.Validate()
}

func validateWorkspaceConflictSide(field string, side WorkspaceConflictSide) error {
	if !side.ContentHash.Present {
		return validationError(field+".contentHash", "required_key_missing")
	}
	if side.Path == nil {
		if !side.Tombstone {
			return validationError(field+".path", "null_requires_tombstone")
		}
	} else if _, err := ParseWorkspacePath(string(*side.Path)); err != nil {
		return validationError(field+".path", "invalid_path")
	}
	if side.Tombstone {
		if side.ContentHash.Value != nil {
			return validationError(field+".contentHash", "must_be_null_for_tombstone")
		}
		return side.Metadata.Validate(WorkspaceEntryTombstone)
	}
	if side.ContentHash.Value == nil {
		return side.Metadata.Validate(WorkspaceEntryDirectory)
	}
	if _, err := ParseWorkspaceContentHash(string(*side.ContentHash.Value)); err != nil {
		return validationError(field+".contentHash", "invalid_blake3")
	}
	return side.Metadata.Validate(WorkspaceEntryFile)
}

func workspaceConflictSideIsLiveFileAt(side WorkspaceConflictSide, path WorkspacePath) bool {
	return !side.Tombstone && side.Path != nil && *side.Path == path && side.ContentHash.Value != nil
}

func validateWorkspaceConflictSideReplay(request WorkspaceConflictResolvedRequest, side WorkspaceConflictSide) error {
	if side.Path == nil || side.Tombstone || request.Path != *side.Path ||
		!workspaceNullableHashEqual(request.ContentHash, side.ContentHash) || request.Metadata != side.Metadata {
		return validationError("choice", "side_mismatch")
	}
	return nil
}

func workspaceNullableHashEqual(left, right WorkspaceNullableHash) bool {
	if left.Present != right.Present || (left.Value == nil) != (right.Value == nil) {
		return false
	}
	return left.Value == nil || *left.Value == *right.Value
}

func workspacePathStateEqual(left, right WorkspacePathState) bool {
	return left.Path == right.Path &&
		left.PathRevision == right.PathRevision &&
		left.Kind == right.Kind &&
		workspaceNullableHashEqual(left.ContentHash, right.ContentHash) &&
		left.Metadata == right.Metadata &&
		left.Tombstone == right.Tombstone
}

type WorkspaceV2DataFactory func() any

type WorkspaceV2ActionSpec struct {
	Flows map[WorkspaceV2Flow]WorkspaceV2DataFactory
}

func workspaceV2Factory[T any]() WorkspaceV2DataFactory {
	return func() any { return new(T) }
}

var WorkspaceV2ActionSpecs = map[WorkspaceV2Action]WorkspaceV2ActionSpec{
	WorkspaceActionHello: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowClientRequest: workspaceV2Factory[WorkspaceHelloRequest](), WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceHelloResponse](),
	}},
	WorkspaceActionSubscribe: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowClientRequest: workspaceV2Factory[WorkspaceSubscribeRequest](),
	}},
	WorkspaceActionSnapshotBegin: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceSnapshotBeginMessage](),
	}},
	WorkspaceActionSnapshotEntry: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceSnapshotEntryMessage](),
	}},
	WorkspaceActionSnapshotEnd: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceSnapshotEndMessage](),
	}},
	WorkspaceActionMutation: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowClientRequest: workspaceV2Factory[WorkspaceMutation](),
	}},
	WorkspaceActionMutationAccepted: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceMutationAcceptedMessage](),
	}},
	WorkspaceActionMutationRejected: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceMutationRejectedMessage](),
	}},
	WorkspaceActionEvent: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceEventMessage](),
	}},
	WorkspaceActionAck: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowClientRequest: workspaceV2Factory[WorkspaceAckRequest](), WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceAckRequest](),
	}},
	WorkspaceActionBlobNeed: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowClientRequest:  workspaceV2Factory[WorkspaceBlobNeedDownloadRequest](),
		WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceBlobNeedDownloadResponse](),
		WorkspaceFlowServerPush:     workspaceV2Factory[WorkspaceBlobNeedUploadPush](),
	}},
	WorkspaceActionBlobBegin: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowClientRequest:  workspaceV2Factory[WorkspaceBlobBeginMessage](),
		WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceBlobBeginMessage](),
		WorkspaceFlowServerPush:     workspaceV2Factory[WorkspaceBlobBeginMessage](),
	}},
	WorkspaceActionBlobEnd: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowClientRequest:  workspaceV2Factory[WorkspaceBlobEndMessage](),
		WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceBlobEndMessage](),
		WorkspaceFlowServerPush:     workspaceV2Factory[WorkspaceBlobEndMessage](),
	}},
	WorkspaceActionConflictCreated: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowServerPush: workspaceV2Factory[WorkspaceConflictCreatedMessage](),
	}},
	WorkspaceActionConflictResolved: {Flows: map[WorkspaceV2Flow]WorkspaceV2DataFactory{
		WorkspaceFlowClientRequest:  workspaceV2Factory[WorkspaceConflictResolvedRequest](),
		WorkspaceFlowServerResponse: workspaceV2Factory[WorkspaceConflictResolvedMessage](),
		WorkspaceFlowServerPush:     workspaceV2Factory[WorkspaceConflictResolvedMessage](),
	}},
}

func NewWorkspaceV2Data(action WorkspaceV2Action, flow WorkspaceV2Flow) (any, error) {
	spec, ok := WorkspaceV2ActionSpecs[action]
	if !ok {
		return nil, validationError("action", "unknown_action")
	}
	factory, ok := spec.Flows[flow]
	if !ok {
		return nil, validationError("flow", "flow_not_allowed")
	}
	return factory(), nil
}

func DecodeWorkspaceV2Data(action WorkspaceV2Action, flow WorkspaceV2Flow, data []byte) (any, error) {
	dst, err := NewWorkspaceV2Data(action, flow)
	if err != nil {
		return nil, err
	}
	if err := strictJSONDecode(data, dst); err != nil {
		return nil, validationError("data", "invalid_json")
	}
	return dst, nil
}
