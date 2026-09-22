package httpapi

import (
	"bytes"
	"context"
	"io"
	"log"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestHTTPLifetimeFinalizesHistoryAndCancelsForwardContext(t *testing.T) {
	for _, test := range []struct {
		name, contentType, prefix, repeat string
		upstreamStatus, publicStatus      int
		status, reason                    string
	}{
		{"idle_sse", "text/event-stream", "", ": synthetic-private-body\n\n", 200, 200, storage.RequestUpstreamErr, "event_idle_timeout"},
		{"first_output", "text/event-stream", "", "data: {\"type\":\"synthetic-private-body\",\"delta\":\"synthetic-private-body\"}\n\n", 200, 200, storage.RequestUpstreamErr, "first_output_timeout"},
		{"output_idle", "text/event-stream", "data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic-private-body\"}\n\n", "data: {\"type\":\"response.in_progress\"}\n\n", 200, 200, storage.RequestUpstreamErr, "output_idle_timeout"},
		{"terminal_open", "text/event-stream", "data: {\"type\":\"response.completed\"}\n\n", ": synthetic-private-body\n\n", 200, 200, storage.RequestCompleted, "terminal_tail_closed"},
		{"error_sse", "text/event-stream", "", ": synthetic-private-body\n\n", 503, 503, storage.RequestUpstreamErr, "event_idle_timeout"},
		{"error_json", "application/json", "{\"error\":\"synthetic-private-body\"", " ", 503, 502, storage.RequestUpstreamErr, "event_idle_timeout"},
	} {
		t.Run(test.name, func(t *testing.T) {
			store, _, key := setupHTTPTest(t)
			body, _ := endlessHTTPBody(t, test.prefix, test.repeat)
			var forwardContext context.Context
			core := &fakeCore{response: func(ctx context.Context, _ string) (*http.Response, error) {
				forwardContext = ctx
				return &http.Response{StatusCode: test.upstreamStatus, Header: http.Header{"Content-Type": []string{test.contentType}}, Body: body}, nil
			}}
			var logs bytes.Buffer
			handler := NewHandler(store, core, log.New(&logs, "", 0))
			handler.httpTimeouts = shortHTTPTimeouts()
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			defer cancel()
			request := httptest.NewRequest(http.MethodPost, "/v1/responses", strings.NewReader(`{"stream":true}`)).WithContext(ctx)
			request.Header.Set("Authorization", "Bearer "+key.Secret)
			writer := httptest.NewRecorder()
			handler.ServeHTTP(writer, request)
			response := writer.Result()
			defer response.Body.Close()
			_, _ = io.ReadAll(response.Body)
			if response.StatusCode != test.publicStatus {
				t.Fatalf("public status = %d", response.StatusCode)
			}
			records := waitForHistory(t, store, key.ID, 1)
			if records[0].Status != test.status {
				t.Fatalf("history status = %s, want %s", records[0].Status, test.status)
			}
			if ctx.Err() != nil || forwardContext.Err() == nil {
				t.Fatal("upstream cancellation was not scoped to the forwarded request")
			}
			if !strings.Contains(logs.String(), test.reason) || strings.Contains(logs.String(), "synthetic-private-body") || strings.Contains(logs.String(), key.Secret) {
				t.Fatal("timeout diagnostics missing or contain private payloads")
			}
			if (test.name == "idle_sse" || test.name == "first_output" || test.name == "output_idle") && (response.Trailer.Get(protocolv1.FailurePhaseTrailer) != "upstream_stream" || response.Trailer.Get(protocolv1.RetryAdviceTrailer) != "never") {
				t.Fatal("idle failure lost no-replay metadata")
			}
			if test.name != "error_json" && (!strings.Contains(logs.String(), "first_output_ms=") || !strings.Contains(logs.String(), "events=")) {
				t.Fatal("stream timing/count summary missing")
			}
		})
	}
}
