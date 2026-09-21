package integration

import (
	"bytes"
	"context"
	"io"
	"net/http"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestResponsesHTTPClosesCompletedUpstreamAndValidatesItsTail(t *testing.T) {
	const completed = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_lifetime\",\"output\":[],\"usage\":{\"total_tokens\":7}}}\n\n"
	const failed = "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_lifetime\",\"status\":\"failed\",\"output\":[],\"usage\":{\"total_tokens\":7}}}\n\n"
	for _, test := range []struct {
		name                                         string
		streaming, apiKey, failedFooter, invalidTail bool
	}{
		{name: "subscription_sse", streaming: true},
		{name: "subscription_json"},
		{name: "api_key_sse", streaming: true, apiKey: true},
		{name: "delayed_failed_footer", streaming: true, failedFooter: true},
		{name: "sse_late_invalid", streaming: true, invalidTail: true},
		{name: "json_late_invalid", invalidTail: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			upstreamClosed := make(chan struct{})
			fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(writer http.ResponseWriter, _ []byte) (string, string) {
				defer close(upstreamClosed)
				writer.Header().Set("Content-Type", "text/event-stream")
				first := completed
				if test.failedFooter {
					first = "data: {\"type\":\"error\",\"code\":\"synthetic\"}\n\n"
				}
				_, _ = io.WriteString(writer, first)
				controller := http.NewResponseController(writer)
				_ = controller.Flush()
				if test.failedFooter || test.invalidTail {
					time.Sleep(75 * time.Millisecond)
					tail := failed
					if test.invalidTail {
						tail = "data: invalid\n\n"
					}
					_, _ = io.WriteString(writer, tail)
					_ = controller.Flush()
				}
				// An upstream that never sends EOF; bounded only to make fixture failure safe.
				ticker := time.NewTicker(10 * time.Millisecond)
				defer ticker.Stop()
				deadline := time.NewTimer(6 * time.Second)
				defer deadline.Stop()
				for {
					select {
					case <-ticker.C:
						if _, err := io.WriteString(writer, ": heartbeat\n\n"); err != nil {
							return "resp_lifetime", ""
						}
						if err := controller.Flush(); err != nil {
							return "resp_lifetime", ""
						}
					case <-deadline.C:
						return "resp_lifetime", ""
					}
				}
			})
			key, keyID := fixture.subscriptionKey, fixture.subscriptionKeyID
			if test.apiKey {
				key, keyID = fixture.apiKey, fixture.apiKeyID
			}
			payload := mustRequestJSON(t, map[string]any{"model": "gpt-5.4", "stream": test.streaming, "input": "synthetic lifetime probe"})
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			request, _ := http.NewRequestWithContext(ctx, http.MethodPost, fixture.public.URL+"/v1/responses", bytes.NewReader(payload))
			request.Header.Set("Authorization", "Bearer "+key)
			request.Header.Set("Content-Type", "application/json")
			started := time.Now()
			response, err := fixture.public.Client().Do(request)
			if err != nil {
				t.Fatal("public lifetime request failed")
			}
			defer response.Body.Close()
			body, err := io.ReadAll(response.Body)
			if err != nil {
				t.Fatal("public response did not close cleanly")
			}
			if elapsed := time.Since(started); elapsed > 4*time.Second {
				t.Fatalf("response took %v without waiting for provider EOF", elapsed)
			}
			select {
			case <-upstreamClosed:
			case <-time.After(time.Second):
				t.Fatal("closing the public response retained its upstream connection")
			}
			status := http.StatusOK
			if test.invalidTail && !test.streaming {
				status = http.StatusBadGateway
			}
			if response.StatusCode != status {
				t.Fatalf("status = %d, want %d", response.StatusCode, status)
			}
			if test.apiKey && !bytes.HasPrefix(body, []byte(completed)) {
				t.Fatal("API key SSE bytes changed")
			}
			if test.failedFooter && !bytes.Contains(body, []byte(`"type":"response.failed"`)) {
				t.Fatal("bounded tail lost a valid delayed failure footer")
			}
			if test.invalidTail && test.streaming && (response.Trailer.Get(protocolv1.FailurePhaseTrailer) != "upstream_stream" || response.Trailer.Get(protocolv1.RetryAdviceTrailer) != "never") {
				t.Fatal("late invalid data lost no-replay failure metadata")
			}
			record := waitForProfileRequestRecord(t, fixture.store, keyID, response.Header.Get("X-Mini-Sub2Api-Request-Id"))
			want := storage.RequestCompleted
			if test.failedFooter || test.invalidTail {
				want = storage.RequestUpstreamErr
			}
			if record.Status != want {
				t.Fatalf("history status = %s, want %s", record.Status, want)
			}
		})
	}
}
