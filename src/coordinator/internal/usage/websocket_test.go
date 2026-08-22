package usage

import "testing"

func TestParseWebSocketTerminalEvent(t *testing.T) {
	event, ok := ParseWebSocketEvent([]byte(`{
        "type":"response.completed",
        "response":{"usage":{
            "input_tokens":11,
            "input_tokens_details":{"cached_tokens":4,"cache_write_tokens":2},
            "output_tokens":5,
            "output_tokens_details":{"reasoning_tokens":3},
            "total_tokens":16
        }}
    }`))
	if !ok || event.Type != "response.completed" || event.Usage == nil ||
		event.Usage.TotalTokens != 16 || event.Usage.CachedInputTokens != 4 ||
		event.Usage.CacheWriteInputTokens != 2 || event.Usage.ReasoningOutputTokens != 3 {
		t.Fatalf("event = %#v, %v", event, ok)
	}
}

func TestParseWebSocketEventRejectsInvalidTypeOrUsage(t *testing.T) {
	for _, data := range [][]byte{[]byte(`not-json`), []byte(`{}`)} {
		if _, ok := ParseWebSocketEvent(data); ok {
			t.Fatalf("accepted %q", data)
		}
	}
	event, ok := ParseWebSocketEvent([]byte(
		`{"type":"response.completed","usage":{"input_tokens":-1}}`,
	))
	if !ok || event.Usage != nil {
		t.Fatalf("negative usage event = %#v, %v", event, ok)
	}
}
