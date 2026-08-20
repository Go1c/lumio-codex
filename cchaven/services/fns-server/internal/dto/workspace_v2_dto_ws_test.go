package dto

import (
	"encoding/binary"
	"encoding/json"
	"math"
	"reflect"
	"strings"
	"testing"

	"github.com/google/uuid"
	"github.com/stretchr/testify/require"
	"github.com/zeebo/blake3"
)

func requireWorkspaceValidationError(t *testing.T, err error, field, reason string) {
	t.Helper()
	var validationErr *WorkspaceValidationError
	require.ErrorAs(t, err, &validationErr)
	require.Equal(t, field, validationErr.Field)
	require.Equal(t, reason, validationErr.Reason)
}

func TestWorkspaceRevisionJSON(t *testing.T) {
	t.Parallel()
	tests := []struct {
		name    string
		input   string
		want    WorkspaceRevision
		wantErr string
	}{
		{name: "zero", input: `"0"`, want: 0},
		{name: "max", input: `"18446744073709551615"`, want: WorkspaceRevision(math.MaxUint64)},
		{name: "number rejected", input: `1`, wantErr: "must_be_string"},
		{name: "negative rejected", input: `"-1"`, wantErr: "non_canonical_decimal"},
		{name: "empty", input: `""`, wantErr: "empty"},
		{name: "leading zero", input: `"01"`, wantErr: "non_canonical_decimal"},
		{name: "overflow", input: `"18446744073709551616"`, wantErr: "non_canonical_decimal"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var got WorkspaceRevision
			err := json.Unmarshal([]byte(tt.input), &got)
			if tt.wantErr == "" {
				require.NoError(t, err)
				require.Equal(t, tt.want, got)
				encoded, err := json.Marshal(got)
				require.NoError(t, err)
				require.Equal(t, tt.input, string(encoded))
				return
			}
			requireWorkspaceValidationError(t, err, "revision", tt.wantErr)
		})
	}
}

func TestWorkspaceConflictRevisionPublicTypeIsOpaque(t *testing.T) {
	t.Parallel()
	conflictRevisionType := reflect.TypeOf(WorkspaceConflictRevision{})
	require.Equal(t, reflect.Struct, conflictRevisionType.Kind())
	require.True(t, conflictRevisionType.Comparable())
	require.NotEqual(t, reflect.TypeOf(WorkspaceRevision(0)), conflictRevisionType)
	require.Equal(t, 1, conflictRevisionType.NumField())
	require.False(t, conflictRevisionType.Field(0).IsExported())

	for _, messageType := range []reflect.Type{
		reflect.TypeOf(WorkspaceConflictCreatedMessage{}),
		reflect.TypeOf(WorkspaceConflictResolvedRequest{}),
		reflect.TypeOf(WorkspaceConflictResolvedMessage{}),
	} {
		field, ok := messageType.FieldByName("ConflictRevision")
		require.True(t, ok)
		require.Equal(t, conflictRevisionType, field.Type)
	}
}

func TestWorkspaceConflictRevisionJSON(t *testing.T) {
	t.Parallel()
	for _, input := range []string{`"1"`, `"18446744073709551615"`} {
		text := strings.Trim(input, `"`)
		parsed, err := ParseWorkspaceConflictRevision(text)
		require.NoError(t, err)

		var decoded WorkspaceConflictRevision
		require.NoError(t, json.Unmarshal([]byte(input), &decoded))
		require.Equal(t, parsed, decoded)

		encoded, err := json.Marshal(decoded)
		require.NoError(t, err)
		require.Equal(t, input, string(encoded))
	}

	for _, tt := range []struct {
		input  string
		reason string
	}{
		{input: `"0"`, reason: "must_be_positive"},
		{input: `0`, reason: "must_be_string"},
		{input: `""`, reason: "empty"},
		{input: `"-1"`, reason: "non_canonical_decimal"},
		{input: `"01"`, reason: "non_canonical_decimal"},
		{input: `"18446744073709551616"`, reason: "non_canonical_decimal"},
	} {
		var decoded WorkspaceConflictRevision
		err := json.Unmarshal([]byte(tt.input), &decoded)
		requireWorkspaceValidationError(t, err, "conflictRevision", tt.reason)
	}

	_, err := json.Marshal(WorkspaceConflictRevision{})
	requireWorkspaceValidationError(t, err, "conflictRevision", "must_be_positive")
}

func workspaceTestConflictRevision(t *testing.T, value string) WorkspaceConflictRevision {
	t.Helper()
	revision, err := ParseWorkspaceConflictRevision(value)
	require.NoError(t, err)
	return revision
}

func TestWorkspaceContentHashJSON(t *testing.T) {
	t.Parallel()
	valid := "blake3:" + strings.Repeat("0a", 32)
	parsed, err := ParseWorkspaceContentHash(valid)
	require.NoError(t, err)
	require.Equal(t, WorkspaceContentHash(valid), parsed)
	raw, err := json.Marshal(parsed)
	require.NoError(t, err)
	require.Equal(t, `"`+valid+`"`, string(raw))

	for _, value := range []string{
		"sha256:" + strings.Repeat("a", 64),
		"blake3:" + strings.Repeat("a", 63),
		"blake3:" + strings.Repeat("A", 64),
		"blake3:" + strings.Repeat("z", 64),
	} {
		_, err := ParseWorkspaceContentHash(value)
		requireWorkspaceValidationError(t, err, "contentHash", "invalid_blake3")
	}

	var decoded WorkspaceContentHash
	requireWorkspaceValidationError(t, json.Unmarshal([]byte(`12`), &decoded), "contentHash", "must_be_string")
	require.NoError(t, json.Unmarshal([]byte(`"`+valid+`"`), &decoded))
	require.Equal(t, parsed, decoded)
}

func TestWorkspacePathValidation(t *testing.T) {
	t.Parallel()
	valid4096 := strings.Repeat("a", 4096)
	for _, value := range []string{"notes/café.md", valid4096} {
		parsed, err := ParseWorkspacePath(value)
		require.NoError(t, err)
		require.Equal(t, WorkspacePath(value), parsed)
		raw, err := json.Marshal(parsed)
		require.NoError(t, err)
		var roundTrip WorkspacePath
		require.NoError(t, json.Unmarshal(raw, &roundTrip))
		require.Equal(t, parsed, roundTrip)
	}

	tests := []struct {
		value  string
		reason string
	}{
		{value: "", reason: "invalid_length_or_utf8"},
		{value: strings.Repeat("a", 4097), reason: "invalid_length_or_utf8"},
		{value: "/root", reason: "not_relative_posix"},
		{value: "root/", reason: "not_relative_posix"},
		{value: "a//b", reason: "not_relative_posix"},
		{value: `a\b`, reason: "not_relative_posix"},
		{value: "a/../b", reason: "invalid_segment"},
		{value: "a/./b", reason: "invalid_segment"},
		{value: "caf\u0065\u0301.md", reason: "not_nfc"},
		{value: "a\x00b", reason: "unsafe_character"},
		{value: "bad?.md", reason: "unsafe_character"},
		{value: "name. ", reason: "windows_unsafe_suffix"},
		{value: "CON.txt", reason: "windows_device_name"},
		{value: "dir/lpt9", reason: "windows_device_name"},
	}
	for _, tt := range tests {
		_, err := ParseWorkspacePath(tt.value)
		requireWorkspaceValidationError(t, err, "path", tt.reason)
	}
}

func TestWorkspacePrimitivesUUIDNullableHashAndMetadata(t *testing.T) {
	t.Parallel()
	id := "10000000-0000-4000-8000-00000000000a"
	gotID, err := ParseWorkspaceUUID("clientId", id)
	require.NoError(t, err)
	require.Equal(t, WorkspaceUUID(id), gotID)
	_, err = ParseWorkspaceUUID("clientId", strings.ToUpper(id))
	requireWorkspaceValidationError(t, err, "clientId", "invalid_uuid")

	var nullHash WorkspaceNullableHash
	require.NoError(t, json.Unmarshal([]byte("null"), &nullHash))
	require.True(t, nullHash.Present)
	require.Nil(t, nullHash.Value)
	raw, err := json.Marshal(nullHash)
	require.NoError(t, err)
	require.Equal(t, "null", string(raw))

	hashText := "blake3:" + strings.Repeat("ab", 32)
	var valueHash WorkspaceNullableHash
	require.NoError(t, json.Unmarshal([]byte(`"`+hashText+`"`), &valueHash))
	require.True(t, valueHash.Present)
	require.Equal(t, WorkspaceContentHash(hashText), *valueHash.Value)

	_, err = json.Marshal(WorkspaceNullableHash{})
	requireWorkspaceValidationError(t, err, "contentHash", "required_key_missing")

	var nullUUID WorkspaceNullableUUID
	require.NoError(t, json.Unmarshal([]byte("null"), &nullUUID))
	require.True(t, nullUUID.Present)
	require.Nil(t, nullUUID.Value)
	_, err = json.Marshal(WorkspaceNullableUUID{})
	requireWorkspaceValidationError(t, err, "uuid", "required_key_missing")

	var nullUint64 WorkspaceNullableUint64
	require.NoError(t, json.Unmarshal([]byte("null"), &nullUint64))
	require.True(t, nullUint64.Present)
	require.Nil(t, nullUint64.Value)
	_, err = json.Marshal(WorkspaceNullableUint64{})
	requireWorkspaceValidationError(t, err, "uint64", "required_key_missing")

	require.NoError(t, (WorkspaceFileMetadata{Size: 1, ModifiedAtMS: 1, Executable: true}).Validate(WorkspaceEntryFile))
	requireWorkspaceValidationError(t, (WorkspaceFileMetadata{Size: WorkspaceMaxBlobBytes + 1}).Validate(WorkspaceEntryFile), "metadata.size", "limit_exceeded")
	requireWorkspaceValidationError(t, (WorkspaceFileMetadata{ModifiedAtMS: -1}).Validate(WorkspaceEntryFile), "metadata.modifiedAtMs", "out_of_range")
	requireWorkspaceValidationError(t, (WorkspaceFileMetadata{Size: 1}).Validate(WorkspaceEntryDirectory), "metadata.size", "must_be_zero")
	requireWorkspaceValidationError(t, (WorkspaceFileMetadata{Executable: true}).Validate(WorkspaceEntryTombstone), "metadata.executable", "must_be_false")
}

func TestWorkspaceV2EnvelopeStrictness(t *testing.T) {
	t.Parallel()
	type payload struct {
		Value string `json:"value"`
	}
	requestID := WorkspaceUUID("10000000-0000-4000-8000-000000000001")
	var request WorkspaceV2Request[payload]
	require.NoError(t, DecodeWorkspaceV2Request([]byte(`{"requestId":"10000000-0000-4000-8000-000000000001","data":{"value":"x"}}`), &request))
	require.Equal(t, requestID, request.RequestID)
	require.Equal(t, "x", request.Data.Value)

	for _, raw := range []string{
		`{"requestId":"10000000-0000-4000-8000-000000000001","requestId":"10000000-0000-4000-8000-000000000001","data":{"value":"x"}}`,
		`{"requestId":"10000000-0000-4000-8000-000000000001","data":{"value":"x","value":"y"}}`,
		`{"requestId":"10000000-0000-4000-8000-000000000001","data":{"value":"x","extra":true}}`,
		`{"requestId":"10000000-0000-4000-8000-000000000001","data":{"value":"x"}} {}`,
	} {
		err := DecodeWorkspaceV2Request([]byte(raw), &request)
		requireWorkspaceValidationError(t, err, "frame", "invalid_json")
	}

	data := payload{Value: "ok"}
	requireWorkspaceValidationError(t, (WorkspaceV2Response[payload]{Status: true}).Validate(), "response", "success_requires_data")
	errValue := NewWorkspaceV2Error(WorkspaceErrorInvalidRequest)
	requireWorkspaceValidationError(t, (WorkspaceV2Response[payload]{Status: true, Data: &data, Error: &errValue}).Validate(), "response", "success_forbids_error")
	requireWorkspaceValidationError(t, (WorkspaceV2Response[payload]{Status: false}).Validate(), "response", "error_requires_error")
	requireWorkspaceValidationError(t, (WorkspaceV2Response[payload]{Status: false, Data: &data, Error: &errValue}).Validate(), "response", "error_forbids_data")
}

func TestEncodeWorkspaceV2ResponseEnforcesRegistryFlowAndType(t *testing.T) {
	t.Parallel()
	requestID := workspaceTestUUID(6)
	hello := WorkspaceHelloResponse{
		ProtocolVersion: "2", ServerVersion: "2.0.0", MaxControlFrameBytes: WorkspaceMaxControlFrameBytes,
		MaxBinaryChunkBytes: WorkspaceBlobChunkSize, MaxBlobBytes: WorkspaceMaxBlobBytes,
		MaxTransfersPerConnection: 4, HeartbeatSeconds: 25,
	}
	encoded, err := EncodeWorkspaceV2Response(WorkspaceActionHello, WorkspaceV2Response[WorkspaceHelloResponse]{
		RequestID: &requestID, Status: true, Data: &hello,
	})
	require.NoError(t, err)
	require.Equal(t, `WorkspaceHello|{"requestId":"10000000-0000-4000-8000-000000000006","status":true,"data":{"protocolVersion":"2","serverVersion":"2.0.0","maxControlFrameBytes":65536,"maxBinaryChunkBytes":1048576,"maxBlobBytes":5368709120,"maxTransfersPerConnection":4,"heartbeatSeconds":25}}`, string(encoded))

	begin := WorkspaceSnapshotBeginMessage{
		WorkspaceID: workspaceTestUUID(2), StreamID: workspaceTestUUID(3), Mode: WorkspaceSnapshotFull,
		FinalRevision: 1, EntryCount: 1,
	}
	encoded, err = EncodeWorkspaceV2Response(WorkspaceActionSnapshotBegin, WorkspaceV2Response[WorkspaceSnapshotBeginMessage]{Status: true, Data: &begin})
	require.NoError(t, err)
	require.True(t, strings.HasPrefix(string(encoded), `WorkspaceSnapshotBegin|{"status":true,"data":`))

	subscribe := WorkspaceSubscribeRequest{WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1)}
	_, err = EncodeWorkspaceV2Response(WorkspaceActionSubscribe, WorkspaceV2Response[WorkspaceSubscribeRequest]{RequestID: &requestID, Status: true, Data: &subscribe})
	requireWorkspaceValidationError(t, err, "flow", "flow_not_allowed")

	ack := WorkspaceAckRequest{WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1), Revision: 1}
	_, err = EncodeWorkspaceV2Response(WorkspaceActionHello, WorkspaceV2Response[WorkspaceAckRequest]{RequestID: &requestID, Status: true, Data: &ack})
	requireWorkspaceValidationError(t, err, "data", "type_mismatch")

	failure := NewWorkspaceV2Error(WorkspaceErrorInvalidRequest)
	encoded, err = EncodeWorkspaceV2Response(WorkspaceActionSubscribe, WorkspaceV2Response[WorkspaceSubscribeRequest]{
		RequestID: &requestID, Status: false, Error: &failure,
	})
	require.NoError(t, err)
	require.Equal(t, `WorkspaceSubscribe|{"requestId":"10000000-0000-4000-8000-000000000006","status":false,"error":{"code":"invalid_request","message":"invalid request","retryable":false}}`, string(encoded))

	encoded, err = EncodeWorkspaceV2Response(WorkspaceActionEvent, WorkspaceV2Response[WorkspaceEventMessage]{
		RequestID: &requestID, Status: false, Error: &failure,
	})
	require.NoError(t, err)
	require.Equal(t, `WorkspaceEvent|{"requestId":"10000000-0000-4000-8000-000000000006","status":false,"error":{"code":"invalid_request","message":"invalid request","retryable":false}}`, string(encoded))
}

func TestWorkspaceV2OptionalWirePointersRejectExplicitNull(t *testing.T) {
	t.Parallel()
	requestID := workspaceTestUUID(6)
	ack := WorkspaceAckRequest{WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1), Revision: 1}
	success := WorkspaceV2Response[WorkspaceAckRequest]{RequestID: &requestID, Status: true, Data: &ack}
	for _, field := range []string{"requestId", "data"} {
		raw := workspaceTestJSONFieldNull(t, success, field)
		var decoded WorkspaceV2Response[WorkspaceAckRequest]
		require.Error(t, strictJSONDecode(raw, &decoded), field)
	}
	failureValue := NewWorkspaceV2Error(WorkspaceErrorInvalidRequest)
	failure := WorkspaceV2Response[WorkspaceAckRequest]{RequestID: &requestID, Status: false, Error: &failureValue}
	raw := workspaceTestJSONFieldNull(t, failure, "error")
	var decodedFailure WorkspaceV2Response[WorkspaceAckRequest]
	require.Error(t, strictJSONDecode(raw, &decodedFailure))

	newPath := WorkspacePath("archive/a.md")
	targetRevision := WorkspaceRevision(0)
	mutation := WorkspaceMutation{
		WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1), OperationID: workspaceTestUUID(4),
		Path: "notes/a.md", BasePathRevision: 1, Kind: WorkspaceMutationRename,
		ContentHash: workspaceHashValue(), Metadata: WorkspaceFileMetadata{Size: 3, ModifiedAtMS: 1},
		NewPath: &newPath, TargetBasePathRevision: &targetRevision,
	}
	for _, field := range []string{"newPath", "targetBasePathRevision"} {
		raw := workspaceTestJSONFieldNull(t, mutation, field)
		var decoded WorkspaceMutation
		require.Error(t, strictJSONDecode(raw, &decoded), field)
	}

	oldState := WorkspacePathState{
		Path: "notes/a.md", PathRevision: 2, Kind: WorkspaceEntryTombstone,
		ContentHash: workspaceNullHash(), Metadata: WorkspaceFileMetadata{}, Tombstone: true,
	}
	newState := WorkspacePathState{
		Path: newPath, PathRevision: 2, Kind: WorkspaceEntryFile,
		ContentHash: workspaceHashValue(), Metadata: WorkspaceFileMetadata{Size: 3, ModifiedAtMS: 1},
	}
	accepted := WorkspaceMutationAcceptedMessage{
		WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1), OperationID: workspaceTestUUID(4),
		Revision: 2, PathState: newState, OldPathState: &oldState, NewPathState: &newState,
	}
	event := WorkspaceEventMessage{
		WorkspaceID: workspaceTestUUID(2), StreamID: workspaceTestUUID(3), Index: 1, Revision: 2,
		OperationID: workspaceTestUUID(4), OriginClientID: workspaceTestUUID(1), Mutation: mutation,
		PathState: newState, OldPathState: &oldState, NewPathState: &newState,
	}
	for _, tt := range []struct {
		name  string
		value any
		dst   any
	}{
		{name: "accepted old", value: accepted, dst: &WorkspaceMutationAcceptedMessage{}},
		{name: "accepted new", value: accepted, dst: &WorkspaceMutationAcceptedMessage{}},
		{name: "event old", value: event, dst: &WorkspaceEventMessage{}},
		{name: "event new", value: event, dst: &WorkspaceEventMessage{}},
	} {
		field := "oldPathState"
		if strings.HasSuffix(tt.name, "new") {
			field = "newPathState"
		}
		t.Run(tt.name, func(t *testing.T) {
			raw := workspaceTestJSONFieldNull(t, tt.value, field)
			require.Error(t, strictJSONDecode(raw, tt.dst))
		})
	}
}

func TestWorkspaceV2StrictDecoderNullKinds(t *testing.T) {
	t.Parallel()
	type nested struct {
		Value string `json:"value"`
	}
	type schema struct {
		Boolean bool              `json:"boolean"`
		Number  uint32            `json:"number"`
		Text    string            `json:"text"`
		Struct  nested            `json:"struct"`
		Slice   []string          `json:"slice"`
		Array   [1]string         `json:"array"`
		Map     map[string]string `json:"map"`
		Raw     json.RawMessage   `json:"raw"`
	}
	valid := schema{
		Struct: nested{Value: "value"}, Slice: []string{}, Array: [1]string{"value"},
		Map: map[string]string{}, Raw: json.RawMessage("null"),
	}
	for _, field := range []string{"boolean", "number", "text", "struct", "slice", "array", "map"} {
		raw := workspaceTestJSONFieldNull(t, valid, field)
		var decoded schema
		require.Error(t, strictJSONDecode(raw, &decoded), field)
	}
	raw, err := json.Marshal(valid)
	require.NoError(t, err)
	var decoded schema
	require.NoError(t, strictJSONDecode(raw, &decoded))
	require.Equal(t, json.RawMessage("null"), decoded.Raw)
}

func workspaceTestJSONFieldNull(t *testing.T, value any, field string) []byte {
	t.Helper()
	raw, err := json.Marshal(value)
	require.NoError(t, err)
	var object map[string]json.RawMessage
	require.NoError(t, json.Unmarshal(raw, &object))
	require.Contains(t, object, field)
	object[field] = json.RawMessage("null")
	raw, err = json.Marshal(object)
	require.NoError(t, err)
	return raw
}

func TestEncodeWorkspaceV2UnknownActionFailure(t *testing.T) {
	t.Parallel()
	requestID := workspaceTestUUID(6)
	encoded, err := EncodeWorkspaceV2UnknownActionFailure("WorkspaceFuture1", &requestID)
	require.NoError(t, err)
	require.Equal(t, `WorkspaceFuture1|{"requestId":"10000000-0000-4000-8000-000000000006","status":false,"error":{"code":"unknown_action","message":"unknown workspace action","retryable":false}}`, string(encoded))
	require.Len(t, WorkspaceV2Actions, 15)
	require.NotContains(t, WorkspaceV2Actions, WorkspaceV2Action("WorkspaceFuture1"))

	for _, token := range []string{"", "1Workspace", "Workspace-Future", "WorkspaceFuture|injected", "WorkspaceFuturé", "A" + strings.Repeat("0", 64)} {
		_, err := EncodeWorkspaceV2UnknownActionFailure(token, nil)
		requireWorkspaceValidationError(t, err, "action", "invalid_token")
		if token != "" {
			require.NotContains(t, err.Error(), token)
		}
	}

	_, err = EncodeWorkspaceV2UnknownActionFailure(string(WorkspaceActionHello), nil)
	requireWorkspaceValidationError(t, err, "action", "registered_action")
}

func TestWorkspaceV2ErrorCodes(t *testing.T) {
	t.Parallel()
	want := []WorkspaceV2ErrorCode{
		WorkspaceErrorInvalidFrame, WorkspaceErrorInvalidJSON, WorkspaceErrorUnknownAction,
		WorkspaceErrorUnauthenticated, WorkspaceErrorForbidden, WorkspaceErrorInvalidRequest,
		WorkspaceErrorInvalidRevision, WorkspaceErrorInvalidHash, WorkspaceErrorInvalidPath,
		WorkspaceErrorWorkspaceNotFound, WorkspaceErrorWorkspaceLimitExceeded, WorkspaceErrorClientNotRegistered,
		WorkspaceErrorStaleBaseRevision, WorkspaceErrorOperationReused, WorkspaceErrorBlobRequired, WorkspaceErrorBlobNotFound,
		WorkspaceErrorBlobHashMismatch, WorkspaceErrorBlobSizeMismatch, WorkspaceErrorBlobTransferOutOfOrder,
		WorkspaceErrorBlobLimitExceeded, WorkspaceErrorConflictNotFound, WorkspaceErrorConflictRevisionStale,
		WorkspaceErrorServerBusy, WorkspaceErrorInternal,
	}
	require.Len(t, WorkspaceV2ErrorCodes, 24)
	require.ElementsMatch(t, want, WorkspaceV2ErrorCodes)
	for _, code := range want {
		errValue := NewWorkspaceV2Error(code, WorkspaceV2FieldError{Field: "data.path", Reason: "invalid"})
		require.Equal(t, code, errValue.Code)
		require.NotEmpty(t, errValue.Message)
		require.Equal(t, code == WorkspaceErrorServerBusy || code == WorkspaceErrorInternal, errValue.Retryable)
		require.Len(t, errValue.Fields, 1)
	}
}

func TestWorkspaceV2ActionNames(t *testing.T) {
	t.Parallel()
	want := []WorkspaceV2Action{
		WorkspaceActionHello, WorkspaceActionSubscribe,
		WorkspaceActionSnapshotBegin, WorkspaceActionSnapshotEntry, WorkspaceActionSnapshotEnd,
		WorkspaceActionMutation, WorkspaceActionMutationAccepted, WorkspaceActionMutationRejected,
		WorkspaceActionEvent, WorkspaceActionAck,
		WorkspaceActionBlobNeed, WorkspaceActionBlobBegin, WorkspaceActionBlobEnd,
		WorkspaceActionConflictCreated, WorkspaceActionConflictResolved,
	}
	require.Equal(t, want, WorkspaceV2Actions)
	require.Len(t, WorkspaceV2Actions, 15)
	require.Equal(t, WorkspaceV2Flow("client_request"), WorkspaceFlowClientRequest)
	require.Equal(t, WorkspaceV2Flow("server_response"), WorkspaceFlowServerResponse)
	require.Equal(t, WorkspaceV2Flow("server_push"), WorkspaceFlowServerPush)
}

func workspaceTestUUID(n byte) WorkspaceUUID {
	return WorkspaceUUID("10000000-0000-4000-8000-00000000000" + string([]byte{'0' + n}))
}

func workspaceTestHash() WorkspaceContentHash {
	return WorkspaceContentHash("blake3:" + strings.Repeat("ab", 32))
}

func workspaceHashValue() WorkspaceNullableHash {
	h := workspaceTestHash()
	return WorkspaceNullableHash{Present: true, Value: &h}
}

func workspaceNullHash() WorkspaceNullableHash {
	return WorkspaceNullableHash{Present: true}
}

func TestWorkspaceV2HelloAndSubscribe(t *testing.T) {
	t.Parallel()
	hello := WorkspaceHelloRequest{
		ProtocolVersion: "2", ClientID: workspaceTestUUID(1), ClientVersion: "1.0.0",
		Capabilities: []string{"binary_chunks", "conflicts", "snapshot_v1"},
	}
	require.NoError(t, hello.Validate())
	raw, err := json.Marshal(hello)
	require.NoError(t, err)
	var decoded WorkspaceHelloRequest
	require.NoError(t, strictJSONDecode(raw, &decoded))
	require.Equal(t, hello, decoded)

	hello.ProtocolVersion = "1"
	requireWorkspaceValidationError(t, hello.Validate(), "protocolVersion", "unsupported")
	hello.ProtocolVersion = "2"
	hello.Capabilities = []string{"conflicts"}
	requireWorkspaceValidationError(t, hello.Validate(), "capabilities", "required_set")

	response := WorkspaceHelloResponse{
		ProtocolVersion: "2", ServerVersion: "2.0.0", MaxControlFrameBytes: WorkspaceMaxControlFrameBytes,
		MaxBinaryChunkBytes: WorkspaceBlobChunkSize, MaxBlobBytes: WorkspaceMaxBlobBytes,
		MaxTransfersPerConnection: 4, HeartbeatSeconds: 25,
	}
	require.NoError(t, response.Validate())

	subscribe := WorkspaceSubscribeRequest{WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1), LastAckRevision: 0}
	require.NoError(t, subscribe.Validate())
}

func TestWorkspaceV2SnapshotValidation(t *testing.T) {
	t.Parallel()
	begin := WorkspaceSnapshotBeginMessage{
		WorkspaceID: workspaceTestUUID(2), StreamID: workspaceTestUUID(3), Mode: WorkspaceSnapshotFull,
		FromRevision: 0, FinalRevision: 2, EntryCount: 2,
	}
	require.NoError(t, begin.Validate())
	begin.EventCount = 1
	requireWorkspaceValidationError(t, begin.Validate(), "eventCount", "must_be_zero_for_snapshot")
	begin = WorkspaceSnapshotBeginMessage{
		WorkspaceID: workspaceTestUUID(2), StreamID: workspaceTestUUID(3), Mode: WorkspaceSnapshotIncremental,
		FromRevision: 1, FinalRevision: 2, EventCount: 1,
	}
	require.NoError(t, begin.Validate())
	begin.EntryCount = 1
	requireWorkspaceValidationError(t, begin.Validate(), "entryCount", "must_be_zero_for_incremental")
	begin.EntryCount = 0
	begin.FinalRevision = 0
	requireWorkspaceValidationError(t, begin.Validate(), "finalRevision", "before_from_revision")

	state := WorkspacePathState{
		Path: "notes/café.md", PathRevision: 2, Kind: WorkspaceEntryFile,
		ContentHash: workspaceHashValue(), Metadata: WorkspaceFileMetadata{Size: 4, ModifiedAtMS: 1},
	}
	require.NoError(t, state.Validate())
	entry := WorkspaceSnapshotEntryMessage{WorkspaceID: workspaceTestUUID(2), StreamID: workspaceTestUUID(3), Index: 0, Entry: state}
	require.NoError(t, entry.Validate(0))
	requireWorkspaceValidationError(t, entry.Validate(1), "index", "stream_gap")

	end := WorkspaceSnapshotEndMessage{
		WorkspaceID: workspaceTestUUID(2), StreamID: workspaceTestUUID(3), Mode: WorkspaceSnapshotIncremental,
		DeliveredCount: 1, FinalRevision: 2,
	}
	begin.FinalRevision = 2
	require.NoError(t, end.ValidateAgainst(begin))
	end.DeliveredCount = 2
	requireWorkspaceValidationError(t, end.ValidateAgainst(begin), "deliveredCount", "count_mismatch")
}

func TestWorkspaceSnapshotBeginRequiresConflictCountKey(t *testing.T) {
	t.Parallel()
	raw := []byte(`{"workspaceId":"10000000-0000-4000-8000-000000000002","streamId":"10000000-0000-4000-8000-000000000003","mode":"snapshot","fromRevision":"0","finalRevision":"1","entryCount":1,"eventCount":0}`)
	var begin WorkspaceSnapshotBeginMessage
	err := strictJSONDecode(raw, &begin)
	require.ErrorContains(t, err, `required JSON key "conflictCount" missing`)
}

func TestWorkspaceSnapshotEndCountsPendingConflicts(t *testing.T) {
	t.Parallel()
	begin := WorkspaceSnapshotBeginMessage{
		WorkspaceID: workspaceTestUUID(2), StreamID: workspaceTestUUID(3), Mode: WorkspaceSnapshotFull,
		FinalRevision: 2, EntryCount: 2, ConflictCount: 3,
	}
	end := WorkspaceSnapshotEndMessage{
		WorkspaceID: workspaceTestUUID(2), StreamID: workspaceTestUUID(3), Mode: WorkspaceSnapshotFull,
		DeliveredCount: 2, FinalRevision: 2,
	}
	requireWorkspaceValidationError(t, end.ValidateAgainst(begin), "deliveredCount", "count_mismatch")
	end.DeliveredCount = 5
	require.NoError(t, end.ValidateAgainst(begin))
}

func TestWorkspaceSnapshotEndRejectsDeliveredCountOverflow(t *testing.T) {
	t.Parallel()
	begin := WorkspaceSnapshotBeginMessage{
		WorkspaceID: workspaceTestUUID(2), StreamID: workspaceTestUUID(3), Mode: WorkspaceSnapshotFull,
		FinalRevision: 2, EntryCount: math.MaxUint32, ConflictCount: 1,
	}
	end := WorkspaceSnapshotEndMessage{
		WorkspaceID: workspaceTestUUID(2), StreamID: workspaceTestUUID(3), Mode: WorkspaceSnapshotFull,
		DeliveredCount: math.MaxUint32, FinalRevision: 2,
	}
	requireWorkspaceValidationError(t, end.ValidateAgainst(begin), "deliveredCount", "count_overflow")
}

func TestWorkspaceV2MutationValidation(t *testing.T) {
	t.Parallel()
	base := WorkspaceMutation{
		WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1), OperationID: workspaceTestUUID(4),
		Path: "notes/a.md", BasePathRevision: 1, Kind: WorkspaceMutationUpsertFile,
		ContentHash: workspaceHashValue(), Metadata: WorkspaceFileMetadata{Size: 3, ModifiedAtMS: 1},
	}
	require.NoError(t, base.Validate())
	raw, err := json.Marshal(base)
	require.NoError(t, err)
	require.Contains(t, string(raw), `"contentHash":"blake3:`)

	missingHash := base
	missingHash.ContentHash = workspaceNullHash()
	requireWorkspaceValidationError(t, missingHash.Validate(), "contentHash", "required_for_kind")

	mkdir := base
	mkdir.Kind = WorkspaceMutationMkdir
	mkdir.ContentHash = workspaceNullHash()
	mkdir.Metadata = WorkspaceFileMetadata{}
	require.NoError(t, mkdir.Validate())
	mkdir.ContentHash = workspaceHashValue()
	requireWorkspaceValidationError(t, mkdir.Validate(), "contentHash", "must_be_null_for_kind")

	rename := base
	rename.Kind = WorkspaceMutationRename
	newPath := WorkspacePath("archive/a.md")
	targetRevision := WorkspaceRevision(0)
	rename.NewPath = &newPath
	rename.TargetBasePathRevision = &targetRevision
	require.NoError(t, rename.Validate())
	rename.NewPath = nil
	requireWorkspaceValidationError(t, rename.Validate(), "newPath", "required_for_rename")
	rename.NewPath = &newPath
	child := WorkspacePath("notes/a.md/child")
	rename.NewPath = &child
	requireWorkspaceValidationError(t, rename.Validate(), "newPath", "directory_into_child")

	nonRename := base
	nonRename.NewPath = &newPath
	nonRename.TargetBasePathRevision = &targetRevision
	requireWorkspaceValidationError(t, nonRename.Validate(), "newPath", "forbidden_for_kind")
}

func TestWorkspaceV2MutationResultsAndEvent(t *testing.T) {
	t.Parallel()
	state := WorkspacePathState{
		Path: "notes/a.md", PathRevision: 2, Kind: WorkspaceEntryFile,
		ContentHash: workspaceHashValue(), Metadata: WorkspaceFileMetadata{Size: 3, ModifiedAtMS: 1},
	}
	accepted := WorkspaceMutationAcceptedMessage{
		WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1), OperationID: workspaceTestUUID(4),
		Revision: 2, PathState: state,
	}
	require.NoError(t, accepted.Validate())

	rejected := WorkspaceMutationRejectedMessage{
		WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1), OperationID: workspaceTestUUID(4),
		Reason: WorkspaceMutationRejectOperationReused,
	}
	require.NoError(t, rejected.Validate())
	conflictID := workspaceTestUUID(5)
	rejected.Reason = WorkspaceMutationRejectConflictCreated
	rejected.ConflictID = &conflictID
	require.NoError(t, rejected.Validate())
	rejected.RequiredHash = func() *WorkspaceContentHash { h := workspaceTestHash(); return &h }()
	requireWorkspaceValidationError(t, rejected.Validate(), "requiredHash", "forbidden_for_reason")

	mutation := WorkspaceMutation{
		WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1), OperationID: workspaceTestUUID(4),
		Path: "notes/a.md", BasePathRevision: 1, Kind: WorkspaceMutationUpsertFile,
		ContentHash: workspaceHashValue(), Metadata: WorkspaceFileMetadata{Size: 3, ModifiedAtMS: 1},
	}
	event := WorkspaceEventMessage{
		WorkspaceID: workspaceTestUUID(2), StreamID: workspaceTestUUID(3), Index: 1, Revision: 2,
		OperationID: workspaceTestUUID(4), OriginClientID: workspaceTestUUID(1), Mutation: mutation, PathState: state,
	}
	require.NoError(t, event.Validate(0, 1))
	requireWorkspaceValidationError(t, event.Validate(1, 1), "index", "stream_gap")
	requireWorkspaceValidationError(t, event.Validate(0, 2), "revision", "not_strictly_increasing")
}

func TestWorkspaceV2MutationAcceptedRenameStateIntegrity(t *testing.T) {
	t.Parallel()
	oldState := WorkspacePathState{
		Path: "notes/a.md", PathRevision: 2, Kind: WorkspaceEntryTombstone,
		ContentHash: workspaceNullHash(), Metadata: WorkspaceFileMetadata{}, Tombstone: true,
	}
	newState := WorkspacePathState{
		Path: "archive/a.md", PathRevision: 2, Kind: WorkspaceEntryFile,
		ContentHash: workspaceHashValue(), Metadata: WorkspaceFileMetadata{Size: 3, ModifiedAtMS: 1},
	}
	primaryState := newState
	equalHash := *newState.ContentHash.Value
	primaryState.ContentHash.Value = &equalHash
	valid := WorkspaceMutationAcceptedMessage{
		WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1), OperationID: workspaceTestUUID(4),
		Revision: 2, PathState: primaryState, OldPathState: &oldState, NewPathState: &newState,
	}
	require.NotSame(t, valid.PathState.ContentHash.Value, valid.NewPathState.ContentHash.Value)
	require.NoError(t, valid.Validate())

	for _, tt := range []struct {
		name   string
		mutate func(*WorkspaceMutationAcceptedMessage)
		field  string
		reason string
	}{
		{name: "old only", mutate: func(m *WorkspaceMutationAcceptedMessage) { m.NewPathState = nil }, field: "pathState", reason: "rename_pair_required"},
		{name: "new only", mutate: func(m *WorkspaceMutationAcceptedMessage) { m.OldPathState = nil }, field: "pathState", reason: "rename_pair_required"},
		{name: "same paths", mutate: func(m *WorkspaceMutationAcceptedMessage) { m.OldPathState.Path = m.NewPathState.Path }, field: "oldPathState.path", reason: "rename_path_required"},
		{name: "old revision", mutate: func(m *WorkspaceMutationAcceptedMessage) { m.OldPathState.PathRevision = 1 }, field: "oldPathState.pathRevision", reason: "revision_mismatch"},
		{name: "new revision", mutate: func(m *WorkspaceMutationAcceptedMessage) { m.NewPathState.PathRevision = 1 }, field: "newPathState.pathRevision", reason: "revision_mismatch"},
		{name: "primary differs", mutate: func(m *WorkspaceMutationAcceptedMessage) { m.PathState.Metadata.Size = 4 }, field: "pathState", reason: "new_path_state_mismatch"},
		{name: "invalid old", mutate: func(m *WorkspaceMutationAcceptedMessage) { m.OldPathState.Path = "" }, field: "path", reason: "invalid_length_or_utf8"},
		{name: "invalid new", mutate: func(m *WorkspaceMutationAcceptedMessage) { m.NewPathState.Path = "" }, field: "path", reason: "invalid_length_or_utf8"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			message := valid
			oldCopy, newCopy := *valid.OldPathState, *valid.NewPathState
			message.OldPathState, message.NewPathState = &oldCopy, &newCopy
			tt.mutate(&message)
			requireWorkspaceValidationError(t, message.Validate(), tt.field, tt.reason)
		})
	}
}

func TestWorkspaceV2EventRenameStateRules(t *testing.T) {
	t.Parallel()
	newPath := WorkspacePath("archive/a.md")
	targetRevision := WorkspaceRevision(0)
	mutation := WorkspaceMutation{
		WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1), OperationID: workspaceTestUUID(4),
		Path: "notes/a.md", BasePathRevision: 1, Kind: WorkspaceMutationRename,
		ContentHash: workspaceHashValue(), Metadata: WorkspaceFileMetadata{Size: 3, ModifiedAtMS: 1},
		NewPath: &newPath, TargetBasePathRevision: &targetRevision,
	}
	oldState := WorkspacePathState{
		Path: "notes/a.md", PathRevision: 2, Kind: WorkspaceEntryTombstone,
		ContentHash: workspaceNullHash(), Metadata: WorkspaceFileMetadata{}, Tombstone: true,
	}
	newState := WorkspacePathState{
		Path: newPath, PathRevision: 2, Kind: WorkspaceEntryFile,
		ContentHash: workspaceHashValue(), Metadata: WorkspaceFileMetadata{Size: 3, ModifiedAtMS: 1},
	}
	valid := WorkspaceEventMessage{
		WorkspaceID: workspaceTestUUID(2), StreamID: workspaceTestUUID(3), Index: 1, Revision: 2,
		OperationID: workspaceTestUUID(4), OriginClientID: workspaceTestUUID(1), Mutation: mutation,
		PathState: newState, OldPathState: &oldState, NewPathState: &newState,
	}
	equalHash := *newState.ContentHash.Value
	valid.PathState.ContentHash.Value = &equalHash
	require.NotSame(t, valid.PathState.ContentHash.Value, valid.NewPathState.ContentHash.Value)
	require.NoError(t, valid.Validate(0, 1))

	for _, tt := range []struct {
		name string
		old  *WorkspacePathState
		new  *WorkspacePathState
	}{
		{name: "both absent"},
		{name: "old absent", new: &newState},
		{name: "new absent", old: &oldState},
	} {
		t.Run(tt.name, func(t *testing.T) {
			event := valid
			event.OldPathState = tt.old
			event.NewPathState = tt.new
			requireWorkspaceValidationError(t, event.Validate(0, 1), "pathState", "rename_pair_required")
		})
	}

	t.Run("pair forbidden for non-rename", func(t *testing.T) {
		event := valid
		event.Mutation.Kind = WorkspaceMutationUpsertFile
		event.Mutation.NewPath = nil
		event.Mutation.TargetBasePathRevision = nil
		requireWorkspaceValidationError(t, event.Validate(0, 1), "pathState", "forbidden_for_kind")
	})

	t.Run("invalid old state", func(t *testing.T) {
		event := valid
		invalidOld := oldState
		invalidOld.Path = ""
		event.OldPathState = &invalidOld
		requireWorkspaceValidationError(t, event.Validate(0, 1), "path", "invalid_length_or_utf8")
	})

	t.Run("invalid new state", func(t *testing.T) {
		event := valid
		invalidNew := newState
		invalidNew.Kind = WorkspaceEntryTombstone
		event.NewPathState = &invalidNew
		requireWorkspaceValidationError(t, event.Validate(0, 1), "tombstone", "kind_mismatch")
	})

	for _, tt := range []struct {
		name   string
		mutate func(*WorkspaceEventMessage)
		field  string
		reason string
	}{
		{name: "old path differs from mutation", mutate: func(m *WorkspaceEventMessage) { m.OldPathState.Path = "notes/b.md" }, field: "oldPathState.path", reason: "mutation_path_mismatch"},
		{name: "new path differs from mutation", mutate: func(m *WorkspaceEventMessage) { m.NewPathState.Path = "archive/b.md" }, field: "newPathState.path", reason: "mutation_new_path_mismatch"},
		{name: "old revision", mutate: func(m *WorkspaceEventMessage) { m.OldPathState.PathRevision = 1 }, field: "oldPathState.pathRevision", reason: "revision_mismatch"},
		{name: "new revision", mutate: func(m *WorkspaceEventMessage) { m.NewPathState.PathRevision = 1 }, field: "newPathState.pathRevision", reason: "revision_mismatch"},
		{name: "primary differs", mutate: func(m *WorkspaceEventMessage) { m.PathState.Metadata.Size = 4 }, field: "pathState", reason: "new_path_state_mismatch"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			event := valid
			oldCopy, newCopy := *valid.OldPathState, *valid.NewPathState
			event.OldPathState, event.NewPathState = &oldCopy, &newCopy
			tt.mutate(&event)
			requireWorkspaceValidationError(t, event.Validate(0, 1), tt.field, tt.reason)
		})
	}
}

func TestWorkspaceV2MutationRejectedReasonWireType(t *testing.T) {
	t.Parallel()
	field, ok := reflect.TypeOf(WorkspaceMutationRejectedMessage{}).FieldByName("Reason")
	require.True(t, ok)
	require.Equal(t, reflect.TypeOf(""), field.Type)
}

func TestWorkspaceV2AckValidation(t *testing.T) {
	t.Parallel()
	ack := WorkspaceAckRequest{WorkspaceID: workspaceTestUUID(2), ClientID: workspaceTestUUID(1), Revision: 3}
	require.NoError(t, ack.Validate(2, 3))
	requireWorkspaceValidationError(t, ack.Validate(3, 3), "revision", "ack_regression")
	requireWorkspaceValidationError(t, ack.Validate(2, 2), "revision", "ack_overshoot")
}

func TestWorkspaceV2BlobMessages(t *testing.T) {
	t.Parallel()
	upload := WorkspaceBlobNeedUploadPush{
		WorkspaceID: workspaceTestUUID(2), Direction: WorkspaceBlobUpload,
		OperationID: workspaceTestUUID(4), ContentHash: workspaceTestHash(), Size: 7,
	}
	require.NoError(t, upload.Validate())
	upload.Direction = WorkspaceBlobDownload
	requireWorkspaceValidationError(t, upload.Validate(), "direction", "must_be_upload")

	downloadRequest := WorkspaceBlobNeedDownloadRequest{
		WorkspaceID: workspaceTestUUID(2), Direction: WorkspaceBlobDownload,
		OperationID: WorkspaceNullableUUID{Present: true}, ContentHash: workspaceTestHash(),
		Size: WorkspaceNullableUint64{Present: true},
	}
	require.NoError(t, downloadRequest.Validate())

	downloadResponse := WorkspaceBlobNeedDownloadResponse{
		WorkspaceID: workspaceTestUUID(2), Direction: WorkspaceBlobDownload,
		OperationID: WorkspaceNullableUUID{Present: true}, ContentHash: workspaceTestHash(), Size: 0,
	}
	require.NoError(t, downloadResponse.Validate())

	begin := WorkspaceBlobBeginMessage{
		WorkspaceID: workspaceTestUUID(2), TransferID: workspaceTestUUID(5), Direction: WorkspaceBlobUpload,
		ContentHash: workspaceTestHash(), Size: WorkspaceBlobChunkSize + 7,
		ChunkSize: WorkspaceBlobChunkSize, ChunkCount: 2,
	}
	require.NoError(t, begin.Validate())
	begin.ChunkCount = 1
	requireWorkspaceValidationError(t, begin.Validate(), "chunkCount", "arithmetic_mismatch")
	begin.Size = 0
	begin.ChunkCount = 0
	require.NoError(t, begin.Validate())
	begin.Size = WorkspaceMaxBlobBytes + 1
	requireWorkspaceValidationError(t, begin.Validate(), "size", "limit_exceeded")

	end := WorkspaceBlobEndMessage{
		WorkspaceID: workspaceTestUUID(2), TransferID: workspaceTestUUID(5), Direction: WorkspaceBlobDownload,
		ContentHash: workspaceTestHash(),
		Size:        0, ChunkCount: 0,
	}
	require.NoError(t, end.Validate())
	end.ChunkCount = 1
	requireWorkspaceValidationError(t, end.Validate(), "chunkCount", "arithmetic_mismatch")
}

func TestWorkspaceBlobNeedDownloadRequiresExplicitNull(t *testing.T) {
	t.Parallel()
	base := `{"workspaceId":"10000000-0000-4000-8000-000000000001","direction":"download","contentHash":"blake3:` + strings.Repeat("0", 64) + `"}`
	var missing WorkspaceBlobNeedDownloadRequest
	require.NoError(t, json.Unmarshal([]byte(base), &missing))
	requireWorkspaceValidationError(t, missing.Validate(), "operationId", "required_key_missing")

	explicit := strings.TrimSuffix(base, "}") + `,"operationId":null,"size":null}`
	var valid WorkspaceBlobNeedDownloadRequest
	require.NoError(t, json.Unmarshal([]byte(explicit), &valid))
	require.NoError(t, valid.Validate())
	require.True(t, valid.OperationID.Present)
	require.Nil(t, valid.OperationID.Value)
	require.True(t, valid.Size.Present)
	require.Nil(t, valid.Size.Value)

	nonNull := strings.TrimSuffix(base, "}") + `,"operationId":"10000000-0000-4000-8000-000000000002","size":1}`
	var invalid WorkspaceBlobNeedDownloadRequest
	require.NoError(t, json.Unmarshal([]byte(nonNull), &invalid))
	requireWorkspaceValidationError(t, invalid.Validate(), "operationId", "must_be_null")

	responseJSON := strings.TrimSuffix(base, "}") + `,"operationId":null,"size":0}`
	var response WorkspaceBlobNeedDownloadResponse
	require.NoError(t, json.Unmarshal([]byte(responseJSON), &response))
	require.NoError(t, response.Validate())
}

func TestWorkspaceBlobDigest(t *testing.T) {
	t.Parallel()
	for _, payload := range [][]byte{[]byte("workspace-sync-v2"), nil} {
		want := blake3.Sum256(payload)
		full, first16 := ComputeWorkspaceBlobDigest(payload)
		require.Equal(t, want, full)
		require.Equal(t, want[:16], first16[:])
	}
}

func TestWorkspaceBlobHeaderLayout(t *testing.T) {
	t.Parallel()
	id := uuid.MustParse("10000000-0000-4000-8000-000000000001")
	digest := [16]byte{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15}
	raw, err := MarshalWorkspaceBlobHeader(WorkspaceBlobHeader{
		Direction: WorkspaceBlobUpload, Final: true, TransferID: id,
		ChunkIndex: 2, Offset: 2 * WorkspaceBlobChunkSize, PayloadLen: 7,
		ChunkDigest: digest,
	})
	require.NoError(t, err)
	require.Equal(t, []byte("FNS2"), raw[0:4])
	require.Equal(t, byte(2), raw[4])
	require.Equal(t, byte(1), raw[5])
	require.Equal(t, byte(1), raw[6])
	require.Equal(t, byte(64), raw[7])
	require.Equal(t, uint64(2), binary.BigEndian.Uint64(raw[24:32]))
	require.Equal(t, uint64(2*WorkspaceBlobChunkSize), binary.BigEndian.Uint64(raw[32:40]))
	require.Equal(t, uint32(7), binary.BigEndian.Uint32(raw[40:44]))
	require.Equal(t, []byte{0, 0, 0, 0}, raw[44:48])
	require.Equal(t, digest[:], raw[48:64])

	decoded, err := UnmarshalWorkspaceBlobHeader(raw[:], 7, digest)
	require.NoError(t, err)
	require.Equal(t, WorkspaceBlobUpload, decoded.Direction)
	require.Equal(t, id, decoded.TransferID)
	require.NoError(t, decoded.ValidateSequence(2, 2*WorkspaceBlobChunkSize, true))
	requireWorkspaceValidationError(t, decoded.ValidateSequence(1, 2*WorkspaceBlobChunkSize, true), "chunkIndex", "out_of_order")
	requireWorkspaceValidationError(t, decoded.ValidateSequence(2, 0, true), "offset", "out_of_order")
	requireWorkspaceValidationError(t, decoded.ValidateSequence(2, 2*WorkspaceBlobChunkSize, false), "final", "flag_mismatch")
}

func TestWorkspaceBlobHeaderRejectsInvalidFields(t *testing.T) {
	t.Parallel()
	digest := [16]byte{1}
	raw, err := MarshalWorkspaceBlobHeader(WorkspaceBlobHeader{
		Direction: WorkspaceBlobDownload, Final: false,
		TransferID: uuid.MustParse("10000000-0000-4000-8000-000000000001"),
		PayloadLen: WorkspaceBlobChunkSize, ChunkDigest: digest,
	})
	require.NoError(t, err)

	tests := []struct {
		name   string
		mutate func([]byte) []byte
		actual uint32
		digest [16]byte
		field  string
		reason string
	}{
		{name: "truncated", mutate: func(b []byte) []byte { return b[:63] }, actual: WorkspaceBlobChunkSize, digest: digest, field: "header", reason: "invalid_length"},
		{name: "oversized", mutate: func(b []byte) []byte { return append(b, 0) }, actual: WorkspaceBlobChunkSize, digest: digest, field: "header", reason: "invalid_length"},
		{name: "magic", mutate: func(b []byte) []byte { b[0] = 'X'; return b }, actual: WorkspaceBlobChunkSize, digest: digest, field: "magic", reason: "invalid"},
		{name: "version", mutate: func(b []byte) []byte { b[4] = 1; return b }, actual: WorkspaceBlobChunkSize, digest: digest, field: "version", reason: "invalid"},
		{name: "flags", mutate: func(b []byte) []byte { b[6] = 2; return b }, actual: WorkspaceBlobChunkSize, digest: digest, field: "flags", reason: "reserved_bits"},
		{name: "header length", mutate: func(b []byte) []byte { b[7] = 63; return b }, actual: WorkspaceBlobChunkSize, digest: digest, field: "headerLength", reason: "invalid"},
		{name: "reserved", mutate: func(b []byte) []byte { b[44] = 1; return b }, actual: WorkspaceBlobChunkSize, digest: digest, field: "reserved", reason: "non_zero"},
		{name: "payload length", mutate: func(b []byte) []byte { return b }, actual: 3, digest: digest, field: "payloadLength", reason: "frame_mismatch"},
		{name: "digest", mutate: func(b []byte) []byte { return b }, actual: WorkspaceBlobChunkSize, digest: [16]byte{2}, field: "chunkDigest", reason: "mismatch"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			copyRaw := append([]byte(nil), raw[:]...)
			_, err := UnmarshalWorkspaceBlobHeader(tt.mutate(copyRaw), tt.actual, tt.digest)
			requireWorkspaceValidationError(t, err, tt.field, tt.reason)
		})
	}
}

func TestWorkspaceBlobHeaderRejectsZeroPayload(t *testing.T) {
	t.Parallel()
	id := uuid.MustParse("10000000-0000-4000-8000-000000000001")
	emptyDigest := blake3.Sum256(nil)
	var first16 [16]byte
	copy(first16[:], emptyDigest[:16])

	_, err := MarshalWorkspaceBlobHeader(WorkspaceBlobHeader{
		Direction: WorkspaceBlobDownload, Final: true, TransferID: id, ChunkDigest: first16,
	})
	requireWorkspaceValidationError(t, err, "payloadLength", "empty_payload_forbidden")

	raw := make([]byte, WorkspaceBlobHeaderSize)
	copy(raw[0:4], "FNS2")
	raw[4], raw[5], raw[6], raw[7] = 2, 2, 1, WorkspaceBlobHeaderSize
	copy(raw[8:24], id[:])
	copy(raw[48:64], first16[:])
	_, err = UnmarshalWorkspaceBlobHeader(raw, 0, first16)
	requireWorkspaceValidationError(t, err, "payloadLength", "empty_payload_forbidden")
	requireWorkspaceValidationError(t, (WorkspaceBlobHeader{Final: true, PayloadLen: 0}).ValidateSequence(0, 0, true), "payloadLength", "empty_payload_forbidden")
}

func workspaceConflictLiveSide(path WorkspacePath, revision WorkspaceRevision) WorkspaceConflictSide {
	return WorkspaceConflictSide{
		Path: &path, PathRevision: revision, ContentHash: workspaceHashValue(),
		Metadata: WorkspaceFileMetadata{Size: 3, ModifiedAtMS: 1},
	}
}

func workspaceConflictTombstoneSide(revision WorkspaceRevision) WorkspaceConflictSide {
	return WorkspaceConflictSide{
		PathRevision: revision, ContentHash: workspaceNullHash(), Metadata: WorkspaceFileMetadata{}, Tombstone: true,
	}
}

func workspaceConflictCreated(t *testing.T, kind WorkspaceConflictKind) WorkspaceConflictCreatedMessage {
	t.Helper()
	path := WorkspacePath("notes/a.md")
	created := WorkspaceConflictCreatedMessage{
		WorkspaceID: workspaceTestUUID(2), ConflictID: workspaceTestUUID(5),
		ConflictRevision: workspaceTestConflictRevision(t, "42"),
		Path:             path, Kind: kind, Ancestor: workspaceConflictLiveSide(path, 3),
		Current: workspaceConflictLiveSide(path, 6), Incoming: workspaceConflictLiveSide(path, 5),
		CreatedByOperationID: workspaceTestUUID(4),
	}
	switch kind {
	case WorkspaceConflictDeleteModify:
		created.Incoming = workspaceConflictTombstoneSide(5)
	case WorkspaceConflictRename:
		incomingPath := WorkspacePath("archive/a.md")
		created.Incoming = workspaceConflictLiveSide(incomingPath, 5)
	}
	return created
}

func TestWorkspaceV2ConflictRevisionValidation(t *testing.T) {
	t.Parallel()
	created := workspaceConflictCreated(t, WorkspaceConflictContent)
	created.ConflictRevision = WorkspaceConflictRevision{}
	requireWorkspaceValidationError(t, created.Validate(), "conflictRevision", "must_be_positive")

	created = workspaceConflictCreated(t, WorkspaceConflictContent)
	request := WorkspaceConflictResolvedRequest{
		WorkspaceID: created.WorkspaceID, ClientID: workspaceTestUUID(1), OperationID: workspaceTestUUID(4),
		ConflictID: created.ConflictID, ConflictRevision: WorkspaceConflictRevision{},
		Choice: WorkspaceConflictKeepCurrent, Path: *created.Current.Path,
		ContentHash: created.Current.ContentHash, Metadata: created.Current.Metadata,
	}
	requireWorkspaceValidationError(t, request.ValidateAgainst(created), "conflictRevision", "must_be_positive")

	resolved := WorkspaceConflictResolvedMessage{
		WorkspaceID: workspaceTestUUID(2), ConflictID: workspaceTestUUID(5),
		ConflictRevision: WorkspaceConflictRevision{}, OperationID: workspaceTestUUID(4),
		Revision: 8, Choice: WorkspaceConflictUseMerged,
		PathState: WorkspacePathState{
			Path: "notes/a.md", PathRevision: 8, Kind: WorkspaceEntryFile,
			ContentHash: workspaceHashValue(), Metadata: WorkspaceFileMetadata{Size: 8, ModifiedAtMS: 2},
		},
		ResolvedByClientID: workspaceTestUUID(1),
	}
	requireWorkspaceValidationError(t, resolved.Validate(), "conflictRevision", "must_be_positive")
}

func TestWorkspaceV2ConflictCreatedKinds(t *testing.T) {
	t.Parallel()
	for _, kind := range []WorkspaceConflictKind{
		WorkspaceConflictContent, WorkspaceConflictBinary, WorkspaceConflictDeleteModify, WorkspaceConflictRename,
	} {
		created := workspaceConflictCreated(t, kind)
		require.NoError(t, created.Validate(), kind)
		raw, err := json.Marshal(created)
		require.NoError(t, err)
		var roundTrip WorkspaceConflictCreatedMessage
		require.NoError(t, strictJSONDecode(raw, &roundTrip))
		require.Equal(t, created, roundTrip)
	}

	invalid := workspaceConflictCreated(t, WorkspaceConflictContent)
	invalid.Current = workspaceConflictTombstoneSide(6)
	requireWorkspaceValidationError(t, invalid.Validate(), "current", "kind_mismatch")
	invalid = workspaceConflictCreated(t, WorkspaceConflictRename)
	invalid.Incoming = invalid.Current
	requireWorkspaceValidationError(t, invalid.Validate(), "incoming.path", "rename_path_required")
}

func TestWorkspaceV2ConflictResolutionChoices(t *testing.T) {
	t.Parallel()
	created := workspaceConflictCreated(t, WorkspaceConflictContent)
	request := WorkspaceConflictResolvedRequest{
		WorkspaceID: created.WorkspaceID, ClientID: workspaceTestUUID(1), OperationID: workspaceTestUUID(4),
		ConflictID: created.ConflictID, ConflictRevision: created.ConflictRevision,
		Choice: WorkspaceConflictKeepCurrent, Path: *created.Current.Path,
		ContentHash: created.Current.ContentHash, Metadata: created.Current.Metadata,
	}
	require.NoError(t, request.ValidateAgainst(created))

	request.Choice = WorkspaceConflictUseIncoming
	request.Path = *created.Incoming.Path
	request.ContentHash = created.Incoming.ContentHash
	request.Metadata = created.Incoming.Metadata
	require.NoError(t, request.ValidateAgainst(created))

	request.Choice = WorkspaceConflictUseMerged
	request.Path = created.Path
	request.ContentHash = workspaceHashValue()
	request.Metadata = WorkspaceFileMetadata{Size: 9, ModifiedAtMS: 2}
	require.NoError(t, request.ValidateAgainst(created))
	request.ContentHash = workspaceNullHash()
	requireWorkspaceValidationError(t, request.ValidateAgainst(created), "contentHash", "required_for_merged")

	request.Choice = WorkspaceConflictDelete
	request.Path = created.Path
	request.ContentHash = workspaceNullHash()
	request.Metadata = WorkspaceFileMetadata{}
	require.NoError(t, request.ValidateAgainst(created))
	request.Metadata.ModifiedAtMS = 1
	requireWorkspaceValidationError(t, request.ValidateAgainst(created), "metadata.modifiedAtMs", "must_be_zero_for_delete")
	request.Metadata.ModifiedAtMS = 0
	request.ContentHash = workspaceHashValue()
	requireWorkspaceValidationError(t, request.ValidateAgainst(created), "contentHash", "must_be_null_for_delete")

	request.ConflictRevision = workspaceTestConflictRevision(t, "41")
	request.ContentHash = WorkspaceNullableHash{}
	requireWorkspaceValidationError(t, request.ValidateAgainst(created), "conflictRevision", "conflict_revision_stale")
}

func TestWorkspaceV2ConflictMergedPayload(t *testing.T) {
	t.Parallel()
	created := workspaceConflictCreated(t, WorkspaceConflictBinary)
	resolve := WorkspaceConflictResolvedRequest{
		WorkspaceID: created.WorkspaceID, ClientID: workspaceTestUUID(1), OperationID: workspaceTestUUID(4),
		ConflictID: created.ConflictID, ConflictRevision: created.ConflictRevision,
		Choice: WorkspaceConflictUseMerged, Path: created.Path, ContentHash: workspaceHashValue(),
		Metadata: WorkspaceFileMetadata{Size: 8, ModifiedAtMS: 2},
	}
	require.NoError(t, resolve.ValidateAgainst(created))
	firstID := workspaceTestUUID(6)
	secondID := workspaceTestUUID(7)
	first, err := json.Marshal(WorkspaceV2Request[WorkspaceConflictResolvedRequest]{RequestID: firstID, Data: resolve})
	require.NoError(t, err)
	second, err := json.Marshal(WorkspaceV2Request[WorkspaceConflictResolvedRequest]{RequestID: secondID, Data: resolve})
	require.NoError(t, err)
	require.NotEqual(t, string(first), string(second))
	var firstEnvelope, secondEnvelope WorkspaceV2Request[WorkspaceConflictResolvedRequest]
	require.NoError(t, strictJSONDecode(first, &firstEnvelope))
	require.NoError(t, strictJSONDecode(second, &secondEnvelope))
	require.Equal(t, firstEnvelope.Data, secondEnvelope.Data)
	require.Equal(t, resolve.OperationID, secondEnvelope.Data.OperationID)
}

func TestWorkspaceV2ConflictResolvedRoundTrip(t *testing.T) {
	t.Parallel()
	message := WorkspaceConflictResolvedMessage{
		WorkspaceID: workspaceTestUUID(2), ConflictID: workspaceTestUUID(5),
		ConflictRevision: workspaceTestConflictRevision(t, "42"),
		OperationID:      workspaceTestUUID(4), Revision: 8, Choice: WorkspaceConflictUseMerged,
		PathState: WorkspacePathState{
			Path: "notes/a.md", PathRevision: 8, Kind: WorkspaceEntryFile,
			ContentHash: workspaceHashValue(), Metadata: WorkspaceFileMetadata{Size: 8, ModifiedAtMS: 2},
		},
		ResolvedByClientID: workspaceTestUUID(1),
	}
	require.NoError(t, message.Validate())
	raw, err := json.Marshal(message)
	require.NoError(t, err)
	var roundTrip WorkspaceConflictResolvedMessage
	require.NoError(t, strictJSONDecode(raw, &roundTrip))
	require.Equal(t, message, roundTrip)
}

func TestWorkspaceV2ActionRegistryFlows(t *testing.T) {
	t.Parallel()
	tests := []struct {
		action WorkspaceV2Action
		flow   WorkspaceV2Flow
		want   any
	}{
		{WorkspaceActionHello, WorkspaceFlowClientRequest, (*WorkspaceHelloRequest)(nil)},
		{WorkspaceActionHello, WorkspaceFlowServerResponse, (*WorkspaceHelloResponse)(nil)},
		{WorkspaceActionSubscribe, WorkspaceFlowClientRequest, (*WorkspaceSubscribeRequest)(nil)},
		{WorkspaceActionSnapshotBegin, WorkspaceFlowServerPush, (*WorkspaceSnapshotBeginMessage)(nil)},
		{WorkspaceActionSnapshotEntry, WorkspaceFlowServerPush, (*WorkspaceSnapshotEntryMessage)(nil)},
		{WorkspaceActionSnapshotEnd, WorkspaceFlowServerPush, (*WorkspaceSnapshotEndMessage)(nil)},
		{WorkspaceActionMutation, WorkspaceFlowClientRequest, (*WorkspaceMutation)(nil)},
		{WorkspaceActionMutationAccepted, WorkspaceFlowServerResponse, (*WorkspaceMutationAcceptedMessage)(nil)},
		{WorkspaceActionMutationRejected, WorkspaceFlowServerResponse, (*WorkspaceMutationRejectedMessage)(nil)},
		{WorkspaceActionEvent, WorkspaceFlowServerPush, (*WorkspaceEventMessage)(nil)},
		{WorkspaceActionAck, WorkspaceFlowClientRequest, (*WorkspaceAckRequest)(nil)},
		{WorkspaceActionAck, WorkspaceFlowServerResponse, (*WorkspaceAckRequest)(nil)},
		{WorkspaceActionBlobNeed, WorkspaceFlowClientRequest, (*WorkspaceBlobNeedDownloadRequest)(nil)},
		{WorkspaceActionBlobNeed, WorkspaceFlowServerResponse, (*WorkspaceBlobNeedDownloadResponse)(nil)},
		{WorkspaceActionBlobNeed, WorkspaceFlowServerPush, (*WorkspaceBlobNeedUploadPush)(nil)},
		{WorkspaceActionBlobBegin, WorkspaceFlowClientRequest, (*WorkspaceBlobBeginMessage)(nil)},
		{WorkspaceActionBlobBegin, WorkspaceFlowServerResponse, (*WorkspaceBlobBeginMessage)(nil)},
		{WorkspaceActionBlobBegin, WorkspaceFlowServerPush, (*WorkspaceBlobBeginMessage)(nil)},
		{WorkspaceActionBlobEnd, WorkspaceFlowClientRequest, (*WorkspaceBlobEndMessage)(nil)},
		{WorkspaceActionBlobEnd, WorkspaceFlowServerResponse, (*WorkspaceBlobEndMessage)(nil)},
		{WorkspaceActionBlobEnd, WorkspaceFlowServerPush, (*WorkspaceBlobEndMessage)(nil)},
		{WorkspaceActionConflictCreated, WorkspaceFlowServerPush, (*WorkspaceConflictCreatedMessage)(nil)},
		{WorkspaceActionConflictResolved, WorkspaceFlowClientRequest, (*WorkspaceConflictResolvedRequest)(nil)},
		{WorkspaceActionConflictResolved, WorkspaceFlowServerResponse, (*WorkspaceConflictResolvedMessage)(nil)},
		{WorkspaceActionConflictResolved, WorkspaceFlowServerPush, (*WorkspaceConflictResolvedMessage)(nil)},
	}
	require.Len(t, tests, 25)
	for _, tt := range tests {
		got, err := NewWorkspaceV2Data(tt.action, tt.flow)
		require.NoError(t, err, "%s/%s", tt.action, tt.flow)
		require.IsType(t, tt.want, got, "%s/%s", tt.action, tt.flow)
	}

	require.Len(t, WorkspaceV2ActionSpecs, len(WorkspaceV2Actions))
	for _, action := range WorkspaceV2Actions {
		_, ok := WorkspaceV2ActionSpecs[action]
		require.Truef(t, ok, "registry missing %s", action)
	}
	_, err := NewWorkspaceV2Data(WorkspaceActionSubscribe, WorkspaceFlowServerPush)
	requireWorkspaceValidationError(t, err, "flow", "flow_not_allowed")
	_, err = NewWorkspaceV2Data(WorkspaceV2Action("WorkspaceExtra"), WorkspaceFlowClientRequest)
	requireWorkspaceValidationError(t, err, "action", "unknown_action")
}
