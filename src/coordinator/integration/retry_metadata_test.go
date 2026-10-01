package integration

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func retryMetadataError() map[string]any {
	return map[string]any{
		"code": "server_is_overloaded", "type": "service_unavailable_error",
		"message": "Our servers are currently overloaded. Please try again later.",
		"headers": map[string]any{"x-retry-metadata": "NO_MORE_RETRY",
			"Authorization": "synthetic-private", "Set-Cookie": "synthetic-private",
			"x-retry-metadata-private": "synthetic-private"},
	}
}

func retryMetadataEvent(kind, responseID string) map[string]any {
	if kind == "response.failed" {
		return map[string]any{"type": kind, "response": map[string]any{
			"id": responseID, "object": "response", "status": "failed", "output": []any{}, "error": retryMetadataError()}}
	}
	if kind == "flat" {
		event := retryMetadataError()
		event["type"], event["status"] = "error", 503
		return event
	}
	return map[string]any{"type": "error", "status": 503, "error": retryMetadataError()}
}

func assertRetryMetadata(t *testing.T, value map[string]any, subscription bool) {
	t.Helper()
	if response, ok := value["response"].(map[string]any); ok {
		value = response
	}
	if failure, ok := value["error"].(map[string]any); ok {
		value = failure
	}
	headers, ok := value["headers"].(map[string]any)
	if value["code"] != "server_is_overloaded" || !ok || headers["x-retry-metadata"] != "NO_MORE_RETRY" {
		t.Fatal("overload code or nested retry metadata was lost")
	}
	if subscription && (len(headers) != 1 || strings.Contains(string(mustRequestJSONValue(value)), "synthetic-private") ||
		value["message"] == retryMetadataError()["message"]) {
		t.Fatal("private error fields escaped")
	}
}

func TestRetryMetadataThroughPublicSSEAndWebSocket(t *testing.T) {
	for _, kind := range []string{"error", "flat", "response.failed"} {
		t.Run(kind, func(t *testing.T) {
			fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
				id := fmt.Sprintf("resp_retry_%d", loopbackResponseSequence.Add(1))
				w.Header().Set("Content-Type", "text/event-stream")
				w.Header().Set("X-Retry-Metadata", "NO_MORE_RETRY")
				w.Header().Set("Authorization", "synthetic-private")
				_, _ = fmt.Fprintf(w, "data: %s\n\n", mustRequestJSONValue(retryMetadataEvent(kind, id)))
				return id, ""
			})
			wsFixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(c *websocket.Conn, _ []byte, id string) {
				ctx, cancel := context.WithTimeout(context.Background(), time.Second)
				defer cancel()
				_ = c.Write(ctx, websocket.MessageText, mustRequestJSONValue(retryMetadataEvent(kind, id)))
			})
			for _, subscription := range []bool{false, true} {
				key, wsKey := fixture.apiKey, wsFixture.apiKey
				if subscription {
					key, wsKey = fixture.subscriptionKey, wsFixture.subscriptionKey
				}
				status, body, headers := publicRequestWithHeaders(t, fixture.public, key,
					`{"model":"gpt-5.5","input":[],"stream":true}`, nil)
				if status != 200 || headers.Get("X-Retry-Metadata") != "NO_MORE_RETRY" || headers.Get("Authorization") != "" {
					t.Fatal("HTTP retry header or privacy changed")
				}
				seen := false
				for _, line := range strings.Split(body, "\n") {
					if data, ok := strings.CutPrefix(line, "data: "); ok && data != "[DONE]" {
						assertRetryMetadata(t, decodeRequestObject(t, []byte(data)), subscription)
						seen = true
					}
				}
				if !seen {
					t.Fatal("missing SSE error")
				}
				waitForRoutingCapture(t, fixture.captures)
				c := dialResponsesProfileWebSocket(t, wsFixture.public, wsKey, http.Header{"Originator": {"codex_exec"}})
				writeE2EWebSocketText(t, c, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
				assertRetryMetadata(t, decodeRequestObject(t, []byte(readE2EWebSocketText(t, c))), subscription)
				_ = c.CloseNow()
			}
		})
	}
}

func TestRetryMetadataThroughHTTPRejectionAndDeferredWebSocket(t *testing.T) {
	fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusServiceUnavailable)
		_ = json.NewEncoder(w).Encode(map[string]any{"error": retryMetadataError()})
		return "", ""
	})
	for _, key := range []string{fixture.subscriptionKey, fixture.apiKey} {
		status, body, _ := publicRequest(t, fixture.public, key, `{"model":"gpt-5.5","input":[]}`)
		if status != http.StatusServiceUnavailable {
			t.Fatal("overload status changed")
		}
		assertRetryMetadata(t, decodeRequestObject(t, []byte(body)), key == fixture.subscriptionKey)
		waitForRoutingCapture(t, fixture.captures)
	}
	c := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, http.Header{"Originator": {"codex_exec"}})
	defer c.CloseNow()
	writeE2EWebSocketText(t, c, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
	assertRetryMetadata(t, decodeRequestObject(t, []byte(readE2EWebSocketText(t, c))), true)
	waitForRoutingCapture(t, fixture.captures)
}

func TestRetryMetadataThroughHTTPJSON(t *testing.T) {
	fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
		id := fmt.Sprintf("resp_retry_json_%d", loopbackResponseSequence.Add(1))
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = fmt.Fprintf(w, "data: %s\n\n", mustRequestJSONValue(retryMetadataEvent("response.failed", id)))
		return id, ""
	})
	status, body, _ := publicRequest(t, fixture.public, fixture.subscriptionKey, `{"model":"gpt-5.5","input":[]}`)
	if status != http.StatusOK {
		t.Fatal("JSON response failed")
	}
	assertRetryMetadata(t, decodeRequestObject(t, []byte(body)), true)
}
