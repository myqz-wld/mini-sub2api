package integration

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/coder/websocket"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestCodexProfilesPreserveActiveWebSocketDeliveryAcrossStateOutage(t *testing.T) {
	profiles := []struct {
		name         string
		subscription bool
	}{
		{name: "codex_api_key"},
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
					`{"type":"response.create","model":"gpt-5.4","conversation":"active-state-outage","input":[]}`,
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
				readContext, cancel := context.WithTimeout(context.Background(), 2*time.Second)
				_, _, readErr := connection.Read(readContext)
				cancel()
				if websocket.CloseStatus(readErr) != websocket.StatusCode(protocolv1.FailureCloseCode) {
					t.Fatalf("active state outage close = %v", readErr)
				}
				var closeError websocket.CloseError
				if !errors.As(readErr, &closeError) {
					t.Fatalf("active state outage metadata unavailable: %v", readErr)
				}
				var failure protocolv1.FailureMetadata
				if json.Unmarshal([]byte(closeError.Reason), &failure) != nil ||
					failure.Phase != protocolv1.PhaseInternal {
					t.Fatalf("active state outage failure = %#v / %q", failure, closeError.Reason)
				}
				if observed {
					if failure.RetryAdvice != protocolv1.RetryNever ||
						failure.DeliveryState != protocolv1.DeliveryDelivered {
						t.Fatalf("observed outage authorized replay: %#v", failure)
					}
				} else if failure.RetryAdvice != protocolv1.RetryAmbiguous ||
					failure.DeliveryState != protocolv1.DeliveryPossiblyDelivered {
					t.Fatalf("attempted outage authorized safe replay: %#v", failure)
				}
				select {
				case capture := <-fixture.captures:
					t.Fatalf("state-failing control reached upstream: %#v", capture)
				case <-time.After(200 * time.Millisecond):
				}
				assertProfileWebSocketDiagnosticHistory(
					t, fixture.store, keyID, createCapture.ProviderRequestID, 1,
				)
			})
		}
	}
}
