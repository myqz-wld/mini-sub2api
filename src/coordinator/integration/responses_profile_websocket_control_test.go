package integration

import (
	"context"
	"encoding/json"
	"net/http"
	"testing"

	"github.com/coder/websocket"
)

func TestCodexProfilesTranslateTypedWebSocketControlFrames(t *testing.T) {
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
		{name: "codex_api_key", secret: fixture.apiKey, keyID: fixture.apiKeyID},
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
			writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.4","conversation":"control-conversation","input":[]}`)
			createdText := readE2EWebSocketText(t, connection)
			var created map[string]any
			if json.Unmarshal([]byte(createdText), &created) != nil {
				t.Fatalf("created event = %q", createdText)
			}
			publicResponseID, _ := created["response"].(map[string]any)["id"].(string)
			if publicResponseID == "" {
				t.Fatal("created event omitted response alias")
			}
			createCapture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
			control := mustRequestJSON(t, map[string]any{
				"type": "response.append_input_item", "response_id": publicResponseID,
				"item": map[string]any{
					"type": "function_call_output", "id": "item-control-downstream",
					"call_id": "call-control-downstream",
					"output":  map[string]any{"opaque_id": "opaque-must-stay"},
				},
			})
			writeE2EWebSocketText(t, connection, string(control))
			completedText := readE2EWebSocketText(t, connection)
			var completed map[string]any
			if json.Unmarshal([]byte(completedText), &completed) != nil {
				t.Fatalf("completed event = %q", completedText)
			}
			completedResponse, _ := completed["response"].(map[string]any)
			if completedResponse["id"] != publicResponseID {
				t.Fatalf("completed response alias changed: %#v", completed)
			}
			controlCapture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
			upstreamControl := decodeResponsesProfileWebSocketFrame(t, controlCapture.Frame)
			if upstreamControl["response_id"] != createCapture.ResponseID {
				t.Fatalf("control response alias was not restored: %#v", upstreamControl)
			}
			item, _ := upstreamControl["item"].(map[string]any)
			if item["id"] == "item-control-downstream" || item["call_id"] == "call-control-downstream" {
				t.Fatalf("control item IDs were not pseudonymized: %#v", item)
			}
			output, _ := item["output"].(map[string]any)
			if output["opaque_id"] != "opaque-must-stay" {
				t.Fatalf("opaque control output changed: %#v", output)
			}
			if createCapture.ProviderRequestID != controlCapture.ProviderRequestID {
				t.Fatal("one provider connection emitted inconsistent request diagnostics")
			}
			assertProfileWebSocketDiagnosticHistory(
				t, fixture.store, profile.keyID, createCapture.ProviderRequestID, 1,
			)
			assertWebSocketProfileCredentialBoundary(t, createCapture, profile.subscription)
		})
	}
}
