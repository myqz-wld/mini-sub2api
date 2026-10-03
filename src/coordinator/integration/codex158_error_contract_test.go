package integration

import (
	"context"
	"encoding/json"
	"net/http"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestCodex158NativeErrorsThroughPublicHTTPAndWebSocket(t *testing.T) {
	for _, tc := range []struct {
		code, kind, wantCode, wantType string
		status                         int
	}{
		{"invalid_prompt", "", "invalid_prompt", "", 400},
		{"flex_unavailable", "", "flex_unavailable", "", 429},
		{"server_is_overloaded", "", "server_is_overloaded", "", 503},
		{"slow_down", "", "slow_down", "", 503},
		{"misalignment_policy_violation", "", "misalignment_policy_violation", "", 403},
		{"misalignment_policy_violation", "", "misalignment_policy_violation", "", 400},
		{"cyber_policy", "", "cyber_policy", "", 400},
		{"bio_policy", "", "bio_policy", "", 400},
		{"", "usage_limit_reached", "usage_limit_reached", "usage_limit_reached", 429},
		{"", "usage_not_included", "usage_not_included", "usage_not_included", 429},
		{"", "insufficient_quota", "insufficient_quota", "insufficient_quota", 429},
		{"credit_balance_exhausted", "", "insufficient_quota", "insufficient_quota", 429},
	} {
		t.Run(tc.wantCode, func(t *testing.T) {
			var attempts atomic.Int32
			errorFields := map[string]any{"message": "private-synthetic", "detail": "private-synthetic"}
			if tc.code != "" {
				errorFields["code"] = tc.code
			}
			if tc.kind != "" {
				errorFields["type"] = tc.kind
			}
			payload := mustRequestJSON(t, map[string]any{"error": errorFields})
			fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
				attempts.Add(1)
				w.Header().Set("Content-Type", "application/json")
				w.WriteHeader(tc.status)
				_, _ = w.Write(payload)
				return "", ""
			})
			status, raw, _ := publicRequest(t, fixture.public, fixture.subscriptionKey, `{"model":"gpt-5.5","input":[]}`)
			if status != tc.status {
				t.Fatal("Subscription status changed")
			}
			e := decodeRequestObject(t, []byte(raw))["error"].(map[string]any)
			wantType := tc.wantType
			if wantType == "" {
				wantType = "mini_sub2api_error"
			}
			if e["code"] != tc.wantCode || e["type"] != wantType || e["requestId"] != nil || e["detail"] != nil || e["message"] == "private-synthetic" ||
				e["retryAdvice"] != "never" || e["deliveryState"] != "delivered" || e["phase"] != "upstream_response" {
				t.Fatal("native category or private public-envelope boundary changed")
			}
			waitForRoutingCapture(t, fixture.captures)
			status, raw, _ = publicRequest(t, fixture.public, fixture.apiKey, `{"model":"gpt-5.5","input":[]}`)
			if status != tc.status || raw != string(payload) {
				t.Fatal("API-key error changed")
			}
			waitForRoutingCapture(t, fixture.captures)

			c := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, http.Header{"Originator": []string{"codex_exec"}})
			defer c.CloseNow()
			writeE2EWebSocketText(t, c, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			defer cancel()
			_, frame, err := c.Read(ctx)
			if err != nil {
				t.Fatal("typed deferred error was not delivered")
			}
			event := decodeRequestObject(t, frame)
			e = event["error"].(map[string]any)
			if event["type"] != "error" || event["status"] != float64(tc.status) || e["code"] != tc.wantCode || e["message"] == "private-synthetic" || e["detail"] != nil {
				t.Fatal("deferred category/status/privacy mismatch")
			}
			if tc.wantType != "" && e["type"] != tc.wantType {
				t.Fatal("native quota type missing")
			}
			_, _, err = c.Read(ctx)
			if websocket.CloseStatus(err) != 4500 {
				t.Fatal("missing bounded failure close")
			}
			waitForRoutingCapture(t, fixture.captures)
			if attempts.Load() != 3 {
				t.Fatal("unexpected upstream replay")
			}
		})
	}
}

func TestNativeWrappedWebSocketErrorPreservesBothCredentialRoutes(t *testing.T) {
	payload := mustRequestJSONValue(map[string]any{"type": "error", "status": 400, "error": map[string]any{"code": "invalid_prompt", "message": "private-synthetic"}})
	fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(c *websocket.Conn, _ []byte, _ string) {
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		defer cancel()
		_ = c.Write(ctx, websocket.MessageText, payload)
	})
	for _, subscription := range []bool{false, true} {
		name, key := "api-key", fixture.apiKey
		if subscription {
			name, key = "subscription", fixture.subscriptionKey
		}
		t.Run(name, func(t *testing.T) {
			c := dialResponsesProfileWebSocket(t, fixture.public, key, http.Header{"Originator": []string{"codex_exec"}})
			defer c.CloseNow()
			writeE2EWebSocketText(t, c, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
			if subscription {
				readResponsesProfileModelNotice(t, c)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			defer cancel()
			_, got, err := c.Read(ctx)
			if err != nil {
				t.Fatal("valid wrapped error was dropped")
			}
			if !subscription {
				if string(got) != string(payload) {
					t.Fatal("API-key frame changed")
				}
				return
			}
			var event struct {
				Type   string
				Status int
				Error  struct{ Code, Message string }
			}
			if json.Unmarshal(got, &event) != nil || event.Type != "error" || event.Status != 400 || event.Error.Code != "invalid_prompt" || event.Error.Message == "private-synthetic" {
				t.Fatal("Subscription wrapped error classification/privacy mismatch")
			}
		})
	}
}
