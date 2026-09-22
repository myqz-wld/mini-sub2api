package httpapi

import (
	"context"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestHTTPStatusTrafficCannotRenewOutputDeadlines(t *testing.T) {
	for _, repeated := range []string{
		`{"type":"response.metadata"}`,
		`{"type":"response.in_progress"}`,
		`{"type":"synthetic.private.event","delta":"synthetic-payload"}`,
		`{"type":"response.output_text.delta","delta":""}`,
	} {
		for _, initialOutput := range []bool{false, true} {
			prefix, want := "", streamStopFirstOutput
			if initialOutput {
				prefix, want = "data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"synthetic-payload\"}\n\n", streamStopOutputIdle
			}
			body, abort := endlessHTTPBody(t, prefix, "data: "+repeated+"\n\n")
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			_, outcome, reason, diagnostics := streamBody(httptest.NewRecorder(), body, "text/event-stream", ctx, shortHTTPTimeouts(), abort)
			cancel()
			if outcome != streamUpstreamError || reason != want {
				t.Fatalf("output=%t event=%s outcome/reason=%v/%s", initialOutput, repeated, outcome, reason)
			}
			if diagnostics.progress.Events < 2 || (diagnostics.firstOutput >= 0) != initialOutput {
				t.Fatalf("incorrect observations: %s", diagnostics)
			}
			if text := diagnostics.String(); strings.Contains(text, "synthetic") || strings.Contains(text, "response.") {
				t.Fatalf("diagnostics exposed event data: %s", text)
			}
		}
	}
}

func TestHTTPOutputClassesAndNonSSEBytesKeepMakingProgress(t *testing.T) {
	for _, test := range []struct{ contentType, chunk string }{
		{"text/event-stream", `data: {"type":"response.reasoning_summary_text.delta","delta":"x"}` + "\n\n"},
		{"text/event-stream", `data: {"type":"response.function_call_arguments.delta","delta":"{"}` + "\n\n"},
		{"text/event-stream", `data: {"type":"response.output_audio.delta","delta":"x"}` + "\n\n"},
		{"text/event-stream", `data: {"type":"response.output_item.done","item":{"type":"message"}}` + "\n\n"},
		{"", `data: {"type":"response.output_text.delta","delta":"x"}` + "\n\n"},
		{"application/octet-stream", "synthetic-bytes"},
	} {
		body, abort := endlessHTTPBody(t, "", test.chunk)
		ctx, cancel := context.WithTimeout(context.Background(), 250*time.Millisecond)
		_, outcome, reason, diagnostics := streamBody(httptest.NewRecorder(), body, test.contentType, ctx, shortHTTPTimeouts(), abort)
		cancel()
		if outcome != streamClientDisconnected || reason != streamStopCanceled {
			t.Fatalf("progress aborted early: %v/%s %s", outcome, reason, diagnostics)
		}
	}
}

func TestHTTPForcedCloseDoesNotCountUnframedOutput(t *testing.T) {
	body, abort := endlessHTTPBody(t, `data: {"type":"response.output_text.delta","delta":"synthetic"}`, " ")
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	_, outcome, reason, diagnostics := streamBody(httptest.NewRecorder(), body, "text/event-stream", ctx, shortHTTPTimeouts(), abort)
	if outcome != streamUpstreamError || reason != streamStopIdle || diagnostics.progress.Outputs != 0 || diagnostics.firstOutput != -1 {
		t.Fatalf("unframed data counted as output: %v/%s %s", outcome, reason, diagnostics)
	}
}
