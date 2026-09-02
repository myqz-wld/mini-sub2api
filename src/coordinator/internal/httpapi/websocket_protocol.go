package httpapi

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

const maxHandshakeRejectionBytes = 64 * 1024

type clientApplicationEvent struct {
	eventType     string
	operationKind string
}

func parseProviderRequestIDControl(payload []byte) (string, bool, bool) {
	var envelope struct {
		Type string `json:"type"`
	}
	if json.Unmarshal(payload, &envelope) != nil || envelope.Type != protocolv1.ProviderRequestIDEventType {
		return "", false, false
	}
	decoder := json.NewDecoder(bytes.NewReader(payload))
	decoder.DisallowUnknownFields()
	var control protocolv1.ProviderRequestIDControl
	if decoder.Decode(&control) != nil || control.Type != protocolv1.ProviderRequestIDEventType ||
		!validProviderRequestID(control.ProviderRequestID) {
		return "", true, false
	}
	if err := decoder.Decode(&struct{}{}); err != io.EOF {
		return "", true, false
	}
	return control.ProviderRequestID, true, true
}

func parseClientApplicationEvent(payload []byte) (clientApplicationEvent, bool) {
	var object map[string]json.RawMessage
	if err := json.Unmarshal(payload, &object); err != nil {
		return clientApplicationEvent{}, false
	}
	var eventType string
	if err := json.Unmarshal(object["type"], &eventType); err != nil || eventType == "" {
		return clientApplicationEvent{}, false
	}
	event := clientApplicationEvent{eventType: eventType}
	if eventType != "response.create" {
		return event, true
	}
	event.operationKind = storage.OperationInference
	if raw, exists := object["generate"]; exists {
		var generate bool
		if err := json.Unmarshal(raw, &generate); err != nil {
			return clientApplicationEvent{}, false
		}
		if !generate {
			event.operationKind = storage.OperationWebSocketPrewarm
		}
	}
	return event, true
}

func validateWebSocketHandshake(request *http.Request) (int, string, string) {
	if !headerContainsToken(request.Header, "Connection", "upgrade") ||
		!headerContainsToken(request.Header, "Upgrade", "websocket") {
		return http.StatusUpgradeRequired, "websocket_required", "A WebSocket upgrade is required."
	}
	if request.Header.Get("Sec-WebSocket-Version") != "13" {
		return http.StatusBadRequest, "invalid_websocket", "The WebSocket handshake is invalid."
	}
	keys := request.Header.Values("Sec-WebSocket-Key")
	if len(keys) != 1 {
		return http.StatusBadRequest, "invalid_websocket", "The WebSocket handshake is invalid."
	}
	decoded, err := base64.StdEncoding.DecodeString(strings.TrimSpace(keys[0]))
	if err != nil || len(decoded) != 16 {
		return http.StatusBadRequest, "invalid_websocket", "The WebSocket handshake is invalid."
	}
	if origin := request.Header.Get("Origin"); origin != "" {
		parsed, err := url.Parse(origin)
		if err != nil || parsed.Host == "" || !strings.EqualFold(parsed.Host, request.Host) {
			return http.StatusForbidden, "invalid_origin", "The WebSocket origin is not authorized."
		}
	}
	return 0, "", ""
}

func headerContainsToken(headers http.Header, name, token string) bool {
	for _, value := range headers.Values(name) {
		for _, candidate := range strings.Split(value, ",") {
			if strings.EqualFold(strings.TrimSpace(candidate), token) {
				return true
			}
		}
	}
	return false
}

func writeInvalidWebSocketHandshake(
	writer http.ResponseWriter,
	status int,
	code, message, requestID string,
) {
	if status == http.StatusUpgradeRequired {
		writer.Header().Set("Upgrade", "websocket")
		writer.Header().Set("Sec-WebSocket-Version", "13")
	}
	writeOpenAIError(writer, status, code, message, requestID)
}

func copyWebSocketUpgradeHeaders(
	destination, source http.Header,
	gatewayRequestID string,
) *time.Duration {
	ttfb := copyResponseHeaders(destination, source, gatewayRequestID)
	destination.Del("Content-Type")
	destination.Del("Content-Encoding")
	return ttfb
}

func writeWebSocketHandshakeRejection(
	writer http.ResponseWriter,
	response *http.Response,
	requestID string,
) {
	data, err := io.ReadAll(io.LimitReader(response.Body, maxHandshakeRejectionBytes+1))
	contentType := response.Header.Get("Content-Type")
	preserveBody := err == nil && len(data) <= maxHandshakeRejectionBytes &&
		(strings.HasPrefix(strings.ToLower(contentType), "application/json") ||
			strings.HasPrefix(strings.ToLower(contentType), "text/"))
	copyResponseHeaders(writer.Header(), response.Header, requestID)
	if !preserveBody {
		writer.Header().Del("Content-Type")
		writer.Header().Del("Content-Encoding")
		writeOpenAIError(
			writer, response.StatusCode, "upstream_rejected",
			"The upstream service rejected the WebSocket handshake.", requestID,
		)
		return
	}
	writer.Header().Set("Content-Type", contentType)
	if response.StatusCode == http.StatusUpgradeRequired {
		writer.Header().Set("Upgrade", "websocket")
	}
	writer.Header().Set("X-Mini-Sub2Api-Request-Id", requestID)
	writer.WriteHeader(response.StatusCode)
	_, _ = writer.Write(data)
}
