package usage

import "testing"

func TestStreamProgressDoesNotFlushOrCountHeartbeatAndPartialData(t *testing.T) {
	observer := NewObserver("text/event-stream")
	for _, chunk := range []string{": heartbeat\r\n\r\n", "data: \n\n", "data: [DONE]\n\n", "data: {\"type\":\"response.completed\"}"} {
		observer.Observe([]byte(chunk))
		if events, terminal := observer.StreamProgress(); events != 0 || terminal != TerminalUnknown {
			t.Fatalf("partial/heartbeat counted as an event: %d, %v", events, terminal)
		}
	}
	if !observer.HasPendingSSEData() {
		t.Fatal("unframed completion was lost")
	}
	observer.Observe([]byte("\n\n"))
	if events, terminal := observer.StreamProgress(); events != 1 || terminal != TerminalCompleted {
		t.Fatalf("framed completion not observed: %d, %v", events, terminal)
	}
	if observer.HasPendingSSEData() {
		t.Fatal("complete event retained a pending fragment")
	}
	observer.Observe([]byte("data: {\"type\":\"error\"}\n\n"))
	if events, terminal := observer.StreamProgress(); events != 2 || terminal != TerminalUpstreamError {
		t.Fatalf("late error was lost: %d, %v", events, terminal)
	}
}

func TestPendingSSEDataDistinguishesCommentsFromTruncatedEvents(t *testing.T) {
	for _, test := range []struct {
		input   string
		pending bool
	}{
		{": heartbeat", false}, {"data: [DONE]", false}, {"data: ", false},
		{"data: {broken", true}, {"data: {}", true},
	} {
		observer := NewObserver("text/event-stream")
		observer.Observe([]byte(test.input))
		if observer.HasPendingSSEData() != test.pending {
			t.Fatalf("pending data for %q = %t", test.input, !test.pending)
		}
	}
}
