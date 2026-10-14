package integration

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func retryAfterEvent(kind, id string) map[string]any {
	value := retryMetadataEvent(kind, id)
	nested := value
	if response, ok := nested["response"].(map[string]any); ok {
		nested = response
	}
	if failure, ok := nested["error"].(map[string]any); ok {
		nested = failure
	}
	nested["headers"].(map[string]any)["Retry-After"] = 7
	if kind == "error" {
		value["headers"] = map[string]any{"retry-after": "30"}
	}
	return value
}

func assertRetryAfter(t *testing.T, event map[string]any) {
	t.Helper()
	nested := event
	if response, ok := nested["response"].(map[string]any); ok {
		nested = response
	}
	if failure, ok := nested["error"].(map[string]any); ok {
		nested = failure
	}
	headers, ok := nested["headers"].(map[string]any)
	if !ok || headers["retry-after"] != "7" || headers["x-retry-metadata"] != "NO_MORE_RETRY" || len(headers) != 2 {
		t.Fatal("safe nested Retry-After and retry metadata did not survive")
	}
	if strings.Contains(string(mustRequestJSONValue(event)), "synthetic-private") {
		t.Fatal("private error material escaped")
	}
}

func TestRetryAfterAcrossSSEJSONAndWebSocketWithoutGatewayReplay(t *testing.T) {
	for _, kind := range []string{"error", "response.failed"} {
		t.Run(kind, func(t *testing.T) {
			var sends atomic.Int32
			fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
				sends.Add(1)
				id := fmt.Sprintf("resp_retry_after_%d", loopbackResponseSequence.Add(1))
				w.Header().Set("Content-Type", "text/event-stream")
				fmt.Fprintf(w, "data: %s\n\n", mustRequestJSONValue(retryAfterEvent(kind, id)))
				return id, ""
			})
			streams := []bool{false, true}
			if kind == "error" {
				streams = []bool{true}
			}
			for _, stream := range streams {
				status, body, _ := publicRequest(t, fixture.public, fixture.subscriptionKey, fmt.Sprintf(`{"model":"gpt-5.5","input":[],"stream":%t}`, stream))
				if status != http.StatusOK {
					t.Fatal("response relay failed")
				}
				if stream {
					seen := false
					for _, line := range strings.Split(body, "\n") {
						if payload, ok := strings.CutPrefix(line, "data: "); ok && payload != "[DONE]" {
							assertRetryAfter(t, decodeRequestObject(t, []byte(payload)))
							seen = true
						}
					}
					if !seen {
						t.Fatal("SSE failure absent")
					}
				} else {
					assertRetryAfter(t, decodeRequestObject(t, []byte(body)))
				}
			}
			if sends.Load() != int32(len(streams)) {
				t.Fatal("gateway replayed inference")
			}
			var frames atomic.Int32
			ws := newResponsesProfileWebSocketFixtureWithResponder(t, func(c *websocket.Conn, _ []byte, id string) {
				frames.Add(1)
				ctx, cancel := context.WithTimeout(context.Background(), time.Second)
				defer cancel()
				_ = c.Write(ctx, websocket.MessageText, mustRequestJSONValue(retryAfterEvent(kind, id)))
			})
			c := dialResponsesProfileWebSocket(t, ws.public, ws.subscriptionKey, http.Header{"Originator": {"codex_exec"}})
			defer c.CloseNow()
			writeE2EWebSocketText(t, c, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
			if kind == "error" {
				readResponsesProfileModelNotice(t, c)
			}
			assertRetryAfter(t, decodeRequestObject(t, []byte(readE2EWebSocketText(t, c))))
			if frames.Load() != 1 {
				t.Fatal("gateway replayed WS inference")
			}
		})
	}
}

func TestDeferredRetryAfterKeepsActualGenericAndNativeStatus(t *testing.T) {
	for _, status := range []int{429, 503, 418, 426, 501} {
		for _, code := range []string{"server_is_overloaded", "flex_unavailable", "insufficient_quota", "private_unknown"} {
			for _, advice := range []string{"7", "Wed, 21 Oct 2015 07:28:00 GMT", "invalid-private"} {
				t.Run(fmt.Sprintf("%d/%s/%s", status, code, strings.ReplaceAll(advice, " ", "_")), func(t *testing.T) {
					var calls atomic.Int32
					fixture := newResponsesProfileWebSocketFixtureWithHandshake(t, nil, func(w http.ResponseWriter, _ *http.Request, _ int) bool {
						calls.Add(1)
						w.Header().Set("Retry-After", advice)
						w.Header().Set("X-Retry-Metadata", "NO_MORE_RETRY")
						w.Header().Set("X-Request-Id", "synthetic-private-provider-id")
						w.Header().Set("Content-Type", "application/json")
						w.WriteHeader(status)
						_ = json.NewEncoder(w).Encode(map[string]any{"error": map[string]any{"code": code, "message": "synthetic-private"}})
						return false
					})
					c := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, http.Header{"Originator": {"codex_exec"}})
					defer c.CloseNow()
					writeE2EWebSocketText(t, c, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
					raw := readE2EWebSocketText(t, c)
					event := decodeRequestObject(t, []byte(raw))
					failure := event["error"].(map[string]any)
					headers, _ := failure["headers"].(map[string]any)
					if event["type"] != "error" || event["status"] != float64(status) {
						t.Fatal("actual rejected-upgrade status lost")
					}
					if advice == "invalid-private" {
						if headers["retry-after"] != nil {
							t.Fatal("invalid advice survived")
						}
					} else if headers["retry-after"] != advice {
						t.Fatal("valid advice lost")
					}
					if headers["x-retry-metadata"] != "NO_MORE_RETRY" || strings.Contains(raw, "private") {
						t.Fatal("retry metadata/privacy boundary changed")
					}
					assertUpgradeFailureClose(t, c, protocolv1.DeliveryDelivered)
					if calls.Load() != 1 {
						t.Fatal("handshake rejection was replayed")
					}
					assertNoUpgradeFrames(t, fixture)
				})
			}
		}
	}
}
