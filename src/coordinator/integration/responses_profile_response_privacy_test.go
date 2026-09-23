package integration

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"testing"

	"github.com/coder/websocket"
)

func privacyMetadata() map[string]any {
	return map[string]any{"type": "response.metadata", "headers": map[string]any{
		"Authorization": "synthetic-private-auth", "Set-Cookie": "synthetic-private-cookie",
		"X-Codex-Installation-Id": "synthetic-private-installation", "X-Future-Private": "synthetic-private-extension",
		"X-Request-Id": []any{"synthetic-provider-id"}, "retry-after": "5",
		"x-codex-turn-state": []any{"synthetic-routing-state"},
	}}
}

func privacyFailure(responseID string) map[string]any {
	return map[string]any{"type": "response.failed", "response": map[string]any{
		"id": responseID, "object": "response", "output": []any{},
		"headers": privacyMetadata()["headers"],
		"error":   map[string]any{"code": "context_length_exceeded", "message": "synthetic-private-error", "id": "synthetic-private-id"},
	}}
}

func assertPublicMetadataPrivacy(t *testing.T, event map[string]any, requestID string, subscription bool) {
	t.Helper()
	headers, ok := event["headers"].(map[string]any)
	if !ok {
		t.Fatal("metadata header container missing")
	}
	if !subscription {
		if headers["Authorization"] != "synthetic-private-auth" {
			t.Fatal("API-key metadata changed")
		}
		return
	}
	for _, name := range []string{"Authorization", "Set-Cookie", "X-Codex-Installation-Id", "X-Future-Private"} {
		if _, exists := headers[name]; exists {
			t.Fatalf("private metadata field crossed: %s", name)
		}
	}
	values, ok := headers["X-Request-Id"].([]any)
	if !ok || len(values) != 1 || values[0] == "synthetic-provider-id" || values[0] == "" {
		t.Fatal("metadata provider request ID was not aliased")
	}
	if requestID != "" && values[0] != requestID {
		t.Fatal("metadata alias differs from gateway request ID")
	}
	routing, ok := headers["x-codex-turn-state"].([]any)
	if headers["retry-after"] != "5" || !ok || len(routing) != 1 || routing[0] != "synthetic-routing-state" {
		t.Fatal("approved metadata control changed")
	}
}

func assertPublicFailurePrivacy(t *testing.T, event map[string]any, subscription bool) {
	t.Helper()
	response, ok := event["response"].(map[string]any)
	if !ok {
		response = event
	}
	assertPublicMetadataPrivacy(t, response, "", subscription)
	failure, ok := response["error"].(map[string]any)
	if !ok {
		t.Fatal("terminal error missing")
	}
	if !subscription {
		if failure["message"] != "synthetic-private-error" {
			t.Fatal("API-key error body changed")
		}
		return
	}
	if len(failure) != 2 || failure["code"] != "context_length_exceeded" || failure["message"] != "The upstream request failed." {
		t.Fatal("Subscription error fields crossed the public boundary or lost the native error code")
	}
}

func TestProfilesApplyResponsePrivacyToHTTPJSONAndSSE(t *testing.T) {
	fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, body []byte) (string, string) {
		responseID := fmt.Sprintf("resp_privacy_%d", loopbackResponseSequence.Add(1))
		var request map[string]any
		if json.Unmarshal(body, &request) != nil {
			t.Error("invalid mock request")
			return "", ""
		}
		w.Header().Set("X-Request-Id", "synthetic-provider-id")
		w.Header().Set("Authorization", "synthetic-private-auth")
		if request["stream"] == true {
			w.Header().Set("Content-Type", "text/event-stream")
			for _, event := range []map[string]any{privacyMetadata(), privacyFailure(responseID)} {
				_, _ = fmt.Fprint(w, ": synthetic-private-sse\nid: synthetic-private-sse\nretry: synthetic-private-sse\n")
				_, _ = fmt.Fprintf(w, "data: %s\n\n", mustRequestJSONValue(event))
			}
		} else {
			w.Header().Set("Content-Type", "application/json")
			_ = json.NewEncoder(w).Encode(privacyFailure(responseID)["response"])
		}
		return responseID, "synthetic-provider-id"
	})
	for _, subscription := range []bool{false, true} {
		for _, stream := range []bool{false, true} {
			t.Run(fmt.Sprintf("subscription=%t/stream=%t", subscription, stream), func(t *testing.T) {
				key := fixture.apiKey
				if subscription {
					key = fixture.subscriptionKey
				}
				status, body, headers := publicRequestWithHeaders(t, fixture.public, key,
					fmt.Sprintf(`{"model":"gpt-5.4","input":[],"stream":%t}`, stream), nil)
				if status != http.StatusOK || headers.Get("Authorization") != "" {
					t.Fatal("HTTP privacy fixture failed")
				}
				if !stream {
					assertPublicFailurePrivacy(t, decodeRequestObject(t, []byte(body)), subscription)
					return
				}
				if strings.Contains(body, "synthetic-private-sse") == subscription {
					t.Fatal("SSE envelope privacy differs from its credential profile")
				}
				metadataSeen, failureSeen := false, false
				for _, line := range strings.Split(body, "\n") {
					data, ok := strings.CutPrefix(line, "data: ")
					if !ok {
						continue
					}
					event := decodeRequestObject(t, []byte(data))
					switch event["type"] {
					case "response.metadata":
						assertPublicMetadataPrivacy(t, event, headers.Get("X-Mini-Sub2Api-Request-Id"), subscription)
						metadataSeen = true
					case "response.failed":
						assertPublicFailurePrivacy(t, event, subscription)
						failureSeen = true
					}
				}
				if !metadataSeen || !failureSeen {
					t.Fatal("missing privacy fixture events")
				}
			})
		}
	}
}

func TestProfilesApplyResponsePrivacyToWebSocket(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(connection *websocket.Conn, _ []byte, responseID string) {
		for _, event := range []map[string]any{privacyMetadata(), privacyFailure(responseID)} {
			_ = connection.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(event))
		}
	})
	for _, subscription := range []bool{false, true} {
		t.Run(fmt.Sprintf("subscription=%t", subscription), func(t *testing.T) {
			key := fixture.apiKey
			if subscription {
				key = fixture.subscriptionKey
			}
			connection := dialResponsesProfileWebSocket(t, fixture.public, key, http.Header{"Originator": []string{"codex_exec"}})
			defer connection.CloseNow()
			writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.4","input":[]}`)
			metadata := decodeRequestObject(t, []byte(readE2EWebSocketText(t, connection)))
			assertPublicMetadataPrivacy(t, metadata, "", subscription)
			failure := decodeRequestObject(t, []byte(readE2EWebSocketText(t, connection)))
			assertPublicFailurePrivacy(t, failure, subscription)
		})
	}
}
