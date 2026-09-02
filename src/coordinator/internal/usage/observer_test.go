package usage

import (
	"testing"
)

func TestSSEUsageAcrossArbitraryChunks(t *testing.T) {
	observer := NewObserver("text/event-stream; charset=utf-8")
	body := "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n" +
		"event: response.completed\r\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{" +
		"\"input_tokens\":10,\"input_tokens_details\":{\"cached_tokens\":3,\"cache_write_tokens\":2}," +
		"\"output_tokens\":7,\"output_tokens_details\":{\"reasoning_tokens\":4},\"total_tokens\":17}}}\r\n\r\n"
	for index := 0; index < len(body); {
		next := index + 7
		if next > len(body) {
			next = len(body)
		}
		observer.Observe([]byte(body[index:next]))
		index = next
	}
	got := observer.Usage()
	if got == nil {
		t.Fatal("usage not observed")
	}
	if got.InputTokens != 10 || got.CachedInputTokens != 3 || got.CacheWriteInputTokens != 2 ||
		got.OutputTokens != 7 || got.ReasoningOutputTokens != 4 || got.TotalTokens != 17 {
		t.Fatalf("usage = %#v", got)
	}
}

func TestNonStreamingUsage(t *testing.T) {
	observer := NewObserver("application/json")
	observer.Observe([]byte(`{"id":"resp_test","usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}`))
	got := observer.Usage()
	if got == nil || got.TotalTokens != 5 {
		t.Fatalf("usage = %#v", got)
	}
}

func TestFinalSSEEventWithoutBlankLineIsObservedAtEOF(t *testing.T) {
	observer := NewObserver("text/event-stream")
	observer.Observe([]byte(`data: {"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}`))
	got := observer.Usage()
	if got == nil || got.TotalTokens != 2 {
		t.Fatalf("usage = %#v", got)
	}
	if observer.TerminalStatus() != TerminalCompleted {
		t.Fatalf("terminal = %v", observer.TerminalStatus())
	}
}

func TestTerminalFailuresAreObservedForSSEAndJSON(t *testing.T) {
	for _, body := range []string{
		`data: {"type":"response.failed","response":{"usage":{"total_tokens":2}}}` + "\n\n",
		`data: {"type":"response.incomplete","response":{}}`,
		`data: {"type":"error","error":{"message":"opaque"}}` + "\n\n",
	} {
		observer := NewObserver("text/event-stream")
		observer.Observe([]byte(body))
		if observer.TerminalStatus() != TerminalUpstreamError {
			t.Fatalf("SSE terminal for %q = %v", body, observer.TerminalStatus())
		}
	}
	for _, status := range []string{"failed", "incomplete"} {
		observer := NewObserver("application/json")
		observer.Observe([]byte(`{"status":"` + status + `"}`))
		if observer.TerminalStatus() != TerminalUpstreamError {
			t.Fatalf("JSON terminal %s = %v", status, observer.TerminalStatus())
		}
	}
}

func TestSSEUsageIsDetectedWhenUpstreamOmitsContentType(t *testing.T) {
	observer := NewObserver("")
	observer.Observe([]byte("event: response.completed\n"))
	observer.Observe([]byte("data: {\"type\":\"response.completed\",\"response\":{\"usage\":{" +
		"\"input_tokens\":4,\"output_tokens\":2,\"total_tokens\":6}}}\n\n"))
	got := observer.Usage()
	if got == nil || got.InputTokens != 4 || got.OutputTokens != 2 || got.TotalTokens != 6 {
		t.Fatalf("usage = %#v", got)
	}
}

func TestOversizedOrNegativeUsageIsIgnored(t *testing.T) {
	observer := NewObserver("application/json")
	observer.Observe(make([]byte, maxObservedEventBytes+1))
	if got := observer.Usage(); got != nil {
		t.Fatalf("oversized usage = %#v", got)
	}
	observer = NewObserver("application/json")
	observer.Observe([]byte(`{"usage":{"input_tokens":-1,"output_tokens":2,"total_tokens":1}}`))
	if got := observer.Usage(); got != nil {
		t.Fatalf("negative usage = %#v", got)
	}
}
