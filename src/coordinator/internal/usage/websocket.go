package usage

import (
	"encoding/json"

	"mini-sub2api/src/coordinator/internal/storage"
)

type WebSocketEvent struct {
	Type                string
	Usage               *storage.TokenUsage
	ResponseID          string
	ExpectsFailedFooter bool
}

func ParseWebSocketEvent(data []byte) (WebSocketEvent, bool) {
	// Event admission must not depend on optional usage or response-status fields.
	// Wrapped native errors use a numeric status; ordinary response status is text.
	var envelope struct {
		Type       string          `json:"type"`
		Response   json.RawMessage `json:"response"`
		ResponseID json.RawMessage `json:"response_id"`
		Error      json.RawMessage `json:"error"`
		Code       json.RawMessage `json:"code"`
		Status     json.RawMessage `json:"status"`
		StatusCode json.RawMessage `json:"status_code"`
	}
	if err := json.Unmarshal(data, &envelope); err != nil || envelope.Type == "" {
		return WebSocketEvent{}, false
	}
	event := WebSocketEvent{Type: envelope.Type}
	var response struct {
		ID json.RawMessage `json:"id"`
	}
	_ = json.Unmarshal(envelope.Response, &response)
	id := response.ID
	if len(id) == 0 {
		id = envelope.ResponseID
	}
	_ = json.Unmarshal(id, &event.ResponseID)
	if len(event.ResponseID) > 512 {
		event.ResponseID = ""
	}
	if event.Type == "error" {
		var failure struct {
			Code json.RawMessage `json:"code"`
		}
		_ = json.Unmarshal(envelope.Error, &failure)
		code := failure.Code
		if len(code) == 0 {
			code = envelope.Code
		}
		var text string
		_ = json.Unmarshal(code, &text)
		var status, statusCode uint16
		_ = json.Unmarshal(envelope.Status, &status)
		_ = json.Unmarshal(envelope.StatusCode, &statusCode)
		event.ExpectsFailedFooter = text != "flex_unavailable" &&
			!(status >= 100 && status <= 599) && !(statusCode >= 100 && statusCode <= 599)
	}
	if parsed, ok := parseUsage(data); ok {
		event.Usage = &parsed
	}
	return event, true
}
