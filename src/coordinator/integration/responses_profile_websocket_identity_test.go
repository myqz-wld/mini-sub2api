package integration

import (
	"context"
	"encoding/json"
	"net/http"
	"testing"
	"time"

	"github.com/coder/websocket"

	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func assertProfileWebSocketHandshakePrivacy(
	t *testing.T,
	response *http.Response,
	deferred bool,
	providerRequestID string,
) {
	t.Helper()
	if response == nil {
		t.Fatal("public WebSocket handshake response is missing")
	}
	gatewayRequestID := response.Header.Get("X-Mini-Sub2Api-Request-Id")
	if gatewayRequestID == "" {
		t.Fatalf("public WebSocket request ID is missing: %#v", response.Header)
	}
	for _, name := range []string{
		protocolv1.ProviderRequestIDHeader,
		"X-Codex-Installation-Id",
		"X-Unrecognized-Provider-Extension",
	} {
		if response.Header.Get(name) != "" {
			t.Fatalf("private WebSocket header %s crossed: %#v", name, response.Header)
		}
	}
	if deferred {
		if response.Header.Get("X-Request-Id") != "" ||
			response.Header.Get("Openai-Request-Id") != "" ||
			response.Header.Get("Openai-Model") != "" {
			t.Fatalf("deferred provider metadata appeared before connect: %#v", response.Header)
		}
		return
	}
	if response.Header.Get("X-Request-Id") != gatewayRequestID ||
		response.Header.Get("Openai-Request-Id") != gatewayRequestID ||
		response.Header.Get("Openai-Model") != "gpt-loopback-ws" ||
		response.Header.Get("X-Request-Id") == providerRequestID {
		t.Fatalf("Bare WebSocket provider aliases = %#v", response.Header)
	}
}

func assertProfileWebSocketDiagnosticHistory(
	t *testing.T,
	store *storage.Store,
	keyID, providerRequestID string,
	want int,
) {
	t.Helper()
	if providerRequestID == "" {
		t.Fatal("upstream WebSocket omitted its provider request ID")
	}
	deadline := time.Now().Add(2 * time.Second)
	for {
		records, err := store.History(context.Background(), keyID, nil, 100)
		if err != nil {
			t.Fatal(err)
		}
		matched := 0
		for _, record := range records {
			if record.ProviderRequestID != nil && *record.ProviderRequestID == providerRequestID {
				if record.Transport != storage.TransportWebSocket ||
					record.Status == storage.RequestInProgress {
					t.Fatalf("provider diagnostic attached to invalid operation: %#v", record)
				}
				matched++
			}
		}
		if matched == want {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf(
				"provider WebSocket diagnostic %q matched %d record(s), want %d: %#v",
				providerRequestID, matched, want, records,
			)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func TestCodexProfilesRestoreWebSocketResponseOwnershipAfterReconnect(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixture(t)
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
			headers := http.Header{"Originator": []string{"codex_exec"}}
			firstConnection := dialResponsesProfileWebSocket(t, fixture.public, profile.secret, headers)
			firstFrame := mustRequestJSON(t, map[string]any{
				"type": "response.create", "model": "gpt-5.4",
				"input": []any{map[string]any{
					"type": "message", "id": "msg-ws-first-" + profile.name,
					"role": "user", "content": "first",
				}},
				"client_metadata": map[string]any{
					"session_id": "ws-session-" + profile.name,
					"thread_id":  "ws-thread-conflict-" + profile.name,
				},
			})
			writeE2EWebSocketText(t, firstConnection, string(firstFrame))
			firstEvents := readResponsesProfileTerminalEvents(t, firstConnection)
			firstCapture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
			firstPublicResponseID := responseIDFromWebSocketEvents(t, firstEvents)
			firstIdentity := identityFromWebSocketCapture(t, firstCapture)
			if firstIdentity.sessionID != firstIdentity.threadID {
				t.Fatalf("conflicting WebSocket roots did not converge: %#v", firstIdentity)
			}
			assertProfileWebSocketDiagnosticHistory(
				t, fixture.store, profile.keyID, firstCapture.ProviderRequestID, 1,
			)
			if err := firstConnection.Close(websocket.StatusNormalClosure, ""); err != nil {
				t.Fatal(err)
			}

			secondConnection := dialResponsesProfileWebSocket(t, fixture.public, profile.secret, headers)
			defer secondConnection.CloseNow()
			secondFrame := mustRequestJSON(t, map[string]any{
				"type": "response.create", "model": "gpt-5.4",
				"previous_response_id": firstPublicResponseID,
				"input": []any{map[string]any{
					"type": "message", "id": "msg-ws-second-" + profile.name,
					"role": "user", "content": "second",
				}},
			})
			writeE2EWebSocketText(t, secondConnection, string(secondFrame))
			secondEvents := readResponsesProfileTerminalEvents(t, secondConnection)
			secondCapture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
			upstreamSecond := decodeResponsesProfileWebSocketFrame(t, secondCapture.Frame)
			if upstreamSecond["previous_response_id"] != firstCapture.ResponseID {
				t.Fatalf("WebSocket response owner was not restored: %#v", upstreamSecond)
			}
			secondIdentity := identityFromWebSocketCapture(t, secondCapture)
			if secondIdentity != firstIdentity {
				t.Fatalf("WebSocket identity changed across reconnect: %#v -> %#v", firstIdentity, secondIdentity)
			}
			if responseIDFromWebSocketEvents(t, secondEvents) == secondCapture.ResponseID {
				t.Fatal("second provider WebSocket response ID crossed publicly")
			}
			if firstCapture.ProviderRequestID == secondCapture.ProviderRequestID {
				t.Fatal("provider WebSocket request ID was reused across connections")
			}
			assertProfileWebSocketDiagnosticHistory(
				t, fixture.store, profile.keyID, secondCapture.ProviderRequestID, 1,
			)
			assertWebSocketProfileCredentialBoundary(t, firstCapture, profile.subscription)
			assertWebSocketProfileCredentialBoundary(t, secondCapture, profile.subscription)
		})
	}
}

func responseIDFromWebSocketEvents(t *testing.T, events []string) string {
	t.Helper()
	for _, encoded := range events {
		var event map[string]any
		if json.Unmarshal([]byte(encoded), &event) != nil {
			continue
		}
		response, _ := event["response"].(map[string]any)
		if responseID, _ := response["id"].(string); responseID != "" {
			return responseID
		}
	}
	t.Fatalf("WebSocket events omitted response ID: %#v", events)
	return ""
}

func identityFromWebSocketCapture(
	t *testing.T,
	capture responsesProfileWebSocketCapture,
) persistedProfileIdentity {
	t.Helper()
	frame := decodeResponsesProfileWebSocketFrame(t, capture.Frame)
	metadata, _ := frame["client_metadata"].(map[string]any)
	identity := persistedProfileIdentity{
		sessionID:      stringValue(metadata["session_id"]),
		threadID:       stringValue(metadata["thread_id"]),
		installationID: stringValue(metadata["x-codex-installation-id"]),
	}
	if !isUUIDVersion(identity.sessionID, '7') || !isUUIDVersion(identity.threadID, '7') ||
		!isUUIDVersion(identity.installationID, '4') || !isUUIDVersion(metadata["turn_id"], '7') {
		t.Fatalf("invalid WebSocket identity = %#v / %#v", identity, metadata)
	}
	return identity
}
