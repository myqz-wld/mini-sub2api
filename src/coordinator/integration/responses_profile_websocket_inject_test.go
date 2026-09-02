package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"testing"

	"github.com/coder/websocket"
)

func TestResponsesProfileWebSocketInjectUsesProfileFiltering(t *testing.T) {
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
						"type": "function_call", "id": "fc-inject-provider",
						"call_id": "call-inject-provider", "name": "lookup", "arguments": "{}",
					},
				}))
			case "response.inject":
				_ = connection.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(map[string]any{
					"type": "response.completed", "response": map[string]any{
						"id": responseID, "usage": map[string]any{"input_tokens": 1, "output_tokens": 0, "total_tokens": 1},
					},
				}))
			}
		},
	)
	for _, test := range []struct {
		name     string
		secret   string
		headers  http.Header
		emulated bool
	}{
		{name: "bare_api_key", secret: fixture.apiKey},
		{name: "codex_api_key", secret: fixture.apiKey, headers: codexScenarioHeaders("inject-api", "inject/9.9.9"), emulated: true},
		{name: "bare_subscription", secret: fixture.subscriptionKey, emulated: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			connection := dialResponsesProfileWebSocket(t, fixture.public, test.secret, test.headers)
			defer connection.CloseNow()
			writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.4","generate":true,"input":[]}`)
			createdEvent := readE2EWebSocketText(t, connection)
			if !bytes.Contains([]byte(createdEvent), []byte("response.created")) {
				t.Fatal("create did not become active before inject")
			}
			var created map[string]any
			if json.Unmarshal([]byte(createdEvent), &created) != nil {
				t.Fatal("created event was not JSON")
			}
			downstreamResponseID, _ := created["response"].(map[string]any)["id"].(string)
			if downstreamResponseID == "" {
				t.Fatal("created event omitted response id")
			}
			callEventText := readE2EWebSocketText(t, connection)
			var callEvent map[string]any
			if json.Unmarshal([]byte(callEventText), &callEvent) != nil {
				t.Fatal("function call event was not JSON")
			}
			callItem, _ := callEvent["item"].(map[string]any)
			publicCallID, _ := callItem["call_id"].(string)
			if test.emulated && (publicCallID == "" || publicCallID == "call-inject-provider") {
				t.Fatalf("function call ID was not translated: %#v", callEvent)
			}
			createCapture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
			if test.name == "codex_api_key" && createCapture.Headers.Get("Version") != "0.149.0" {
				t.Fatal("Codex API-key WebSocket profile did not pin version 0.149.0")
			}
			callID := "call_1"
			if test.emulated {
				callID = publicCallID
			}
			inject := ` {"type":"response.inject","response_id":"` + downstreamResponseID +
				`","input":[{"type":"function_call_output","id":"fco_profile","call_id":"` + callID +
				`","output":{"opaque":true,"unknown":true},"unsupported_item":true}],"unsupported_top":true} `
			writeE2EWebSocketText(t, connection, inject)
			if event := readE2EWebSocketText(t, connection); !bytes.Contains([]byte(event), []byte("response.completed")) {
				t.Fatal("inject did not complete the active response")
			}
			captured := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0].Frame
			if !test.emulated {
				if !bytes.Equal(captured, []byte(inject)) {
					t.Fatal("bare response.inject changed bytes")
				}
				return
			}
			value := decodeResponsesProfileWebSocketFrame(t, captured)
			if value["type"] != "response.inject" || value["response_id"] != createCapture.ResponseID {
				t.Fatal("emulated response.inject lost documented carriers")
			}
			if _, exists := value["unsupported_top"]; exists {
				t.Fatal("emulated response.inject kept an unsupported top-level field")
			}
			input := value["input"].([]any)[0].(map[string]any)
			if input["call_id"] != "call-inject-provider" {
				t.Fatalf("emulated response.inject did not restore call ID: %#v", input)
			}
			if _, exists := input["unsupported_item"]; exists {
				t.Fatal("emulated response.inject kept an unsupported item field")
			}
			output := input["output"].(map[string]any)
			if output["opaque"] != true || output["unknown"] != true {
				t.Fatal("emulated response.inject changed an opaque function output payload")
			}
		})
	}
}
