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
)

type usageWindow1592Case struct {
	name, raw, kind string
	want            *uint16
}

func usageWindow1592Cases() []usageWindow1592Case {
	cases := []usageWindow1592Case{
		{name: "absent"}, {name: "null", raw: "null"},
		{name: "zero", raw: "0", want: new(uint16(0))},
		{name: "five-hours", raw: "300", want: new(uint16(300))},
		{name: "week", raw: "10080", want: new(uint16(10080))},
		{name: "maximum", raw: "65535", want: new(uint16(65535))},
		{name: "negative", raw: "-1"}, {name: "overflow", raw: "65536"},
		{name: "large-overflow", raw: "18446744073709551616"},
		{name: "fraction", raw: "1.5"}, {name: "decimal", raw: "300.0"},
		{name: "exponent", raw: "3e2"}, {name: "string", raw: `"300"`},
		{name: "boolean", raw: "true"}, {name: "object", raw: `{"minutes":300}`},
		{name: "array", raw: "[300]"},
		{name: "other-category", raw: "300", kind: "usage_not_included"},
	}
	for i := range cases {
		if cases[i].kind == "" {
			cases[i].kind = "usage_limit_reached"
		}
	}
	return cases
}

func (tc usageWindow1592Case) errorJSON() string {
	errorJSON := `{"type":"` + tc.kind + `","message":"synthetic-private","debug":"synthetic-private"`
	if tc.raw != "" {
		errorJSON += `,"limit_window_minutes":` + tc.raw
	}
	return errorJSON + `}`
}

func assertUsageWindow1592(t *testing.T, fields map[string]any, tc usageWindow1592Case) {
	t.Helper()
	if fields["type"] != tc.kind {
		t.Fatal("native usage error type changed")
	}
	window, exists := fields["limit_window_minutes"]
	if tc.want == nil {
		if exists {
			t.Fatal("invalid or unrelated usage window reached the public error")
		}
	} else if !exists || window != float64(*tc.want) {
		t.Fatal("bounded usage window missing or changed")
	}
	if fields["debug"] != nil || fields["message"] == "synthetic-private" || fields["limitWindowMinutes"] != nil {
		t.Fatal("private or internal fields reached the public error")
	}
}

func TestUsageWindow1592HTTPAndDeferredHandshake(t *testing.T) {
	var payload atomic.Value
	payload.Store([]byte(`{}`))
	var attempts atomic.Int32
	fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
		attempts.Add(1)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusTooManyRequests)
		_, _ = w.Write(payload.Load().([]byte))
		return "", ""
	})
	for _, tc := range usageWindow1592Cases() {
		t.Run(tc.name, func(t *testing.T) {
			before := attempts.Load()
			upstream := []byte(`{"error":` + tc.errorJSON() + `}`)
			payload.Store(upstream)
			status, raw, _ := publicRequest(t, fixture.public, fixture.subscriptionKey, `{"model":"gpt-5.5","input":[]}`)
			if status != http.StatusTooManyRequests {
				t.Fatal("Subscription status changed")
			}
			fields := decodeRequestObject(t, []byte(raw))["error"].(map[string]any)
			assertUsageWindow1592(t, fields, tc)
			if fields["code"] != tc.kind || fields["deliveryState"] != "delivered" || fields["retryAdvice"] != "never" {
				t.Fatal("HTTP category or delivery metadata changed")
			}
			waitForRoutingCapture(t, fixture.captures)
			status, raw, _ = publicRequest(t, fixture.public, fixture.apiKey, `{"model":"gpt-5.5","input":[]}`)
			if status != http.StatusTooManyRequests || raw != string(upstream) {
				t.Fatal("API-key HTTP error changed")
			}
			waitForRoutingCapture(t, fixture.captures)

			connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey,
				http.Header{"Originator": {"codex_exec"}})
			defer connection.CloseNow()
			writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
			event := decodeRequestObject(t, []byte(readE2EWebSocketText(t, connection)))
			fields = event["error"].(map[string]any)
			assertUsageWindow1592(t, fields, tc)
			if event["type"] != "error" || event["status"] != float64(429) || fields["code"] != tc.kind {
				t.Fatal("deferred native category/status changed")
			}
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			defer cancel()
			_, _, err := connection.Read(ctx)
			if websocket.CloseStatus(err) != 4500 {
				t.Fatal("deferred rejection did not end with a bounded failure close")
			}
			waitForRoutingCapture(t, fixture.captures)
			if attempts.Load()-before != 3 {
				t.Fatal("unexpected upstream replay")
			}
		})
	}
}

func usageWindow1592Event(tc usageWindow1592Case, wrapped bool) []byte {
	if wrapped {
		return []byte(`{"type":"error","status":429,"error":` + tc.errorJSON() + `}`)
	}
	return []byte(fmt.Sprintf(`{"type":"response.failed","response":{"id":"resp_usage_window_%d","object":"response","status":"failed","output":[],"error":%s}}`,
		loopbackResponseSequence.Add(1), tc.errorJSON()))
}

func assertUsageWindow1592Event(t *testing.T, data []byte, tc usageWindow1592Case, wrapped bool) {
	t.Helper()
	event := decodeRequestObject(t, data)
	container := event
	if wrapped {
		if event["type"] != "error" || event["status"] != float64(429) {
			t.Fatal("wrapped error envelope changed")
		}
	} else {
		if event["type"] != "response.failed" {
			t.Fatal("failed event type changed")
		}
		container = event["response"].(map[string]any)
	}
	fields := container["error"].(map[string]any)
	assertUsageWindow1592(t, fields, tc)
	wantLen := 3
	if tc.want != nil {
		wantLen++
	}
	if len(fields) != wantLen || fields["message"] != "The upstream request failed." {
		t.Fatal("streamed public error contains unexpected metadata")
	}
	if strings.Contains(string(data), "synthetic-private") {
		t.Fatal("provider text escaped stream privacy filtering")
	}
}

func TestUsageWindow1592SSEErrors(t *testing.T) {
	var payload atomic.Value
	payload.Store([]byte(`{}`))
	fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = fmt.Fprintf(w, "data: %s\n\n", payload.Load().([]byte))
		return "", ""
	})
	for _, wrapped := range []bool{false, true} {
		for _, tc := range usageWindow1592Cases() {
			for _, subscription := range []bool{false, true} {
				t.Run(fmt.Sprintf("wrapped=%t/%s/subscription=%t", wrapped, tc.name, subscription), func(t *testing.T) {
					upstream := usageWindow1592Event(tc, wrapped)
					payload.Store(upstream)
					key := fixture.apiKey
					if subscription {
						key = fixture.subscriptionKey
					}
					status, raw, _ := publicRequest(t, fixture.public, key, `{"model":"gpt-5.5","input":[],"stream":true}`)
					waitForRoutingCapture(t, fixture.captures)
					if status != http.StatusOK {
						t.Fatal("stream status changed")
					}
					if !subscription {
						if raw != "data: "+string(upstream)+"\n\n" {
							t.Fatal("API-key SSE bytes changed")
						}
						return
					}
					seen := 0
					for _, line := range strings.Split(raw, "\n") {
						data, ok := strings.CutPrefix(line, "data: ")
						if !ok || !json.Valid([]byte(data)) {
							continue
						}
						assertUsageWindow1592Event(t, []byte(data), tc, wrapped)
						seen++
					}
					if seen != 1 {
						t.Fatal("stream error missing or duplicated")
					}
				})
			}
		}
	}
}

func TestUsageWindow1592WebSocketErrors(t *testing.T) {
	var payload atomic.Value
	payload.Store([]byte(`{}`))
	fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(c *websocket.Conn, _ []byte, _ string) {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		_ = c.Write(ctx, websocket.MessageText, payload.Load().([]byte))
	})
	for _, wrapped := range []bool{false, true} {
		for _, tc := range usageWindow1592Cases() {
			for _, subscription := range []bool{false, true} {
				t.Run(fmt.Sprintf("wrapped=%t/%s/subscription=%t", wrapped, tc.name, subscription), func(t *testing.T) {
					upstream := usageWindow1592Event(tc, wrapped)
					payload.Store(upstream)
					key := fixture.apiKey
					if subscription {
						key = fixture.subscriptionKey
					}
					connection := dialResponsesProfileWebSocket(t, fixture.public, key, http.Header{"Originator": {"codex_exec"}})
					defer connection.CloseNow()
					writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
					if subscription && wrapped {
						readResponsesProfileModelNotice(t, connection)
					}
					data := []byte(readE2EWebSocketText(t, connection))
					waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)
					if !subscription {
						if string(data) != string(upstream) {
							t.Fatal("API-key WS bytes changed")
						}
						return
					}
					assertUsageWindow1592Event(t, data, tc, wrapped)
				})
			}
		}
	}
}
