package integration

import (
	"context"
	"encoding/json"
	"net/http"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestCodexProfilesIgnoreUnsupportedTypedWebSocketControlFrames(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixtureWithResponder(
		t,
		func(connection *websocket.Conn, payload []byte, responseID string) {
			var event map[string]any
			if json.Unmarshal(payload, &event) != nil {
				return
			}
			switch event["type"] {
			case "response.create":
				_ = connection.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(map[string]any{
					"type": "response.created", "response": map[string]any{"id": responseID},
				}))
				_ = connection.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(map[string]any{
					"type": "response.output_item.done", "response_id": responseID,
					"item": map[string]any{
						"type": "function_call", "id": "fc-control-provider",
						"call_id": "call-control-provider", "name": "lookup", "arguments": "{}",
					},
				}))
			case "response.append_input_item":
				_ = connection.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(map[string]any{
					"type": "response.completed", "response": map[string]any{
						"id":    event["response_id"],
						"usage": map[string]any{"input_tokens": 1, "output_tokens": 0, "total_tokens": 1},
					},
				}))
			}
		},
	)
	profiles := []struct {
		name         string
		secret       string
		keyID        string
		subscription bool
	}{
		{name: "codex_subscription_marked", secret: fixture.subscriptionKey, keyID: fixture.subscriptionKeyID, subscription: true},
		{
			name: "codex_subscription", secret: fixture.subscriptionKey,
			keyID: fixture.subscriptionKeyID, subscription: true,
		},
	}
	for _, profile := range profiles {
		t.Run(profile.name, func(t *testing.T) {
			connection := dialResponsesProfileWebSocket(
				t, fixture.public, profile.secret,
				http.Header{"Originator": []string{"codex_exec"}},
			)
			defer connection.CloseNow()
			writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.4","generate":true,"input":[],"client_metadata":{"session_id":"control-conversation"}}`)
			createdText := readE2EWebSocketText(t, connection)
			var created map[string]any
			if json.Unmarshal([]byte(createdText), &created) != nil {
				t.Fatalf("created event = %q", createdText)
			}
			publicResponseID, _ := created["response"].(map[string]any)["id"].(string)
			if publicResponseID == "" {
				t.Fatal("created event omitted response alias")
			}
			callText := readE2EWebSocketText(t, connection)
			var callEvent map[string]any
			if json.Unmarshal([]byte(callText), &callEvent) != nil {
				t.Fatalf("call event = %q", callText)
			}
			callItem, _ := callEvent["item"].(map[string]any)
			publicCallID, _ := callItem["call_id"].(string)
			if publicCallID == "" || publicCallID == "call-control-provider" {
				t.Fatalf("call ID was not translated: %#v", callEvent)
			}
			createCapture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
			control := mustRequestJSON(t, map[string]any{
				"type": "response.append_input_item", "response_id": publicResponseID,
				"item": map[string]any{
					"type": "function_call_output", "id": "item-control-downstream",
					"call_id": publicCallID,
					"output":  map[string]any{"opaque_id": "opaque-must-stay"},
				},
			})
			writeE2EWebSocketText(t, connection, string(control))
			select {
			case <-fixture.captures:
				t.Fatal("unsupported typed control reached upstream")
			case <-time.After(100 * time.Millisecond):
			}
			assertWebSocketProfileCredentialBoundary(t, createCapture, profile.subscription)
		})
	}
}
