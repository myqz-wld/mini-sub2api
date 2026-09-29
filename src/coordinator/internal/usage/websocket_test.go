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

func TestWebSocketErrorsDoNotDependOnStatusOrOptionalUsage(t *testing.T) {
	for _, raw := range []string{
		`{"type":"error","status":400,"error":{"code":"invalid_prompt"}}`,
		`{"type":"error","status":429,"error":{"code":"flex_unavailable"}}`,
		`{"type":"error","status":400,"usage":{"total_tokens":"invalid"}}`,
		`{"type":"error","status":400,"response":"optional-provider-extension"}`,
	} {
		event, ok := ParseWebSocketEvent([]byte(raw))
		if !ok || event.Type != "error" || event.Usage != nil {
			t.Fatal("valid error event was rejected by optional fields")
		}
	}
	event, ok := ParseWebSocketEvent([]byte(`{"type":"error","status":400,"usage":{"total_tokens":7}}`))
	if !ok || event.Usage == nil || event.Usage.TotalTokens != 7 {
		t.Fatal("numeric error status prevented valid usage extraction")
	}
	observer := NewObserver("application/json")
	observer.Observe([]byte(`{"type":"error","status":400}`))
	if observer.TerminalStatus() != TerminalUpstreamError {
		t.Fatal("numeric error status prevented HTTP error observation")
	}
}
