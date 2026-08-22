package usage

import (
	"encoding/json"

	"mini-sub2api/src/coordinator/internal/storage"
)

type WebSocketEvent struct {
	Type  string
	Usage *storage.TokenUsage
}

func ParseWebSocketEvent(data []byte) (WebSocketEvent, bool) {
	var envelope responseEnvelope
	if err := json.Unmarshal(data, &envelope); err != nil || envelope.Type == "" {
		return WebSocketEvent{}, false
	}
	event := WebSocketEvent{Type: envelope.Type}
	if parsed, ok := parseUsage(data); ok {
		event.Usage = &parsed
	}
	return event, true
}
