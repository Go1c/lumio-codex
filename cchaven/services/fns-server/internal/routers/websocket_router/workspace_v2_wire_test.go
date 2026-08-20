package websocket_router

import (
	"bytes"
	"encoding/json"
	"errors"
	"testing"

	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/stretchr/testify/require"
)

const workspaceV2WireRequestID = "10000000-0000-4000-8000-000000000001"

func TestWorkspaceV2ControlFrameAcceptsExactly65536Bytes(t *testing.T) {
	frame := workspaceV2UnknownFrameWithDataLength(65_536)
	_, action, err := decodeWorkspaceV2ControlFrame(frame)
	require.Equal(t, "FutureAction", action)
	var wireErr *workspaceV2WireError
	require.ErrorAs(t, err, &wireErr)
	require.Equal(t, dto.WorkspaceErrorUnknownAction, wireErr.Code)
	require.Zero(t, wireErr.CloseCode)
}

func TestWorkspaceV2ControlFrameCloses1009At65537Bytes(t *testing.T) {
	frame := workspaceV2UnknownFrameWithDataLength(65_537)
	_, action, err := decodeWorkspaceV2ControlFrame(frame)
	require.Empty(t, action)
	var wireErr *workspaceV2WireError
	require.ErrorAs(t, err, &wireErr)
	require.Equal(t, uint16(1009), wireErr.CloseCode)
}

func TestWorkspaceV2ControlFrameCloses1002ForMissingSeparatorInvalidUTF8OrUnsafeAction(t *testing.T) {
	cases := map[string][]byte{
		"missing separator": []byte("FutureAction"),
		"invalid utf8":      append([]byte("FutureAction|"), 0xff),
		"unsafe action":     []byte("Future-Action|{}"),
	}
	for name, frame := range cases {
		t.Run(name, func(t *testing.T) {
			_, _, err := decodeWorkspaceV2ControlFrame(frame)
			var wireErr *workspaceV2WireError
			require.ErrorAs(t, err, &wireErr)
			require.Equal(t, uint16(1002), wireErr.CloseCode)
		})
	}
}

func TestWorkspaceV2MalformedEnvelopeReturnsSameKnownActionInvalidJSON(t *testing.T) {
	frame := []byte("WorkspaceHello|{\"requestId\":\"" + workspaceV2WireRequestID + "\",\"data\":{}}")
	_, action, err := decodeWorkspaceV2ControlFrame(frame)
	require.Equal(t, string(dto.WorkspaceActionHello), action)
	var wireErr *workspaceV2WireError
	require.ErrorAs(t, err, &wireErr)
	require.Equal(t, dto.WorkspaceErrorInvalidJSON, wireErr.Code)
	require.Equal(t, workspaceV2WireRequestID, string(*wireErr.RequestID))
}

func TestWorkspaceV2KnownServerOnlyActionReturnsSameActionInvalidRequest(t *testing.T) {
	frame := []byte("WorkspaceSnapshotBegin|{\"requestId\":\"" + workspaceV2WireRequestID + "\",\"data\":{}}")
	_, action, err := decodeWorkspaceV2ControlFrame(frame)
	require.Equal(t, string(dto.WorkspaceActionSnapshotBegin), action)
	var wireErr *workspaceV2WireError
	require.ErrorAs(t, err, &wireErr)
	require.Equal(t, dto.WorkspaceErrorInvalidRequest, wireErr.Code)
	require.Equal(t, workspaceV2WireRequestID, string(*wireErr.RequestID))
}

func TestWorkspaceV2SafeUnknownActionEchoesUnknownActionOnceWithoutRegistryMutation(t *testing.T) {
	actionsBefore := append([]dto.WorkspaceV2Action(nil), dto.WorkspaceV2Actions...)
	specsBefore := make(map[dto.WorkspaceV2Action]dto.WorkspaceV2ActionSpec, len(dto.WorkspaceV2ActionSpecs))
	for action, spec := range dto.WorkspaceV2ActionSpecs {
		specsBefore[action] = spec
	}
	frame := []byte("FutureAction|{\"requestId\":\"" + workspaceV2WireRequestID + "\",\"data\":{}}")
	_, action, err := decodeWorkspaceV2ControlFrame(frame)
	var wireErr *workspaceV2WireError
	require.ErrorAs(t, err, &wireErr)
	raw, encodeErr := encodeWorkspaceV2WireError(action, wireErr)
	require.NoError(t, encodeErr)
	require.Equal(t, "FutureAction", string(raw[:len("FutureAction")]))
	var response dto.WorkspaceV2Response[struct{}]
	require.NoError(t, json.Unmarshal(raw[len("FutureAction|"):], &response))
	require.False(t, response.Status)
	require.Equal(t, dto.WorkspaceErrorUnknownAction, response.Error.Code)
	require.Equal(t, actionsBefore, dto.WorkspaceV2Actions)
	require.Equal(t, specsBefore, dto.WorkspaceV2ActionSpecs)

	_, _, err = decodeWorkspaceV2ControlFrame([]byte("Future-Action|{}"))
	require.Error(t, err)
	var unsafeErr *workspaceV2WireError
	require.True(t, errors.As(err, &unsafeErr))
	require.Equal(t, uint16(1002), unsafeErr.CloseCode)
}

func workspaceV2UnknownFrameWithDataLength(total int) []byte {
	prefix := []byte("FutureAction|{\"requestId\":\"" + workspaceV2WireRequestID + "\",\"data\":\"")
	suffix := []byte("\"}")
	return bytes.Join([][]byte{prefix, bytes.Repeat([]byte("a"), total-len(prefix)-len(suffix)), suffix}, nil)
}

func assertWorkspaceV2WireError(t *testing.T, err error, code dto.WorkspaceV2ErrorCode, closeCode uint16) *workspaceV2WireError {
	t.Helper()
	var wireErr *workspaceV2WireError
	require.ErrorAs(t, err, &wireErr)
	require.Equal(t, code, wireErr.Code)
	require.Equal(t, closeCode, wireErr.CloseCode)
	return wireErr
}
