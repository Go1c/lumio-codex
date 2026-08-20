package websocket_router

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"unicode/utf8"

	"github.com/haierkeys/fast-note-sync-service/internal/dto"
)

type workspaceV2DecodedRequest struct {
	action    dto.WorkspaceV2Action
	requestID dto.WorkspaceUUID
	data      any
}

type workspaceV2WireError struct {
	Action    string
	RequestID *dto.WorkspaceUUID
	Code      dto.WorkspaceV2ErrorCode
	Fields    []dto.WorkspaceV2FieldError
	CloseCode uint16
}

func (e *workspaceV2WireError) Error() string {
	if e == nil {
		return ""
	}
	if e.CloseCode != 0 {
		return fmt.Sprintf("workspace v2 close %d", e.CloseCode)
	}
	return string(e.Code)
}

func decodeWorkspaceV2ControlFrame(frame []byte) (*workspaceV2DecodedRequest, string, error) {
	if len(frame) > dto.WorkspaceMaxControlFrameBytes {
		return nil, "", &workspaceV2WireError{Code: dto.WorkspaceErrorInvalidFrame, CloseCode: 1009}
	}
	if !utf8.Valid(frame) {
		return nil, "", &workspaceV2WireError{Code: dto.WorkspaceErrorInvalidFrame, CloseCode: 1002}
	}
	separator := strings.IndexByte(string(frame), '|')
	if separator <= 0 {
		return nil, "", &workspaceV2WireError{Code: dto.WorkspaceErrorInvalidFrame, CloseCode: 1002}
	}
	actionToken := string(frame[:separator])
	if !workspaceV2ValidActionToken(actionToken) {
		return nil, "", &workspaceV2WireError{Code: dto.WorkspaceErrorInvalidFrame, CloseCode: 1002}
	}

	var envelope dto.WorkspaceV2Request[json.RawMessage]
	if err := dto.DecodeWorkspaceV2Request(frame[separator+1:], &envelope); err != nil {
		return nil, actionToken, workspaceV2WireValidationError(actionToken, nil, dto.WorkspaceErrorInvalidJSON, err)
	}
	requestID := envelope.RequestID
	data, err := dto.DecodeWorkspaceV2Data(
		dto.WorkspaceV2Action(actionToken),
		dto.WorkspaceFlowClientRequest,
		envelope.Data,
	)
	if err != nil {
		var validationErr *dto.WorkspaceValidationError
		if errors.As(err, &validationErr) && validationErr.Field == "action" && validationErr.Reason == "unknown_action" {
			return nil, actionToken, workspaceV2WireValidationError(actionToken, &requestID, dto.WorkspaceErrorUnknownAction, err)
		}
		if errors.As(err, &validationErr) && validationErr.Field == "flow" {
			return nil, actionToken, workspaceV2WireValidationError(actionToken, &requestID, dto.WorkspaceErrorInvalidRequest, err)
		}
		return nil, actionToken, workspaceV2WireValidationError(actionToken, &requestID, dto.WorkspaceErrorInvalidJSON, err)
	}
	return &workspaceV2DecodedRequest{
		action:    dto.WorkspaceV2Action(actionToken),
		requestID: requestID,
		data:      data,
	}, actionToken, nil
}

func encodeWorkspaceV2Failure(
	action dto.WorkspaceV2Action,
	requestID *dto.WorkspaceUUID,
	code dto.WorkspaceV2ErrorCode,
	fields ...dto.WorkspaceV2FieldError,
) ([]byte, error) {
	errorValue := dto.NewWorkspaceV2Error(code, fields...)
	return dto.EncodeWorkspaceV2Response[struct{}](action, dto.WorkspaceV2Response[struct{}]{
		RequestID: requestID,
		Status:    false,
		Error:     &errorValue,
	})
}

func encodeWorkspaceV2WireError(action string, wireErr *workspaceV2WireError) ([]byte, error) {
	if wireErr == nil || wireErr.CloseCode != 0 {
		return nil, errors.New("workspace v2 wire error is not encodable")
	}
	if wireErr.Code == dto.WorkspaceErrorUnknownAction {
		return dto.EncodeWorkspaceV2UnknownActionFailure(action, wireErr.RequestID)
	}
	return encodeWorkspaceV2Failure(dto.WorkspaceV2Action(action), wireErr.RequestID, wireErr.Code, wireErr.Fields...)
}

func encodeWorkspaceV2Success[T any](action dto.WorkspaceV2Action, requestID dto.WorkspaceUUID, data *T) ([]byte, error) {
	if err := workspaceV2ValidateData(data); err != nil {
		return nil, err
	}
	return dto.EncodeWorkspaceV2Response(action, dto.WorkspaceV2Response[T]{
		RequestID: &requestID,
		Status:    true,
		Data:      data,
	})
}

func encodeWorkspaceV2Push[T any](action dto.WorkspaceV2Action, data *T) ([]byte, error) {
	if err := workspaceV2ValidateData(data); err != nil {
		return nil, err
	}
	return dto.EncodeWorkspaceV2Response(action, dto.WorkspaceV2Response[T]{
		Status: true,
		Data:   data,
	})
}

func workspaceV2ValidateData[T any](data *T) error {
	if data == nil {
		return errors.New("workspace v2 response data is nil")
	}
	if validatable, ok := any(data).(interface{ Validate() error }); ok {
		return validatable.Validate()
	}
	return nil
}

func workspaceV2WireValidationError(
	action string,
	requestID *dto.WorkspaceUUID,
	code dto.WorkspaceV2ErrorCode,
	err error,
) *workspaceV2WireError {
	wireErr := &workspaceV2WireError{Action: action, RequestID: requestID, Code: code}
	var validationErr *dto.WorkspaceValidationError
	if errors.As(err, &validationErr) {
		wireErr.Fields = []dto.WorkspaceV2FieldError{{Field: validationErr.Field, Reason: validationErr.Reason}}
	}
	return wireErr
}

func workspaceV2ValidActionToken(action string) bool {
	if len(action) == 0 || len(action) > 64 || !workspaceV2ASCIIAlpha(action[0]) {
		return false
	}
	for i := 1; i < len(action); i++ {
		if !workspaceV2ASCIIAlpha(action[i]) && (action[i] < '0' || action[i] > '9') {
			return false
		}
	}
	return true
}

func workspaceV2ASCIIAlpha(value byte) bool {
	return value >= 'A' && value <= 'Z' || value >= 'a' && value <= 'z'
}
