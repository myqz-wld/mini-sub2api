package integration

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestIgnoredWebSocketControlsDoNotAccessUnavailableState(t *testing.T) {
	profiles := []struct {
		name         string
		subscription bool
	}{
		{name: "codex_subscription", subscription: true},
	}
	for _, profile := range profiles {
		for _, observed := range []bool{false, true} {
			phase := "attempted"
			if observed {
				phase = "observed"
			}
			t.Run(profile.name+"_"+phase, func(t *testing.T) {
				fixture := newResponsesProfileWebSocketFixtureWithResponder(
					t,
					func(connection *websocket.Conn, _ []byte, responseID string) {
						if observed {
							_ = connection.Write(
								context.Background(), websocket.MessageText,
								mustRequestJSONValue(map[string]any{
									"type":     "response.created",
									"response": map[string]any{"id": responseID},
								}),
							)
						}
					},
				)
				secret, keyID := fixture.apiKey, fixture.apiKeyID
				if profile.subscription {
					secret, keyID = fixture.subscriptionKey, fixture.subscriptionKeyID
				}
				connection := dialResponsesProfileWebSocket(
					t, fixture.public, secret,
					http.Header{"Originator": []string{"codex_exec"}},
				)
				defer connection.CloseNow()
				writeE2EWebSocketText(
					t, connection,
					`{"type":"response.create","model":"gpt-5.4","generate":true,"input":[],"client_metadata":{"session_id":"active-state-outage"}}`,
				)
				createCapture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
				responseID := "response-not-yet-observed"
				if observed {
					created := readE2EWebSocketText(t, connection)
					var event map[string]any
					if json.Unmarshal([]byte(created), &event) != nil {
						t.Fatalf("created event = %q", created)
					}
					response, _ := event["response"].(map[string]any)
					responseID, _ = response["id"].(string)
					if responseID == "" {
						t.Fatal("created event omitted response alias")
					}
				}
				stateFiles, err := filepath.Glob(filepath.Join(
					fixture.coreStateDir, "accounts", "rs_*.request-state.json",
				))
				if err != nil || len(stateFiles) != 1 {
					t.Fatalf("request-state files = %#v, %v", stateFiles, err)
				}
				if err := os.WriteFile(stateFiles[0], []byte("{corrupt"), 0o600); err != nil {
					t.Fatal(err)
				}
				control := mustRequestJSON(t, map[string]any{
					"type": "response.inject", "response_id": responseID,
					"input": []any{map[string]any{
						"type": "function_call_output", "call_id": "call-state-outage",
						"output": "done",
					}},
				})
				writeE2EWebSocketText(t, connection, string(control))
				readContext, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
				_, _, readErr := connection.Read(readContext)
				if readContext.Err() != context.DeadlineExceeded {
					t.Fatalf("ignored control unexpectedly terminated connection: %v", readErr)
				}
				cancel()
				select {
				case capture := <-fixture.captures:
					_ = capture
					t.Fatal("ignored control reached upstream")
				case <-time.After(200 * time.Millisecond):
				}
				assertProfileWebSocketDiagnosticHistory(
					t, fixture.store, keyID, createCapture.ProviderRequestID, 1,
				)
			})
		}
	}
}
