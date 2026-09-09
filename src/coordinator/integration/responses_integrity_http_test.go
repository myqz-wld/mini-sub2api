package integration

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestResponsesIntegrityHTTP(t *testing.T) {
	item := map[string]any{"type": "message", "id": "msg_integrity", "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": "verified prefix"}}}
	done := map[string]any{"type": "response.output_item.done", "output_index": 0, "item": item}
	completed := map[string]any{"type": "response.completed", "response": map[string]any{"id": "resp_integrity", "output": []any{item}}}
	errorEvent := map[string]any{"type": "error", "error": map[string]any{"code": "synthetic"}}
	eventBytes := func(events ...any) []byte {
		var out []byte
		for _, event := range events {
			out = append(out, []byte("data: ")...)
			out = append(out, mustRequestJSON(t, event)...)
			out = append(out, '\n', '\n')
		}
		return out
	}
	for _, scenario := range []string{"error-completed", "error-incomplete", "conflict", "malformed", "completed-then-invalid", "valid"} {
		for _, streaming := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/stream=%t", scenario, streaming), func(t *testing.T) {
				body := eventBytes(done, completed)
				switch scenario {
				case "error-completed":
					body = eventBytes(errorEvent, completed)
				case "error-incomplete":
					body = eventBytes(errorEvent, map[string]any{"type": "response.incomplete", "response": map[string]any{"id": "resp_integrity", "output": []any{item}}})
				case "conflict":
					body = eventBytes(done, map[string]any{"type": "response.completed", "response": map[string]any{"id": "resp_integrity", "output": []any{map[string]any{"type": "message", "id": "msg_integrity", "role": "assistant", "content": []any{}}}}})
				case "malformed":
					body = eventBytes(done, map[string]any{"type": "response.completed", "response": map[string]any{"id": "resp_integrity", "output": map[string]any{}}})
				case "completed-then-invalid":
					body = append(body, []byte("data: invalid\n\n")...)
				}
				fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
					w.Header().Set("Content-Type", "text/event-stream")
					_, _ = w.Write(body)
					return "resp_integrity", ""
				})
				payload := mustRequestJSON(t, map[string]any{"model": "gpt-5.4", "stream": streaming, "input": "integrity probe"})
				ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
				defer cancel()
				req, _ := http.NewRequestWithContext(ctx, http.MethodPost, fixture.public.URL+"/v1/responses", bytes.NewReader(payload))
				req.Header.Set("Authorization", "Bearer "+fixture.subscriptionKey)
				req.Header.Set("Content-Type", "application/json")
				response, err := fixture.public.Client().Do(req)
				if err != nil {
					t.Fatal("public integrity request failed")
				}
				defer response.Body.Close()
				actual, err := io.ReadAll(response.Body)
				if err != nil {
					t.Fatal("public integrity body failed")
				}
				valid := scenario == "valid"
				if !streaming && !valid {
					if response.StatusCode != http.StatusBadGateway || !bytes.Contains(actual, []byte("upstream_response_failed")) {
						t.Fatal("aggregate hid upstream failure")
					}
				} else if response.StatusCode != http.StatusOK {
					t.Fatal("unexpected stream status")
				}
				if streaming && !valid {
					if response.Trailer.Get(protocolv1.FailurePhaseTrailer) != "upstream_stream" || response.Trailer.Get(protocolv1.DeliveryStateTrailer) != "delivered" || response.Trailer.Get(protocolv1.RetryAdviceTrailer) != "never" {
						t.Fatal("failure trailers did not survive the Core HTTP hop")
					}
					if scenario != "completed-then-invalid" && strings.Contains(string(actual), `"type":"response.completed"`) {
						t.Fatal("invalid completion was delivered")
					}
					if (scenario == "conflict" || scenario == "malformed") && !bytes.Contains(actual, []byte("verified prefix")) {
						t.Fatal("validated prefix was lost")
					}
				}
				record := waitForProfileRequestRecord(t, fixture.store, fixture.subscriptionKeyID, response.Header.Get("X-Mini-Sub2Api-Request-Id"))
				if (record.Status == storage.RequestCompleted) != valid {
					t.Fatal("invalid terminal was counted as success")
				}
			})
		}
	}
}
