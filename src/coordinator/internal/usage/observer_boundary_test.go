package usage

import (
	"strings"
	"testing"
)

func TestLargeResponseObservation(t *testing.T) {
	padding := strings.Repeat("x", 9*1024*1024)
	for _, format := range []string{"application/json", "text/event-stream"} {
		for _, status := range []string{"completed", "failed", "incomplete"} {
			t.Run(format+"/"+status, func(t *testing.T) {
				body := `{"status":"` + status + `","output":[{"text":"` + padding + `"}],"usage":{"total_tokens":17}}`
				if format == "text/event-stream" {
					body = `data: {"type":"response.` + status + `","response":` + body + "}\r\n\r\n"
				}
				o := NewObserver(format)
				for start := 0; start < len(body); start += 32768 {
					o.Observe([]byte(body[start:min(start+32768, len(body))]))
				}
				expected := TerminalUpstreamError
				if status == "completed" {
					expected = TerminalCompleted
				}
				if o.TerminalStatus() != expected || o.Usage() == nil || o.Usage().TotalTokens != 17 {
					t.Fatal("large response terminal or usage was lost")
				}
			})
		}
	}
}

func TestTerminalFailureCannotBeOverwritten(t *testing.T) {
	o := NewObserver("text/event-stream")
	o.Observe([]byte("data: {\"type\":\"response.failed\"}\n\ndata: {\"type\":\"response.completed\"}\n\n"))
	if o.TerminalStatus() != TerminalUpstreamError {
		t.Fatal("later success hid an observed failure")
	}
}

func TestObserverResumesAfterOversizedEvent(t *testing.T) {
	for _, ending := range []string{"\n\n", "\r\n\r\n", "\r\n\n"} {
		for _, chunkSize := range []int{1, 7, 4096} {
			o := NewObserver("text/event-stream")
			o.maximum = 256
			body := `data: {"type":"response.output_text.delta","delta":"` + strings.Repeat("x", 1024) + `"}` + ending +
				`data: {"type":"response.failed","response":{"usage":{"total_tokens":13}}}` + ending
			for start := 0; start < len(body); start += chunkSize {
				o.Observe([]byte(body[start:min(start+chunkSize, len(body))]))
				if len(o.buffer) > o.maximum {
					t.Fatal("observer exceeded its budget")
				}
			}
			if o.TerminalStatus() != TerminalUpstreamError || o.Usage() == nil || o.Usage().TotalTokens != 13 {
				t.Fatal("oversized event hid a later terminal")
			}
		}
	}
}

func TestManyEventsInOneChunkUsePerEventBudget(t *testing.T) {
	o := NewObserver("text/event-stream")
	o.maximum = 128
	body := strings.Repeat("data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n", 32)
	body += "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"total_tokens\":9}}}\n\n"
	o.Observe([]byte(body))
	if o.TerminalStatus() != TerminalCompleted || o.Usage() == nil || o.Usage().TotalTokens != 9 {
		t.Fatal("a large read containing small events lost its terminal")
	}
}
