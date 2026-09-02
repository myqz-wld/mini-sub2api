package integration

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"testing"

	"github.com/coder/websocket"
)

func TestCodexProfilesCommitCompactionOnlyAfterCompletedTerminal(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixtureWithResponder(
		t,
		func(connection *websocket.Conn, payload []byte, responseID string) {
			var request map[string]any
			if json.Unmarshal(payload, &request) != nil {
				return
			}
			eventType := "response.failed"
			if request["model"] == "complete-compaction" {
				eventType = "response.completed"
			}
			_ = connection.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(map[string]any{
				"type": eventType, "response": map[string]any{"id": responseID},
			}))
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
			frames := []string{
				profileCompactionFrame(t, profile.name, "fail-compaction", "turn-one"),
				profileCompactionFrame(t, profile.name, "complete-compaction", "turn-one"),
				profileCompactionFrame(t, profile.name, "complete-compaction", "turn-two"),
			}
			captures := make([]responsesProfileWebSocketCapture, 0, len(frames))
			for _, frame := range frames {
				writeE2EWebSocketText(t, connection, frame)
				if event := readE2EWebSocketText(t, connection); !strings.Contains(event, "response.") {
					t.Fatalf("compaction terminal = %q", event)
				}
				captures = append(
					captures,
					waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0],
				)
			}
			windows := make([]string, 0, len(captures))
			for _, capture := range captures {
				frame := decodeResponsesProfileWebSocketFrame(t, capture.Frame)
				metadata, _ := frame["client_metadata"].(map[string]any)
				window, _ := metadata["x-codex-window-id"].(string)
				windows = append(windows, window)
				assertWebSocketProfileCredentialBoundary(t, capture, profile.subscription)
			}
			if !strings.HasSuffix(windows[0], ":0") || windows[1] != windows[0] ||
				!strings.HasSuffix(windows[2], ":1") {
				t.Fatalf("two-phase compaction windows = %#v", windows)
			}
			if captures[0].ProviderRequestID != captures[1].ProviderRequestID ||
				captures[1].ProviderRequestID != captures[2].ProviderRequestID {
				t.Fatal("one compaction socket changed provider request diagnostic")
			}
			assertProfileWebSocketDiagnosticHistory(
				t, fixture.store, profile.keyID, captures[0].ProviderRequestID, 3,
			)
		})
	}
}

func profileCompactionFrame(t *testing.T, profile, model, turn string) string {
	t.Helper()
	metadata := map[string]any{
		"session_id":   "compaction-session-" + profile,
		"thread_id":    "compaction-session-" + profile,
		"turn_id":      turn,
		"request_kind": "compaction",
		"compaction": map[string]any{
			"trigger": "manual", "implementation": "responses_compaction_v2",
		},
	}
	metadataJSON, err := json.Marshal(metadata)
	if err != nil {
		t.Fatal(err)
	}
	return string(mustRequestJSON(t, map[string]any{
		"type": "response.create", "model": model,
		"input":           []any{map[string]any{"type": "compaction_trigger"}},
		"client_metadata": map[string]any{"x-codex-turn-metadata": string(metadataJSON)},
	}))
}
