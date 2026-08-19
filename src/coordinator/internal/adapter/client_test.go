package adapter

import (
	"net/http"
	"testing"
)

func TestCopyForwardedHeadersPreservesCurrentCodexMetadata(t *testing.T) {
	source := make(http.Header)
	for name, value := range map[string]string{
		"Accept": "text/event-stream", "Content-Encoding": "zstd",
		"Content-Type": "application/json", "Originator": "codex_exec",
		"Session-Id": "session-test", "Thread-Id": "thread-test",
		"User-Agent": "codex_exec/test", "X-Client-Request-Id": "request-test",
		"X-Codex-Beta-Features": "feature-test", "X-Codex-Turn-Metadata": "metadata-test",
		"X-Codex-Window-Id":                      "window-test",
		"X-OpenAI-Internal-Codex-Responses-Lite": "true",
	} {
		source.Set(name, value)
	}
	source.Set("Authorization", "Bearer downstream")
	source.Set("X-Forwarded-For", "203.0.113.1")
	destination := make(http.Header)

	copyForwardedHeaders(destination, source)

	for name, values := range source {
		if name == "Authorization" || name == "X-Forwarded-For" {
			if destination.Get(name) != "" {
				t.Fatalf("unsafe %s crossed", name)
			}
			continue
		}
		if got := destination.Values(name); len(got) != len(values) || got[0] != values[0] {
			t.Fatalf("forwarded %s = %v, want %v", name, got, values)
		}
	}
}
