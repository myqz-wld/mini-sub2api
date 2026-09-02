package integration

import (
	"context"
	"encoding/json"
	"net/http"
	"testing"

	"github.com/coder/websocket"

	"mini-sub2api/src/coordinator/internal/storage"
)

func TestCodexProfilesTranslateTerminalFailuresWithoutTraversingOpaqueErrors(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixtureWithResponder(
		t,
		func(connection *websocket.Conn, payload []byte, responseID string) {
			var request map[string]any
			if json.Unmarshal(payload, &request) != nil {
				return
			}
			eventType, _ := request["model"].(string)
			switch eventType {
			case "response.failed", "response.incomplete":
				_ = connection.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(map[string]any{
					"type": eventType,
					"response": map[string]any{
						"id": responseID,
						"error": map[string]any{
							"message": "opaque resp_raw conversation_raw",
							"id":      "opaque_nested_id",
						},
					},
				}))
			case "error":
				_ = connection.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(map[string]any{
					"type": "error", "response_id": responseID,
					"error": map[string]any{
						"message": "opaque resp_raw conversation_raw",
						"id":      "opaque_nested_id",
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
		for _, eventType := range []string{"response.failed", "response.incomplete", "error"} {
			t.Run(profile.name+"_"+eventType, func(t *testing.T) {
				connection := dialResponsesProfileWebSocket(
					t, fixture.public, profile.secret,
					http.Header{"Originator": []string{"codex_exec"}},
				)
				defer connection.CloseNow()
				request := mustRequestJSON(t, map[string]any{
					"type": "response.create", "model": eventType,
					"input": []any{}, "client_metadata": map[string]any{
						"session_id": "terminal-" + profile.name + "-" + eventType,
					},
				})
				writeE2EWebSocketText(t, connection, string(request))
				publicText := readE2EWebSocketText(t, connection)
				var public map[string]any
				if json.Unmarshal([]byte(publicText), &public) != nil || public["type"] != eventType {
					t.Fatalf("terminal event = %q", publicText)
				}
				capture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
				if eventType == "error" {
					if public["response_id"] == capture.ResponseID || public["response_id"] == "" {
						t.Fatalf("error response ID was not translated: %#v", public)
					}
				} else {
					response, _ := public["response"].(map[string]any)
					if response["id"] == capture.ResponseID || response["id"] == "" {
						t.Fatalf("terminal response ID was not translated: %#v", public)
					}
				}
				errorObject := terminalErrorObject(public)
				if errorObject["message"] != "opaque resp_raw conversation_raw" ||
					errorObject["id"] != "opaque_nested_id" {
					t.Fatalf("opaque error object changed: %#v", errorObject)
				}
				assertProfileWebSocketDiagnosticHistory(
					t, fixture.store, profile.keyID, capture.ProviderRequestID, 1,
				)
				records, err := fixture.store.History(context.Background(), profile.keyID, nil, 100)
				if err != nil {
					t.Fatal(err)
				}
				var terminalRecord *storage.RequestRecord
				for index := range records {
					record := &records[index]
					if record.ProviderRequestID != nil && *record.ProviderRequestID == capture.ProviderRequestID {
						terminalRecord = record
						break
					}
				}
				if terminalRecord == nil || terminalRecord.Status != storage.RequestUpstreamErr {
					t.Fatalf("terminal failure history = %#v", terminalRecord)
				}
				assertWebSocketProfileCredentialBoundary(t, capture, profile.subscription)
			})
		}
	}
}

func terminalErrorObject(event map[string]any) map[string]any {
	if response, ok := event["response"].(map[string]any); ok {
		errorObject, _ := response["error"].(map[string]any)
		return errorObject
	}
	errorObject, _ := event["error"].(map[string]any)
	return errorObject
}
