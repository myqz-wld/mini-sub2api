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
			writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.4","input":[],"previous_response_id":"caller-parent"}`)
			if event := readE2EWebSocketText(t, connection); !bytes.Contains([]byte(event), []byte("response.created")) {
				t.Fatal("create did not become active before inject")
			}
			createCapture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
			if test.name == "codex_api_key" && createCapture.Headers.Get("Version") != "0.149.0" {
				t.Fatal("Codex API-key WebSocket profile did not pin version 0.149.0")
			}
			inject := ` {"type":"response.inject","response_id":"` + createCapture.ResponseID +
				`","input":[{"type":"function_call_output","id":"fco_profile","call_id":"call_1","output":{"opaque":true,"unknown":true},"unsupported_item":true}],"unsupported_top":true} `
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
