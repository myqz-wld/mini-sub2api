package httpapi

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"

	"github.com/coder/websocket"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

const maxWebSocketCloseReasonBytes = 123

func parseCoreFailureClose(err error) (protocolv1.FailureMetadata, bool) {
	var closeError websocket.CloseError
	if !errors.As(err, &closeError) || closeError.Code != websocket.StatusCode(protocolv1.FailureCloseCode) ||
		len(closeError.Reason) > maxWebSocketCloseReasonBytes {
		return protocolv1.FailureMetadata{}, false
	}
	decoder := json.NewDecoder(bytes.NewBufferString(closeError.Reason))
	decoder.DisallowUnknownFields()
	var metadata protocolv1.FailureMetadata
	if decoder.Decode(&metadata) != nil || !metadata.Valid() {
		return protocolv1.FailureMetadata{}, false
	}
	if err := decoder.Decode(&struct{}{}); err != io.EOF {
		return protocolv1.FailureMetadata{}, false
	}
	return metadata, true
}

func failureCloseReason(metadata protocolv1.FailureMetadata) (string, bool) {
	if !metadata.Valid() {
		return "", false
	}
	encoded, err := json.Marshal(metadata)
	if err != nil || len(encoded) > maxWebSocketCloseReasonBytes {
		return "", false
	}
	return string(encoded), true
}

func passthroughCoreClose(err error) (websocket.StatusCode, bool) {
	code := websocket.CloseStatus(err)
	switch code {
	case websocket.StatusNormalClosure, websocket.StatusGoingAway, websocket.StatusProtocolError,
		websocket.StatusUnsupportedData, websocket.StatusPolicyViolation, websocket.StatusMessageTooBig,
		websocket.StatusServiceRestart:
		return code, true
	default:
		return 0, false
	}
}
