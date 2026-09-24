package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestResponsesProfileWebSocketInjectIsOpaqueForAPIKeyAndIgnoredForSubscription(t *testing.T) {
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
						"id": event["response_id"], "usage": map[string]any{"input_tokens": 1, "output_tokens": 0, "total_tokens": 1},
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
		{name: "codex_api_key", secret: fixture.apiKey, headers: codexScenarioHeaders("inject-api", "inject/9.9.9")},
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
			if test.name == "codex_api_key" && createCapture.Headers.Get("Version") != "9.9.9" {
				t.Fatal("API-key WebSocket changed caller version")
			}
			callID := "call_1"
			if test.emulated {
				callID = publicCallID
			}
			inject := ` {"type":"response.inject","response_id":"` + downstreamResponseID +
				`","input":[{"type":"function_call_output","id":"fco_profile","call_id":"` + callID +
				`","output":{"opaque":true,"unknown":true},"unsupported_item":true}],"unsupported_top":true} `
			writeE2EWebSocketText(t, connection, inject)
			if test.emulated {
				select {
				case <-fixture.captures:
					t.Fatal("unsupported inject reached upstream")
				case <-time.After(100 * time.Millisecond):
				}
				return
			}
			if event := readE2EWebSocketText(t, connection); !bytes.Contains([]byte(event), []byte("response.completed")) {
				t.Fatal("inject did not complete the active response")
			}
			captured := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0].Frame
			if !bytes.Equal(captured, []byte(inject)) {
				t.Fatal("API-key inject changed bytes")
			}

		})
	}
}
