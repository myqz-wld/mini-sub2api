package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestResponsesSSEErrorThenFailedPreservesFailureAndUsage(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, created := range []bool{false, true} {
			for _, fragmented := range []bool{false, true} {
				t.Run(fmt.Sprintf("subscription=%t/created=%t/fragmented=%t", subscription, created, fragmented), func(t *testing.T) {
					wire := sseErrorFailedBody(created)
					fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
						w.Header().Set("Content-Type", "text/event-stream")
						if fragmented {
							for _, b := range wire {
								_, _ = w.Write([]byte{b})
								w.(http.Flusher).Flush()
							}
						} else {
							_, _ = w.Write(wire)
						}
						return "resp_failed_tail", ""
					})
					key, keyID := fixture.apiKey, fixture.apiKeyID
					if subscription {
						key, keyID = fixture.subscriptionKey, fixture.subscriptionKeyID
					}
					ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
					defer cancel()
					req, _ := http.NewRequestWithContext(ctx, http.MethodPost, fixture.public.URL+"/v1/responses", strings.NewReader(`{"model":"gpt-5.4","stream":true,"input":"synthetic failure footer"}`))
					req.Header.Set("Authorization", "Bearer "+key)
					req.Header.Set("Content-Type", "application/json")
					response, err := fixture.public.Client().Do(req)
					if err != nil {
						t.Fatal("failed-footer request transport failed")
					}
					defer response.Body.Close()
					body, err := io.ReadAll(response.Body)
					if err != nil || response.StatusCode != http.StatusOK {
						t.Fatal("failed footer changed HTTP transport completion")
					}
					for _, trailer := range []string{protocolv1.FailurePhaseTrailer, protocolv1.DeliveryStateTrailer, protocolv1.RetryAdviceTrailer} {
						if response.Trailer.Get(trailer) != "" {
							t.Fatal("valid failed footer became a stream translation failure")
						}
					}
					events := decodeFailureSSE(t, body)
					want := 2
					if created {
						want++
					}
					if len(events) != want || events[want-2]["type"] != "error" || events[want-1]["type"] != "response.failed" {
						t.Fatal("failed SSE events were dropped or reordered")
					}
					failed, _ := events[want-1]["response"].(map[string]any)
					usage, _ := failed["usage"].(map[string]any)
					if usage["total_tokens"] != float64(17) {
						t.Fatal("failed footer usage was not delivered")
					}
					id, _ := failed["id"].(string)
					if id == "" || (subscription && id == "resp_failed_tail") {
						t.Fatal("failed response identity was not projected")
					}
					if created && events[0]["response"].(map[string]any)["id"] != id {
						t.Fatal("failed footer changed the response identity")
					}
					if !subscription && !bytes.Equal(body, wire) {
						t.Fatal("API-key SSE bytes changed")
					}
					record := waitForProfileRequestRecord(t, fixture.store, keyID, response.Header.Get("X-Mini-Sub2Api-Request-Id"))
					if record.Status != storage.RequestUpstreamErr || record.Usage == nil || record.Usage.TotalTokens != 17 {
						t.Fatal("failed footer outcome or recorded usage changed")
					}
					stats, err := fixture.store.Stats(ctx, keyID, "", "")
					if err != nil || len(stats) != 1 || stats[0].CompletedCount != 0 || stats[0].ErrorCount != 1 || stats[0].Usage == nil || stats[0].Usage.TotalTokens != 17 {
						t.Fatal("failure was counted more than once or lost its usage")
					}
					waitForRoutingCapture(t, fixture.captures)
					if subscription {
						assertPartialReferenceUnavailable(t, fixture.public.URL, fixture.public.Client(), key, id)
						select {
						case <-fixture.captures:
							t.Fatal("failed response history reached inference")
						default:
						}
					}
				})
			}
		}
	}
}

func sseErrorFailedBody(created bool) []byte {
	body := ""
	if created {
		body = "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_failed_tail\"}}\n\n"
	}
	body += "event: error\ndata: {\"type\":\"error\",\"code\":\"synthetic\",\"message\":\"synthetic failure\"}\n\n"
	body += "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_failed_tail\",\"status\":\"failed\",\"output\":[],\"usage\":{\"total_tokens\":17}}}\n\n"
	return []byte(body)
}

func decodeFailureSSE(t *testing.T, body []byte) []map[string]any {
	t.Helper()
	var events []map[string]any
	for _, line := range bytes.Split(body, []byte("\n")) {
		data, ok := bytes.CutPrefix(line, []byte("data: "))
		if !ok {
			continue
		}
		var event map[string]any
		if json.Unmarshal(data, &event) != nil {
			t.Fatal("invalid SSE event JSON")
		}
		events = append(events, event)
	}
	return events
}
