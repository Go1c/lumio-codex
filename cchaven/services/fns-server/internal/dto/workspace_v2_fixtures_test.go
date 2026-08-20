package dto

import (
	"bufio"
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"testing"

	"github.com/stretchr/testify/require"
	"github.com/zeebo/blake3"
)

type workspaceFixtureManifest struct {
	SchemaVersion string              `json:"schemaVersion"`
	Actions       []WorkspaceV2Action `json:"actions"`
	Files         map[string]string   `json:"files"`
}

type workspaceControlFixtureRow struct {
	Case     string            `json:"case"`
	Sequence string            `json:"sequence,omitempty"`
	Step     uint32            `json:"step,omitempty"`
	Action   WorkspaceV2Action `json:"action"`
	Flow     WorkspaceV2Flow   `json:"flow"`
	Frame    string            `json:"frame"`
}

type workspaceErrorFixtureRow struct {
	Case   string            `json:"case"`
	Action WorkspaceV2Action `json:"action"`
	Frame  string            `json:"frame"`
}

type workspaceInvalidFixtureRow struct {
	Case   string          `json:"case"`
	Value  json.RawMessage `json:"value"`
	Field  string          `json:"field"`
	Reason string          `json:"reason"`
}

type workspaceBinaryHeaderVector struct {
	Case       string                 `json:"case"`
	Direction  WorkspaceBlobDirection `json:"direction"`
	Final      bool                   `json:"final"`
	TransferID WorkspaceUUID          `json:"transferId"`
	ChunkIndex uint64                 `json:"chunkIndex"`
	Offset     uint64                 `json:"offset"`
	PayloadHex string                 `json:"payloadHex"`
	DigestHex  string                 `json:"digestHex"`
	HeaderHex  string                 `json:"headerHex"`
	Valid      bool                   `json:"valid"`
	Reason     string                 `json:"reason,omitempty"`
}

type workspaceBinaryHeaderVectors struct {
	Rows []workspaceBinaryHeaderVector `json:"rows"`
}

func workspaceFixtureRoot() string {
	return filepath.Join("..", "..", "testdata", "workspace-sync-v2")
}

func TestWorkspaceV2FixtureManifest(t *testing.T) {
	t.Parallel()
	raw, err := os.ReadFile(filepath.Join(workspaceFixtureRoot(), "manifest.json"))
	require.NoError(t, err)
	var manifest workspaceFixtureManifest
	require.NoError(t, strictJSONDecode(raw, &manifest))
	require.Equal(t, "workspace-sync-v2-fixtures/1", manifest.SchemaVersion)
	require.Equal(t, WorkspaceV2Actions, manifest.Actions)
	require.Len(t, manifest.Actions, 15)

	wantFiles := []string{
		"valid/control-frames.jsonl", "valid/error-envelopes.jsonl",
		"invalid/revisions.jsonl", "invalid/hashes.jsonl", "invalid/paths.jsonl",
		"binary/header-vectors.json",
	}
	require.Len(t, manifest.Files, len(wantFiles))
	for _, name := range wantFiles {
		digest, ok := manifest.Files[name]
		require.Truef(t, ok, "manifest missing %s", name)
		require.Regexp(t, `^[0-9a-f]{64}$`, digest)
		contents, err := os.ReadFile(filepath.Join(workspaceFixtureRoot(), filepath.FromSlash(name)))
		require.NoError(t, err)
		actual := sha256.Sum256(contents)
		require.Equal(t, digest, hex.EncodeToString(actual[:]), name)
	}
}

func TestWorkspaceV2FixtureControlFrames(t *testing.T) {
	t.Parallel()
	rows := readWorkspaceJSONL[workspaceControlFixtureRow](t, "valid/control-frames.jsonl")
	require.NotEmpty(t, rows)
	covered := make(map[WorkspaceV2Action]map[WorkspaceV2Flow]bool)
	for _, row := range rows {
		require.NotEmpty(t, row.Case)
		t.Run(row.Case, func(t *testing.T) {
			decodeWorkspaceControlFixture(t, row)
		})
		_, payload := splitWorkspaceFixtureFrame(t, row.Frame)
		var envelope map[string]json.RawMessage
		require.NoError(t, json.Unmarshal(payload, &envelope))
		if _, hasData := envelope["data"]; !hasData {
			continue
		}
		if covered[row.Action] == nil {
			covered[row.Action] = make(map[WorkspaceV2Flow]bool)
		}
		covered[row.Action][row.Flow] = true
	}
	decodedMappings := 0
	for _, flows := range covered {
		decodedMappings += len(flows)
	}
	require.Equal(t, 25, decodedMappings)
	for _, action := range WorkspaceV2Actions {
		spec, ok := WorkspaceV2ActionSpecs[action]
		require.Truef(t, ok, "registry missing %s", action)
		for flow := range spec.Flows {
			require.Truef(t, covered[action][flow], "fixtures missing %s/%s", action, flow)
		}
	}
}

func TestDecodeWorkspaceV2DataCoversEveryClientRequestMapping(t *testing.T) {
	rows := readWorkspaceJSONL[workspaceControlFixtureRow](t, "valid/control-frames.jsonl")
	canonical := make(map[WorkspaceV2Action]workspaceControlFixtureRow)
	for _, row := range rows {
		if row.Flow == WorkspaceFlowClientRequest {
			if _, exists := canonical[row.Action]; !exists {
				canonical[row.Action] = row
			}
		}
	}
	tests := []struct {
		action        WorkspaceV2Action
		want          any
		requiredField string
	}{
		{WorkspaceActionHello, (*WorkspaceHelloRequest)(nil), "protocolVersion"},
		{WorkspaceActionSubscribe, (*WorkspaceSubscribeRequest)(nil), "workspaceId"},
		{WorkspaceActionMutation, (*WorkspaceMutation)(nil), "workspaceId"},
		{WorkspaceActionAck, (*WorkspaceAckRequest)(nil), "revision"},
		{WorkspaceActionBlobNeed, (*WorkspaceBlobNeedDownloadRequest)(nil), "workspaceId"},
		{WorkspaceActionBlobBegin, (*WorkspaceBlobBeginMessage)(nil), "chunkCount"},
		{WorkspaceActionBlobEnd, (*WorkspaceBlobEndMessage)(nil), "chunkCount"},
		{WorkspaceActionConflictResolved, (*WorkspaceConflictResolvedRequest)(nil), "choice"},
	}
	require.Len(t, canonical, len(tests))
	for _, tt := range tests {
		t.Run(string(tt.action), func(t *testing.T) {
			row, ok := canonical[tt.action]
			require.True(t, ok)
			data := workspaceFixtureRequestData(t, row)
			decoded, err := DecodeWorkspaceV2Data(tt.action, WorkspaceFlowClientRequest, data)
			require.NoError(t, err)
			require.IsType(t, tt.want, decoded)

			var object map[string]json.RawMessage
			require.NoError(t, json.Unmarshal(data, &object))
			object["unexpected"] = json.RawMessage("true")
			workspaceRequireInvalidData(t, tt.action, object)

			require.NoError(t, json.Unmarshal(data, &object))
			delete(object, tt.requiredField)
			workspaceRequireInvalidData(t, tt.action, object)

			require.NoError(t, json.Unmarshal(data, &object))
			object[tt.requiredField] = json.RawMessage("null")
			workspaceRequireInvalidData(t, tt.action, object)
		})
	}

	_, err := DecodeWorkspaceV2Data(WorkspaceV2Action("WorkspaceExtra"), WorkspaceFlowClientRequest, []byte(`{}`))
	requireWorkspaceValidationError(t, err, "action", "unknown_action")
	_, err = DecodeWorkspaceV2Data(WorkspaceActionSubscribe, WorkspaceFlowServerPush, []byte(`{}`))
	requireWorkspaceValidationError(t, err, "flow", "flow_not_allowed")

	helloData := workspaceFixtureRequestData(t, canonical[WorkspaceActionHello])
	_, err = DecodeWorkspaceV2Data(WorkspaceActionHello, WorkspaceFlowClientRequest, append(append([]byte(nil), helloData...), []byte(` {}`)...))
	requireWorkspaceValidationError(t, err, "data", "invalid_json")
	duplicate := bytes.Replace(helloData, []byte(`{"protocolVersion":`), []byte(`{"protocolVersion":"2","protocolVersion":`), 1)
	_, err = DecodeWorkspaceV2Data(WorkspaceActionHello, WorkspaceFlowClientRequest, duplicate)
	requireWorkspaceValidationError(t, err, "data", "invalid_json")
}

func workspaceRequireInvalidData(t *testing.T, action WorkspaceV2Action, object map[string]json.RawMessage) {
	t.Helper()
	data, err := json.Marshal(object)
	require.NoError(t, err)
	_, err = DecodeWorkspaceV2Data(action, WorkspaceFlowClientRequest, data)
	requireWorkspaceValidationError(t, err, "data", "invalid_json")
}

func TestWorkspaceV2CanonicalFixturesRejectEveryRequiredKeyOmission(t *testing.T) {
	rows := readWorkspaceJSONL[workspaceControlFixtureRow](t, "valid/control-frames.jsonl")
	canonical := make(map[string]workspaceControlFixtureRow)
	for _, row := range rows {
		key := string(row.Action) + "/" + string(row.Flow)
		_, exists := canonical[key]
		preferRequiredNulls := row.Case == "conflict-created-delete-modify-push" || row.Case == "conflict-resolve-delete-request"
		if !exists || preferRequiredNulls {
			canonical[key] = row
		}
	}
	require.Len(t, canonical, 25)

	covered := make(map[string]bool)
	for key, row := range canonical {
		row := row
		t.Run(key, func(t *testing.T) {
			_, payload := splitWorkspaceFixtureFrame(t, row.Frame)
			var document map[string]json.RawMessage
			require.NoError(t, json.Unmarshal(payload, &document))
			paths := workspaceFixtureObjectKeyPaths(t, document, "")
			for _, path := range paths {
				path := path
				t.Run(strings.ReplaceAll(path, ".", "/"), func(t *testing.T) {
					mutated := workspaceFixtureWithoutPath(t, document, path)
					dst := workspaceFixtureStrictDestination(t, row)
					err := strictJSONDecode(mutated, dst)
					if workspaceFixtureOptionalPath(row.Flow, path) {
						require.NoError(t, err, "omitempty path %s must be optional", path)
						return
					}
					require.Error(t, err, "required path %s decoded after omission", path)
					covered[row.Case+"|"+path] = true
				})
			}
		})
	}

	for _, key := range []string{
		"subscribe-request|data.lastAckRevision",
		"snapshot-begin-push|data.fromRevision",
		"snapshot-begin-push|data.eventCount",
		"snapshot-begin-push|data.conflictCount",
		"snapshot-entry-push-nfc|data.index",
		"snapshot-entry-push-nfc|data.entry.metadata.executable",
		"snapshot-entry-push-nfc|data.entry.tombstone",
		"mutation-request|data.basePathRevision",
		"mutation-rejected-response|data.currentPathState",
		"mutation-rejected-response|data.conflictId",
		"mutation-rejected-response|data.requiredHash",
		"blob-need-download-request-required-null|data.operationId",
		"blob-need-download-request-required-null|data.size",
		"blob-need-download-response-zero|data.size",
		"blob-begin-upload-request-zero|data.chunkCount",
		"conflict-created-delete-modify-push|data.incoming.path",
		"conflict-created-delete-modify-push|data.incoming.contentHash",
		"conflict-created-delete-modify-push|data.incoming.metadata.size",
		"conflict-created-delete-modify-push|data.incoming.metadata.modifiedAtMs",
		"conflict-created-delete-modify-push|data.incoming.metadata.executable",
		"conflict-created-delete-modify-push|data.incoming.tombstone",
		"conflict-resolve-delete-request|data.contentHash",
		"conflict-resolve-delete-request|data.metadata.size",
		"conflict-resolve-delete-request|data.metadata.modifiedAtMs",
		"conflict-resolve-delete-request|data.metadata.executable",
	} {
		require.True(t, covered[key], "explicit zero/null omission coverage missing %s", key)
	}
}

func TestWorkspaceV2CanonicalFixturesEnforceNullability(t *testing.T) {
	rows := readWorkspaceJSONL[workspaceControlFixtureRow](t, "valid/control-frames.jsonl")
	canonical := make(map[string]workspaceControlFixtureRow)
	for _, row := range rows {
		key := string(row.Action) + "/" + string(row.Flow)
		_, exists := canonical[key]
		preferRequiredNulls := row.Case == "conflict-created-delete-modify-push" || row.Case == "conflict-resolve-delete-request"
		if !exists || preferRequiredNulls {
			canonical[key] = row
		}
	}
	require.Len(t, canonical, 25)

	covered := make(map[string]bool)
	for key, row := range canonical {
		row := row
		t.Run(key, func(t *testing.T) {
			_, payload := splitWorkspaceFixtureFrame(t, row.Frame)
			var document map[string]json.RawMessage
			require.NoError(t, json.Unmarshal(payload, &document))
			for _, path := range workspaceFixtureObjectKeyPaths(t, document, "") {
				path := path
				t.Run(strings.ReplaceAll(path, ".", "/"), func(t *testing.T) {
					mutated := workspaceFixtureWithNullPath(t, document, path)
					dst := workspaceFixtureStrictDestination(t, row)
					err := strictJSONDecode(mutated, dst)
					if !workspaceFixtureNullablePath(row, path) {
						require.Error(t, err, "non-nullable path %s accepted null", path)
						covered[row.Case+"|"+path] = true
						return
					}
					require.NoError(t, err, "wire-nullable path %s rejected null", path)
					data := workspaceFixtureDecodedData(t, dst, row.Flow)
					original := workspaceFixtureRawAtPath(t, document, path)
					if bytes.Equal(bytes.TrimSpace(original), []byte("null")) {
						require.NoError(t, validateWorkspaceFixtureData(data), path)
					} else {
						require.Error(t, validateWorkspaceFixtureData(data), "cross-field validation accepted substituted null at %s", path)
					}
					covered[row.Case+"|"+path] = true
				})
			}
		})
	}

	for _, key := range []string{
		"hello-request|data.capabilities",
		"hello-response|status",
		"snapshot-begin-push|data.entryCount",
		"snapshot-begin-push|data.conflictCount",
		"snapshot-entry-push-nfc|data.index",
		"snapshot-entry-push-nfc|data.entry.metadata.executable",
		"mutation-rejected-response|data.currentPathState",
		"mutation-rejected-response|data.conflictId",
		"mutation-rejected-response|data.requiredHash",
		"blob-need-download-request-required-null|data.operationId",
		"blob-need-download-request-required-null|data.size",
		"conflict-created-delete-modify-push|data.ancestor.path",
		"conflict-created-delete-modify-push|data.incoming.path",
		"conflict-created-delete-modify-push|data.current.contentHash",
		"conflict-created-delete-modify-push|data.incoming.contentHash",
		"conflict-resolve-delete-request|data.contentHash",
	} {
		require.True(t, covered[key], "explicit nullability coverage missing %s", key)
	}
}

func workspaceFixtureWithNullPath(t *testing.T, document map[string]json.RawMessage, path string) []byte {
	t.Helper()
	raw, err := json.Marshal(document)
	require.NoError(t, err)
	var generic map[string]any
	require.NoError(t, json.Unmarshal(raw, &generic))
	current := generic
	parts := strings.Split(path, ".")
	for _, part := range parts[:len(parts)-1] {
		current = current[part].(map[string]any)
	}
	current[parts[len(parts)-1]] = nil
	raw, err = json.Marshal(generic)
	require.NoError(t, err)
	return raw
}

func workspaceFixtureRawAtPath(t *testing.T, document map[string]json.RawMessage, path string) json.RawMessage {
	t.Helper()
	current := document
	parts := strings.Split(path, ".")
	for _, part := range parts[:len(parts)-1] {
		var nested map[string]json.RawMessage
		require.NoError(t, json.Unmarshal(current[part], &nested))
		current = nested
	}
	return current[parts[len(parts)-1]]
}

func workspaceFixtureDecodedData(t *testing.T, dst any, flow WorkspaceV2Flow) any {
	t.Helper()
	data := reflect.ValueOf(dst).Elem().FieldByName("Data")
	if flow == WorkspaceFlowClientRequest {
		return data.Addr().Interface()
	}
	require.False(t, data.IsNil())
	return data.Interface()
}

func workspaceFixtureNullablePath(row workspaceControlFixtureRow, path string) bool {
	switch row.Action {
	case WorkspaceActionSnapshotEntry:
		return path == "data.entry.contentHash"
	case WorkspaceActionMutation:
		return path == "data.contentHash"
	case WorkspaceActionMutationAccepted:
		return path == "data.pathState.contentHash"
	case WorkspaceActionMutationRejected:
		return path == "data.currentPathState" || path == "data.conflictId" || path == "data.requiredHash"
	case WorkspaceActionEvent:
		return path == "data.mutation.contentHash" || path == "data.pathState.contentHash"
	case WorkspaceActionBlobNeed:
		if row.Flow == WorkspaceFlowClientRequest {
			return path == "data.operationId" || path == "data.size"
		}
		return row.Flow == WorkspaceFlowServerResponse && path == "data.operationId"
	case WorkspaceActionConflictCreated:
		return path == "data.ancestor.path" || path == "data.ancestor.contentHash" ||
			path == "data.current.path" || path == "data.current.contentHash" ||
			path == "data.incoming.path" || path == "data.incoming.contentHash"
	case WorkspaceActionConflictResolved:
		if row.Flow == WorkspaceFlowClientRequest {
			return path == "data.contentHash"
		}
		return path == "data.pathState.contentHash"
	default:
		return false
	}
}

func TestWorkspaceV2StrictDecoderRequiresFalseAndNestedSliceFields(t *testing.T) {
	type child struct {
		Count uint32 `json:"count"`
	}
	type body struct {
		Status bool    `json:"status"`
		Items  []child `json:"items"`
	}
	for _, raw := range []string{
		`{"items":[{"count":0}]}`,
		`{"status":false,"items":[{}]}`,
	} {
		var dst body
		require.Error(t, strictJSONDecode([]byte(raw), &dst))
	}
	var request WorkspaceV2Request[body]
	err := DecodeWorkspaceV2Request([]byte(`{"requestId":"10000000-0000-4000-8000-000000000001","data":{"status":false,"items":[{}]}}`), &request)
	requireWorkspaceValidationError(t, err, "frame", "invalid_json")
}

func TestWorkspaceV2FixtureStructSchemasRejectRequiredKeyOmissions(t *testing.T) {
	workspaceFixtureAssertRequiredFields(t, workspaceFixtureManifest{
		SchemaVersion: "workspace-sync-v2-fixtures/1",
		Actions:       []WorkspaceV2Action{WorkspaceActionHello},
		Files:         map[string]string{"valid/control-frames.jsonl": strings.Repeat("a", 64)},
	}, []string{"schemaVersion", "actions", "files"}, nil)

	workspaceFixtureAssertRequiredFields(t, workspaceControlFixtureRow{
		Case: "control", Sequence: "sequence", Step: 1,
		Action: WorkspaceActionHello, Flow: WorkspaceFlowClientRequest, Frame: `WorkspaceHello|{}`,
	}, []string{"case", "action", "flow", "frame"}, []string{"sequence", "step"})

	workspaceFixtureAssertRequiredFields(t, workspaceErrorFixtureRow{
		Case: "error", Action: WorkspaceActionHello, Frame: `WorkspaceHello|{}`,
	}, []string{"case", "action", "frame"}, nil)

	workspaceFixtureAssertRequiredFields(t, workspaceInvalidFixtureRow{
		Case: "invalid", Value: json.RawMessage(`"bad"`), Field: "path", Reason: "invalid",
	}, []string{"case", "value", "field", "reason"}, nil)

	workspaceFixtureAssertRequiredFields(t, workspaceBinaryHeaderVector{
		Case: "invalid-vector", Direction: WorkspaceBlobUpload, Final: false,
		TransferID: workspaceTestUUID(9), ChunkIndex: 0, Offset: 0,
		PayloadHex: "00", DigestHex: strings.Repeat("0", 64), HeaderHex: strings.Repeat("0", WorkspaceBlobHeaderSize*2),
		Valid: false, Reason: "invalid",
	}, []string{
		"case", "direction", "final", "transferId", "chunkIndex", "offset",
		"payloadHex", "digestHex", "headerHex", "valid",
	}, []string{"reason"})
}

func TestWorkspaceV2FixtureBinaryHeadersRejectMissingValid(t *testing.T) {
	row := workspaceBinaryHeaderVector{
		Case: "invalid-vector", Direction: WorkspaceBlobUpload, Final: false,
		TransferID: workspaceTestUUID(9), ChunkIndex: 0, Offset: 0,
		PayloadHex: "00", DigestHex: strings.Repeat("0", 64), HeaderHex: strings.Repeat("0", WorkspaceBlobHeaderSize*2),
		Valid: false, Reason: "invalid",
	}
	raw, err := json.Marshal(row)
	require.NoError(t, err)
	var object map[string]json.RawMessage
	require.NoError(t, json.Unmarshal(raw, &object))
	delete(object, "valid")
	raw, err = json.Marshal(object)
	require.NoError(t, err)
	raw = append(append([]byte{'['}, raw...), ']')

	var rows []workspaceBinaryHeaderVector
	require.Error(t, decodeWorkspaceBinaryHeaderVectors(raw, &rows))
}

func workspaceFixtureAssertRequiredFields(t *testing.T, value any, required, optional []string) {
	t.Helper()
	raw, err := json.Marshal(value)
	require.NoError(t, err)
	var object map[string]json.RawMessage
	require.NoError(t, json.Unmarshal(raw, &object))

	decode := func(document map[string]json.RawMessage) error {
		mutated, marshalErr := json.Marshal(document)
		require.NoError(t, marshalErr)
		return strictJSONDecode(mutated, reflect.New(reflect.TypeOf(value)).Interface())
	}
	require.NoError(t, decode(object))

	for _, field := range required {
		field := field
		t.Run("required/"+field, func(t *testing.T) {
			mutated := workspaceFixtureTopLevelCopy(object)
			delete(mutated, field)
			require.Error(t, decode(mutated), "required fixture field %s decoded after omission", field)
		})
	}
	for _, field := range optional {
		field := field
		t.Run("optional/"+field, func(t *testing.T) {
			mutated := workspaceFixtureTopLevelCopy(object)
			delete(mutated, field)
			require.NoError(t, decode(mutated), "omitempty fixture field %s must remain optional", field)
		})
	}
}

func workspaceFixtureTopLevelCopy(object map[string]json.RawMessage) map[string]json.RawMessage {
	clone := make(map[string]json.RawMessage, len(object))
	for key, value := range object {
		clone[key] = append(json.RawMessage(nil), value...)
	}
	return clone
}

func workspaceFixtureObjectKeyPaths(t *testing.T, object map[string]json.RawMessage, prefix string) []string {
	t.Helper()
	var paths []string
	for key, raw := range object {
		path := key
		if prefix != "" {
			path = prefix + "." + key
		}
		paths = append(paths, path)
		var nested map[string]json.RawMessage
		if len(raw) > 0 && raw[0] == '{' && json.Unmarshal(raw, &nested) == nil {
			paths = append(paths, workspaceFixtureObjectKeyPaths(t, nested, path)...)
		}
	}
	sort.Strings(paths)
	return paths
}

func workspaceFixtureWithoutPath(t *testing.T, document map[string]json.RawMessage, path string) []byte {
	t.Helper()
	var clone map[string]json.RawMessage
	raw, err := json.Marshal(document)
	require.NoError(t, err)
	require.NoError(t, json.Unmarshal(raw, &clone))
	parts := strings.Split(path, ".")
	current := clone
	for _, part := range parts[:len(parts)-1] {
		var nested map[string]json.RawMessage
		require.NoError(t, json.Unmarshal(current[part], &nested))
		current[part], err = json.Marshal(nested)
		require.NoError(t, err)
		current = nested
	}
	delete(current, parts[len(parts)-1])
	// Rebuild from the original document so nested map mutations are retained.
	var root any = clone
	raw, err = json.Marshal(root)
	require.NoError(t, err)
	var generic map[string]any
	require.NoError(t, json.Unmarshal(raw, &generic))
	genericCurrent := generic
	for _, part := range parts[:len(parts)-1] {
		genericCurrent = genericCurrent[part].(map[string]any)
	}
	delete(genericCurrent, parts[len(parts)-1])
	raw, err = json.Marshal(generic)
	require.NoError(t, err)
	return raw
}

func workspaceFixtureStrictDestination(t *testing.T, row workspaceControlFixtureRow) any {
	t.Helper()
	data, err := NewWorkspaceV2Data(row.Action, row.Flow)
	require.NoError(t, err)
	dataType := reflect.TypeOf(data).Elem()
	fields := []reflect.StructField{}
	if row.Flow == WorkspaceFlowClientRequest {
		fields = append(fields, reflect.StructField{Name: "RequestID", Type: reflect.TypeOf(WorkspaceUUID("")), Tag: `json:"requestId"`})
	} else {
		fields = append(fields, reflect.StructField{Name: "RequestID", Type: reflect.TypeOf((*WorkspaceUUID)(nil)), Tag: `json:"requestId,omitempty"`})
		fields = append(fields, reflect.StructField{Name: "Status", Type: reflect.TypeOf(false), Tag: `json:"status"`})
	}
	if row.Flow == WorkspaceFlowClientRequest {
		fields = append(fields, reflect.StructField{Name: "Data", Type: dataType, Tag: `json:"data"`})
	} else {
		fields = append(fields, reflect.StructField{Name: "Data", Type: reflect.PointerTo(dataType), Tag: `json:"data,omitempty"`})
		fields = append(fields, reflect.StructField{Name: "Error", Type: reflect.TypeOf((*WorkspaceV2Error)(nil)), Tag: `json:"error,omitempty"`})
	}
	return reflect.New(reflect.StructOf(fields)).Interface()
}

func workspaceFixtureOptionalPath(flow WorkspaceV2Flow, path string) bool {
	if flow != WorkspaceFlowClientRequest && (path == "requestId" || path == "data" || path == "error") {
		return true
	}
	switch path {
	case "data.newPath", "data.targetBasePathRevision", "data.oldPathState", "data.newPathState",
		"data.mutation.newPath", "data.mutation.targetBasePathRevision":
		return true
	default:
		return false
	}
}

func TestWorkspaceV2FixtureErrorEnvelopes(t *testing.T) {
	t.Parallel()
	rows := readWorkspaceJSONL[workspaceErrorFixtureRow](t, "valid/error-envelopes.jsonl")
	require.Len(t, rows, len(WorkspaceV2ErrorCodes))
	seen := make(map[WorkspaceV2ErrorCode]bool)
	for _, row := range rows {
		action, payload := splitWorkspaceFixtureFrame(t, row.Frame)
		require.Equal(t, row.Action, action)
		var envelope WorkspaceV2Response[json.RawMessage]
		require.NoError(t, strictJSONDecode(payload, &envelope))
		require.NoError(t, envelope.Validate())
		require.False(t, envelope.Status)
		require.NotNil(t, envelope.RequestID)
		require.NotNil(t, envelope.Error)
		require.Contains(t, WorkspaceV2ErrorCodes, envelope.Error.Code)
		require.False(t, seen[envelope.Error.Code], "duplicate %s", envelope.Error.Code)
		seen[envelope.Error.Code] = true
		want := NewWorkspaceV2Error(envelope.Error.Code, envelope.Error.Fields...)
		require.Equal(t, want, *envelope.Error)
		if envelope.Error.Code == WorkspaceErrorBlobRequired {
			require.False(t, envelope.Error.Retryable)
		}
		encoded, err := json.Marshal(envelope)
		require.NoError(t, err)
		require.Equal(t, string(payload), string(encoded))
	}
	for _, code := range WorkspaceV2ErrorCodes {
		require.Truef(t, seen[code], "missing error code %s", code)
	}
}

func TestWorkspaceV2FixtureInvalidPrimitives(t *testing.T) {
	t.Parallel()
	tests := []struct {
		name  string
		parse func(json.RawMessage) error
	}{
		{name: "invalid/revisions.jsonl", parse: func(raw json.RawMessage) error {
			var value WorkspaceRevision
			return json.Unmarshal(raw, &value)
		}},
		{name: "invalid/hashes.jsonl", parse: func(raw json.RawMessage) error {
			var value WorkspaceContentHash
			return json.Unmarshal(raw, &value)
		}},
		{name: "invalid/paths.jsonl", parse: func(raw json.RawMessage) error {
			var value WorkspacePath
			return json.Unmarshal(raw, &value)
		}},
	}
	for _, tt := range tests {
		rows := readWorkspaceJSONL[workspaceInvalidFixtureRow](t, tt.name)
		require.NotEmpty(t, rows)
		for _, row := range rows {
			require.NotEmpty(t, row.Case)
			require.NotEmpty(t, row.Field)
			require.NotEmpty(t, row.Reason)
			err := tt.parse(row.Value)
			requireWorkspaceValidationError(t, err, row.Field, row.Reason)
		}
	}
}

func TestWorkspaceV2FixtureBinaryHeaders(t *testing.T) {
	t.Parallel()
	raw, err := os.ReadFile(filepath.Join(workspaceFixtureRoot(), "binary", "header-vectors.json"))
	require.NoError(t, err)
	var rows []workspaceBinaryHeaderVector
	require.NoError(t, decodeWorkspaceBinaryHeaderVectors(raw, &rows))
	require.NotEmpty(t, rows)
	validDirections := make(map[WorkspaceBlobDirection]bool)
	var sawFinal, sawPartial, sawValidZero, sawInvalidZero, sawHeaderDigestMismatch, sawPayloadMismatch bool
	for _, row := range rows {
		t.Run(row.Case, func(t *testing.T) {
			testWorkspaceBinaryHeaderVector(t, row)
		})
		if row.Valid {
			validDirections[row.Direction] = true
			sawFinal = sawFinal || row.Final
			payload, decodeErr := hex.DecodeString(row.PayloadHex)
			require.NoError(t, decodeErr)
			sawPartial = sawPartial || (len(payload) > 0 && len(payload) < WorkspaceBlobChunkSize)
			sawValidZero = sawValidZero || len(payload) == 0
		} else {
			payload, decodeErr := hex.DecodeString(row.PayloadHex)
			require.NoError(t, decodeErr)
			sawInvalidZero = sawInvalidZero || len(payload) == 0
			sawHeaderDigestMismatch = sawHeaderDigestMismatch || row.Reason == "mismatch"
			sawPayloadMismatch = sawPayloadMismatch || row.Reason == "full_digest_mismatch"
		}
	}
	require.True(t, validDirections[WorkspaceBlobUpload])
	require.True(t, validDirections[WorkspaceBlobDownload])
	require.True(t, sawFinal)
	require.True(t, sawPartial)
	require.False(t, sawValidZero)
	require.True(t, sawInvalidZero)
	require.True(t, sawHeaderDigestMismatch)
	require.True(t, sawPayloadMismatch)
}

func decodeWorkspaceBinaryHeaderVectors(raw []byte, rows *[]workspaceBinaryHeaderVector) error {
	wrapped := make([]byte, 0, len(raw)+len(`{"rows":}`))
	wrapped = append(wrapped, `{"rows":`...)
	wrapped = append(wrapped, raw...)
	wrapped = append(wrapped, '}')
	var vectors workspaceBinaryHeaderVectors
	if err := strictJSONDecode(wrapped, &vectors); err != nil {
		return err
	}
	*rows = vectors.Rows
	return nil
}

func TestWorkspaceMergedConflictUploadSequenceFixtures(t *testing.T) {
	t.Parallel()
	rows := readWorkspaceJSONL[workspaceControlFixtureRow](t, "valid/control-frames.jsonl")
	sequences := make(map[string][]workspaceControlFixtureRow)
	for _, row := range rows {
		if row.Sequence != "" {
			sequences[row.Sequence] = append(sequences[row.Sequence], row)
		}
	}
	require.ElementsMatch(t, []string{
		"merged-conflict-upload", "merged-conflict-reconnect-missing", "merged-conflict-stale",
		"snapshot-full-conflicts", "snapshot-incremental-conflicts",
	}, workspaceStringKeys(sequences))
	for name, sequence := range sequences {
		sort.Slice(sequence, func(i, j int) bool { return sequence[i].Step < sequence[j].Step })
		for i, row := range sequence {
			require.Equal(t, uint32(i+1), row.Step, "%s step", name)
			if row.Flow == WorkspaceFlowClientRequest {
				require.Less(t, i+1, len(sequence), "%s client request must terminate", name)
				next := sequence[i+1]
				require.Equal(t, WorkspaceFlowServerResponse, next.Flow, "%s step %d", name, row.Step)
				require.Equal(t, row.Action, next.Action)
				require.Equal(t, workspaceFixtureRequestID(t, row), workspaceFixtureRequestID(t, next))
			}
		}
	}

	upload := sortedWorkspaceSequence(sequences["merged-conflict-upload"])
	require.Len(t, upload, 9)
	requireWorkspaceFixtureErrorCode(t, upload[1], WorkspaceErrorBlobRequired)
	require.Equal(t, WorkspaceActionBlobNeed, upload[2].Action)
	require.Equal(t, WorkspaceFlowServerPush, upload[2].Flow)
	firstResolve := workspaceFixtureRequestData(t, upload[0])
	retryResolve := workspaceFixtureRequestData(t, upload[7])
	require.JSONEq(t, string(firstResolve), string(retryResolve))
	require.NotEqual(t, workspaceFixtureRequestID(t, upload[0]), workspaceFixtureRequestID(t, upload[7]))
	var resolve WorkspaceConflictResolvedRequest
	require.NoError(t, strictJSONDecode(firstResolve, &resolve))
	var needEnvelope WorkspaceV2Response[json.RawMessage]
	_, needPayload := splitWorkspaceFixtureFrame(t, upload[2].Frame)
	require.NoError(t, strictJSONDecode(needPayload, &needEnvelope))
	var need WorkspaceBlobNeedUploadPush
	require.NoError(t, strictJSONDecode(*needEnvelope.Data, &need))
	require.Equal(t, resolve.OperationID, need.OperationID)
	require.Equal(t, *resolve.ContentHash.Value, need.ContentHash)

	reconnect := sortedWorkspaceSequence(sequences["merged-conflict-reconnect-missing"])
	require.Len(t, reconnect, 3)
	requireWorkspaceFixtureErrorCode(t, reconnect[1], WorkspaceErrorBlobRequired)
	require.Equal(t, WorkspaceActionBlobNeed, reconnect[2].Action)
	require.JSONEq(t, string(firstResolve), string(workspaceFixtureRequestData(t, reconnect[0])))

	stale := sortedWorkspaceSequence(sequences["merged-conflict-stale"])
	require.Len(t, stale, 2)
	requireWorkspaceFixtureErrorCode(t, stale[1], WorkspaceErrorConflictRevisionStale)
}

func TestWorkspaceSnapshotConflictSequenceFixtures(t *testing.T) {
	t.Parallel()
	rows := readWorkspaceJSONL[workspaceControlFixtureRow](t, "valid/control-frames.jsonl")
	sequences := make(map[string][]workspaceControlFixtureRow)
	for _, row := range rows {
		if strings.HasPrefix(row.Sequence, "snapshot-") {
			sequences[row.Sequence] = append(sequences[row.Sequence], row)
		}
	}

	full := sortedWorkspaceSequence(sequences["snapshot-full-conflicts"])
	require.Equal(t, []WorkspaceV2Action{
		WorkspaceActionSnapshotBegin, WorkspaceActionSnapshotEntry,
		WorkspaceActionConflictCreated, WorkspaceActionSnapshotEnd,
	}, workspaceFixtureActions(full))
	fullBegin := workspaceFixturePushData[WorkspaceSnapshotBeginMessage](t, full[0])
	fullEnd := workspaceFixturePushData[WorkspaceSnapshotEndMessage](t, full[3])
	require.Equal(t, WorkspaceSnapshotFull, fullBegin.Mode)
	require.Equal(t, uint32(1), fullBegin.EntryCount)
	require.Zero(t, fullBegin.EventCount)
	require.Equal(t, uint32(1), fullBegin.ConflictCount)
	require.Equal(t, uint32(2), fullEnd.DeliveredCount)
	require.NoError(t, fullEnd.ValidateAgainst(fullBegin))

	incremental := sortedWorkspaceSequence(sequences["snapshot-incremental-conflicts"])
	require.Equal(t, []WorkspaceV2Action{
		WorkspaceActionSnapshotBegin, WorkspaceActionEvent, WorkspaceActionConflictResolved,
		WorkspaceActionEvent, WorkspaceActionConflictCreated, WorkspaceActionConflictCreated,
		WorkspaceActionSnapshotEnd,
	}, workspaceFixtureActions(incremental))
	for _, row := range incremental {
		require.Equal(t, WorkspaceFlowServerPush, row.Flow)
	}
	incrementalBegin := workspaceFixturePushData[WorkspaceSnapshotBeginMessage](t, incremental[0])
	firstEvent := workspaceFixturePushData[WorkspaceEventMessage](t, incremental[1])
	resolved := workspaceFixturePushData[WorkspaceConflictResolvedMessage](t, incremental[2])
	secondEvent := workspaceFixturePushData[WorkspaceEventMessage](t, incremental[3])
	firstConflict := workspaceFixturePushData[WorkspaceConflictCreatedMessage](t, incremental[4])
	secondConflict := workspaceFixturePushData[WorkspaceConflictCreatedMessage](t, incremental[5])
	incrementalEnd := workspaceFixturePushData[WorkspaceSnapshotEndMessage](t, incremental[6])
	require.Equal(t, WorkspaceSnapshotIncremental, incrementalBegin.Mode)
	require.Zero(t, incrementalBegin.EntryCount)
	require.Equal(t, uint32(3), incrementalBegin.EventCount)
	require.Equal(t, uint32(2), incrementalBegin.ConflictCount)
	require.NoError(t, firstEvent.Validate(0, incrementalBegin.FromRevision))
	require.Less(t, firstEvent.Revision, resolved.Revision)
	require.NoError(t, secondEvent.Validate(firstEvent.Index, resolved.Revision))
	require.Equal(t, firstEvent.Index+1, secondEvent.Index)
	require.Less(t, resolved.Revision, secondEvent.Revision)
	require.Equal(t, incrementalBegin.FinalRevision, secondEvent.Revision)
	require.Less(t, bytes.Compare([]byte(firstConflict.ConflictID), []byte(secondConflict.ConflictID)), 0)
	require.Equal(t, incrementalBegin.WorkspaceID, firstConflict.WorkspaceID)
	require.Equal(t, incrementalBegin.WorkspaceID, secondConflict.WorkspaceID)
	for _, conflictAction := range []struct {
		row    workspaceControlFixtureRow
		action WorkspaceV2Action
	}{
		{row: incremental[2], action: WorkspaceActionConflictResolved},
		{row: incremental[4], action: WorkspaceActionConflictCreated},
		{row: incremental[5], action: WorkspaceActionConflictCreated},
	} {
		raw := workspaceFixturePushRawData(t, conflictAction.row)
		var object map[string]json.RawMessage
		require.NoError(t, json.Unmarshal(raw, &object))
		for _, injected := range []struct {
			field string
			value json.RawMessage
		}{
			{field: "streamId", value: json.RawMessage(`"10000000-0000-4000-8000-000000000003"`)},
			{field: "index", value: json.RawMessage(`0`)},
		} {
			require.NotContains(t, object, injected.field)
			object[injected.field] = injected.value
			mutated, err := json.Marshal(object)
			require.NoError(t, err)
			_, err = DecodeWorkspaceV2Data(conflictAction.action, WorkspaceFlowServerPush, mutated)
			requireWorkspaceValidationError(t, err, "data", "invalid_json")
			delete(object, injected.field)
		}
	}
	require.Equal(t, uint32(5), incrementalEnd.DeliveredCount)
	require.NoError(t, incrementalEnd.ValidateAgainst(incrementalBegin))
}

func workspaceFixtureActions(rows []workspaceControlFixtureRow) []WorkspaceV2Action {
	actions := make([]WorkspaceV2Action, len(rows))
	for i, row := range rows {
		actions[i] = row.Action
	}
	return actions
}

func workspaceFixturePushData[T any](t *testing.T, row workspaceControlFixtureRow) T {
	t.Helper()
	raw := workspaceFixturePushRawData(t, row)
	var data T
	require.NoError(t, strictJSONDecode(raw, &data))
	return data
}

func workspaceFixturePushRawData(t *testing.T, row workspaceControlFixtureRow) json.RawMessage {
	t.Helper()
	require.Equal(t, WorkspaceFlowServerPush, row.Flow)
	_, payload := splitWorkspaceFixtureFrame(t, row.Frame)
	var envelope WorkspaceV2Response[json.RawMessage]
	require.NoError(t, strictJSONDecode(payload, &envelope))
	require.True(t, envelope.Status)
	require.NotNil(t, envelope.Data)
	return *envelope.Data
}

func readWorkspaceJSONL[T any](t *testing.T, name string) []T {
	t.Helper()
	file, err := os.Open(filepath.Join(workspaceFixtureRoot(), filepath.FromSlash(name)))
	require.NoError(t, err)
	t.Cleanup(func() { require.NoError(t, file.Close()) })
	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 4096), WorkspaceMaxControlFrameBytes*2)
	var rows []T
	for scanner.Scan() {
		line := bytes.TrimSpace(scanner.Bytes())
		if len(line) == 0 {
			continue
		}
		var row T
		require.NoError(t, strictJSONDecode(line, &row))
		rows = append(rows, row)
	}
	require.NoError(t, scanner.Err())
	return rows
}

func splitWorkspaceFixtureFrame(t *testing.T, frame string) (WorkspaceV2Action, []byte) {
	t.Helper()
	actionText, payload, ok := strings.Cut(frame, "|")
	require.True(t, ok)
	action := WorkspaceV2Action(actionText)
	require.Contains(t, WorkspaceV2Actions, action)
	require.NotEmpty(t, payload)
	return action, []byte(payload)
}

func decodeWorkspaceControlFixture(t *testing.T, row workspaceControlFixtureRow) {
	t.Helper()
	action, payload := splitWorkspaceFixtureFrame(t, row.Frame)
	require.Equal(t, row.Action, action)
	var data json.RawMessage
	switch row.Flow {
	case WorkspaceFlowClientRequest:
		var envelope WorkspaceV2Request[json.RawMessage]
		require.NoError(t, strictJSONDecode(payload, &envelope))
		require.NotEmpty(t, envelope.RequestID)
		data = envelope.Data
		dst, err := DecodeWorkspaceV2Data(row.Action, row.Flow, data)
		require.NoError(t, err)
		require.NoError(t, validateWorkspaceFixtureData(dst))
		encodedData, err := json.Marshal(dst)
		require.NoError(t, err)
		require.Equal(t, string(data), string(encodedData))
		envelope.Data = encodedData
		encoded, err := json.Marshal(envelope)
		require.NoError(t, err)
		require.Equal(t, string(payload), string(encoded))
	case WorkspaceFlowServerResponse, WorkspaceFlowServerPush:
		var envelope WorkspaceV2Response[json.RawMessage]
		require.NoError(t, strictJSONDecode(payload, &envelope))
		require.NoError(t, envelope.Validate())
		if row.Flow == WorkspaceFlowServerPush {
			require.Nil(t, envelope.RequestID)
			require.True(t, envelope.Status)
		} else {
			require.NotNil(t, envelope.RequestID)
		}
		if envelope.Status {
			require.NotNil(t, envelope.Data)
			data = *envelope.Data
			dst, err := DecodeWorkspaceV2Data(row.Action, row.Flow, data)
			require.NoError(t, err)
			require.NoError(t, validateWorkspaceFixtureData(dst))
			encodedData, err := json.Marshal(dst)
			require.NoError(t, err)
			require.Equal(t, string(data), string(encodedData))
			canonicalData := json.RawMessage(encodedData)
			envelope.Data = &canonicalData
		} else {
			require.NotNil(t, envelope.Error)
			require.Contains(t, WorkspaceV2ErrorCodes, envelope.Error.Code)
		}
		encoded, err := json.Marshal(envelope)
		require.NoError(t, err)
		require.Equal(t, string(payload), string(encoded))
	default:
		t.Fatalf("unknown flow %q", row.Flow)
	}
}

func validateWorkspaceFixtureData(value any) error {
	switch m := value.(type) {
	case *WorkspaceHelloRequest:
		return m.Validate()
	case *WorkspaceHelloResponse:
		return m.Validate()
	case *WorkspaceSubscribeRequest:
		return m.Validate()
	case *WorkspaceSnapshotBeginMessage:
		return m.Validate()
	case *WorkspaceSnapshotEntryMessage:
		return m.Validate(m.Index)
	case *WorkspaceSnapshotEndMessage:
		begin := WorkspaceSnapshotBeginMessage{WorkspaceID: m.WorkspaceID, StreamID: m.StreamID, Mode: m.Mode, FinalRevision: m.FinalRevision}
		if m.Mode == WorkspaceSnapshotFull {
			begin.EntryCount = m.DeliveredCount
		} else {
			begin.EventCount = m.DeliveredCount
		}
		return m.ValidateAgainst(begin)
	case *WorkspaceMutation:
		return m.Validate()
	case *WorkspaceMutationAcceptedMessage:
		return m.Validate()
	case *WorkspaceMutationRejectedMessage:
		return m.Validate()
	case *WorkspaceEventMessage:
		if m.Index == 0 || m.Revision == 0 {
			return fmt.Errorf("event fixture requires positive index/revision")
		}
		return m.Validate(m.Index-1, m.Revision-1)
	case *WorkspaceAckRequest:
		if m.Revision == 0 {
			return fmt.Errorf("ack fixture requires positive revision")
		}
		return m.Validate(m.Revision-1, m.Revision)
	case *WorkspaceBlobNeedUploadPush:
		return m.Validate()
	case *WorkspaceBlobNeedDownloadRequest:
		return m.Validate()
	case *WorkspaceBlobNeedDownloadResponse:
		return m.Validate()
	case *WorkspaceBlobBeginMessage:
		return m.Validate()
	case *WorkspaceBlobEndMessage:
		return m.Validate()
	case *WorkspaceConflictCreatedMessage:
		return m.Validate()
	case *WorkspaceConflictResolvedRequest:
		return m.ValidateAgainst(workspaceFixtureConflictForResolution(*m))
	case *WorkspaceConflictResolvedMessage:
		return m.Validate()
	default:
		return fmt.Errorf("unhandled fixture DTO %T", value)
	}
}

func workspaceFixtureConflictForResolution(resolve WorkspaceConflictResolvedRequest) WorkspaceConflictCreatedMessage {
	const pathRevision WorkspaceRevision = 11
	path := resolve.Path
	hash := resolve.ContentHash
	if hash.Value == nil {
		hash = workspaceHashValue()
	}
	side := WorkspaceConflictSide{Path: &path, PathRevision: pathRevision, ContentHash: hash, Metadata: resolve.Metadata}
	if err := side.Metadata.Validate(WorkspaceEntryFile); err != nil {
		side.Metadata = WorkspaceFileMetadata{Size: 1, ModifiedAtMS: 1}
	}
	created := WorkspaceConflictCreatedMessage{
		WorkspaceID: resolve.WorkspaceID, ConflictID: resolve.ConflictID, ConflictRevision: resolve.ConflictRevision,
		Path: path, Kind: WorkspaceConflictContent, Ancestor: side, Current: side, Incoming: side,
		CreatedByOperationID: workspaceTestUUID(8),
	}
	if resolve.Choice == WorkspaceConflictKeepCurrent {
		created.Current = WorkspaceConflictSide{Path: &path, PathRevision: pathRevision, ContentHash: resolve.ContentHash, Metadata: resolve.Metadata}
	}
	if resolve.Choice == WorkspaceConflictUseIncoming {
		created.Incoming = WorkspaceConflictSide{Path: &path, PathRevision: pathRevision, ContentHash: resolve.ContentHash, Metadata: resolve.Metadata}
	}
	return created
}

func testWorkspaceBinaryHeaderVector(t *testing.T, row workspaceBinaryHeaderVector) {
	t.Helper()
	payload, err := hex.DecodeString(row.PayloadHex)
	require.NoError(t, err)
	wantFull, err := hex.DecodeString(row.DigestHex)
	require.NoError(t, err)
	require.Len(t, wantFull, 32)
	computed := blake3.Sum256(payload)
	full, first16 := ComputeWorkspaceBlobDigest(payload)
	require.Equal(t, computed, full)

	headerBytes, err := hex.DecodeString(row.HeaderHex)
	require.NoError(t, err)
	require.Len(t, headerBytes, WorkspaceBlobHeaderSize)
	header, parseErr := UnmarshalWorkspaceBlobHeader(headerBytes, uint32(len(payload)), first16)
	fullMatches := bytes.Equal(wantFull, computed[:])
	if !row.Valid {
		require.NotEmpty(t, row.Reason)
		if parseErr != nil {
			var validationErr *WorkspaceValidationError
			require.ErrorAs(t, parseErr, &validationErr)
			require.Equal(t, row.Reason, validationErr.Reason)
		} else {
			require.False(t, fullMatches)
			require.Equal(t, "full_digest_mismatch", row.Reason)
		}
		return
	}
	require.NoError(t, parseErr)
	require.True(t, fullMatches, "full BLAKE3 must be independently replayable")
	require.Equal(t, computed[:16], headerBytes[48:64])
	require.Equal(t, row.Direction, header.Direction)
	require.Equal(t, row.Final, header.Final)
	require.Equal(t, row.TransferID, WorkspaceUUID(header.TransferID.String()))
	require.Equal(t, row.ChunkIndex, header.ChunkIndex)
	require.Equal(t, row.Offset, header.Offset)
	encoded, err := MarshalWorkspaceBlobHeader(header)
	require.NoError(t, err)
	require.Equal(t, headerBytes, encoded[:])
}

func workspaceFixtureRequestID(t *testing.T, row workspaceControlFixtureRow) WorkspaceUUID {
	t.Helper()
	_, payload := splitWorkspaceFixtureFrame(t, row.Frame)
	var envelope struct {
		RequestID WorkspaceUUID `json:"requestId"`
	}
	require.NoError(t, json.Unmarshal(payload, &envelope))
	require.NotEmpty(t, envelope.RequestID)
	return envelope.RequestID
}

func workspaceFixtureRequestData(t *testing.T, row workspaceControlFixtureRow) json.RawMessage {
	t.Helper()
	_, payload := splitWorkspaceFixtureFrame(t, row.Frame)
	var envelope WorkspaceV2Request[json.RawMessage]
	require.NoError(t, strictJSONDecode(payload, &envelope))
	return envelope.Data
}

func requireWorkspaceFixtureErrorCode(t *testing.T, row workspaceControlFixtureRow, code WorkspaceV2ErrorCode) {
	t.Helper()
	_, payload := splitWorkspaceFixtureFrame(t, row.Frame)
	var envelope WorkspaceV2Response[json.RawMessage]
	require.NoError(t, strictJSONDecode(payload, &envelope))
	require.False(t, envelope.Status)
	require.NotNil(t, envelope.Error)
	require.Equal(t, code, envelope.Error.Code)
	require.False(t, envelope.Error.Retryable)
}

func sortedWorkspaceSequence(rows []workspaceControlFixtureRow) []workspaceControlFixtureRow {
	result := append([]workspaceControlFixtureRow(nil), rows...)
	sort.Slice(result, func(i, j int) bool { return result[i].Step < result[j].Step })
	return result
}

func workspaceStringKeys[V any](values map[string]V) []string {
	keys := make([]string, 0, len(values))
	for key := range values {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	return keys
}
