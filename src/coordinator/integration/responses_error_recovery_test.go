package integration

import (
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"sync/atomic"
	"testing"

	"mini-sub2api/src/coordinator/internal/storage"
)

func TestContextHTTPRejectionBecomesSubscriptionSSEFailureOnly(t *testing.T) {
	for _, tc := range []struct {
		name    string
		status  int
		body    string
		context bool
	}{
		{"context", 400, `{"error":{"code":"context_length_exceeded","message":"synthetic-private","debug":"synthetic-private"}}`, true},
		{"wrong_status", 413, `{"error":{"code":"context_length_exceeded","message":"synthetic-private"}}`, false},
		{"wrong_code", 400, `{"error":{"code":"unknown","message":"synthetic-private"}}`, false},
		{"malformed", 400, `{"error":{"code":"context_length_exceeded"}} trailing`, false},
		{"oversized", 400, `{"error":{"code":"context_length_exceeded"},"debug":"` + strings.Repeat("x", 65536) + `"}`, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var sends atomic.Int32
			fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
				sends.Add(1)
				w.Header().Set("Content-Type", "application/json")
				w.Header().Set("Retry-After", "2")
				w.Header().Set("Set-Cookie", "synthetic-private")
				w.WriteHeader(tc.status)
				_, _ = w.Write([]byte(tc.body))
				return "", ""
			})
			for _, subscription := range []bool{false, true} {
				for _, stream := range []bool{false, true} {
					key, keyID := fixture.apiKey, fixture.apiKeyID
					if subscription {
						key, keyID = fixture.subscriptionKey, fixture.subscriptionKeyID
					}
					status, body, headers := publicRequest(t, fixture.public, key, fmt.Sprintf(`{"model":"gpt-5.5","input":[],"stream":%t}`, stream))
					waitForRoutingCapture(t, fixture.captures)
					converted := subscription && stream && tc.context
					wantStatus := tc.status
					if converted {
						wantStatus = http.StatusOK
					}
					if status != wantStatus || headers.Get("Set-Cookie") != "" || headers.Get("Retry-After") != "2" {
						t.Fatal("context error status/header contract changed")
					}
					if !subscription {
						if body != tc.body {
							t.Fatal("API-key error bytes changed")
						}
					} else {
						if strings.Contains(body, "synthetic-private") {
							t.Fatal("provider error text escaped")
						}
						var failure map[string]any
						if converted {
							if !strings.HasPrefix(headers.Get("Content-Type"), "text/event-stream") {
								t.Fatal("context error did not become SSE")
							}
							events := decodeFailureSSE(t, []byte(body))
							if len(events) != 1 || events[0]["type"] != "response.failed" {
								t.Fatal("missing or duplicate context failure terminal")
							}
							response := events[0]["response"].(map[string]any)
							if response["id"] != nil || response["output"] != nil || response["usage"] != nil {
								t.Fatal("HTTP rejection fabricated response state")
							}
							failure = response["error"].(map[string]any)
						} else {
							var envelope map[string]any
							if json.Unmarshal([]byte(body), &envelope) != nil {
								t.Fatal("invalid public JSON error")
							}
							failure = envelope["error"].(map[string]any)
						}
						code := "upstream_response_failed"
						if tc.context {
							code = "context_length_exceeded"
						}
						if failure["code"] != code {
							t.Fatal("context category changed")
						}
					}
					record := waitForProfileRequestRecord(t, fixture.store, keyID, headers.Get("X-Mini-Sub2Api-Request-Id"))
					if record.Status != storage.RequestUpstreamErr || record.Usage != nil {
						t.Fatal("rejected request accounting changed")
					}
				}
			}
			if sends.Load() != 4 {
				t.Fatal("context normalization replayed inference")
			}
		})
	}
}

func TestStandaloneSSEErrorGetsSubscriptionFailureWithoutReplay(t *testing.T) {
	const wire = "data: {\"type\":\"error\",\"error\":{\"code\":\"server_is_overloaded\",\"message\":\"synthetic-private\",\"headers\":{\"Retry-After\":\"2\"}}}\n\ndata: [DONE]\n\n"
	var sends atomic.Int32
	fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
		sends.Add(1)
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = w.Write([]byte(wire))
		return "", ""
	})
	for _, subscription := range []bool{false, true} {
		key, keyID := fixture.apiKey, fixture.apiKeyID
		if subscription {
			key, keyID = fixture.subscriptionKey, fixture.subscriptionKeyID
		}
		status, body, headers := publicRequest(t, fixture.public, key, `{"model":"gpt-5.5","input":[],"stream":true}`)
		waitForRoutingCapture(t, fixture.captures)
		if status != http.StatusOK {
			t.Fatal("SSE transport status changed")
		}
		if !subscription {
			if body != wire {
				t.Fatal("API-key SSE changed")
			}
		} else {
			if strings.Contains(body, "synthetic-private") || strings.Contains(body, "[DONE]") {
				t.Fatal("unsafe or premature SSE termination")
			}
			events := decodeFailureSSE(t, []byte(body))
			if len(events) != 2 || events[0]["type"] != "error" || events[1]["type"] != "response.failed" {
				t.Fatal("missing or duplicate fallback failure")
			}
			failure := events[1]["response"].(map[string]any)["error"].(map[string]any)
			if failure["code"] != "server_is_overloaded" || failure["headers"].(map[string]any)["retry-after"] != "2" {
				t.Fatal("retry category or advice lost")
			}
		}
		record := waitForProfileRequestRecord(t, fixture.store, keyID, headers.Get("X-Mini-Sub2Api-Request-Id"))
		if record.Status != storage.RequestUpstreamErr || record.Usage != nil {
			t.Fatal("error-only accounting changed")
		}
	}
	if sends.Load() != 2 {
		t.Fatal("gateway retried standalone error")
	}
}
