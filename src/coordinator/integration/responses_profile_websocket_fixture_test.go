package integration

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/httpapi"
	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func readResponsesProfileModelNotice(t *testing.T, connection *websocket.Conn) {
	t.Helper()
	var event struct {
		Type    string            `json:"type"`
		Headers map[string]string `json:"headers"`
	}
	if json.Unmarshal([]byte(readE2EWebSocketText(t, connection)), &event) != nil ||
		event.Type != "response.metadata" || len(event.Headers) != 1 || event.Headers["openai-model"] != "gpt-loopback-ws" {
		t.Fatal("expected actual handshake model notice before protocol error")
	}
}

func newResponsesProfileWebSocketFixture(t *testing.T) responsesProfileWebSocketFixture {
	return newResponsesProfileWebSocketFixtureWithResponder(t, func(connection *websocket.Conn, payload []byte, responseID string) {
		writeResponsesProfileEvents(connection, responseID, isSyntheticResponsesProfilePrewarm(payload))
	})
}

func newResponsesProfileWebSocketFixtureWithResponder(
	t *testing.T,
	responder responsesProfileWebSocketResponder,
) responsesProfileWebSocketFixture {
	t.Helper()
	return newResponsesProfileWebSocketFixtureWithHandshake(t, responder, nil)
}

// The hook can alter or reject an upstream upgrade without receiving inference frames.
func newResponsesProfileWebSocketFixtureWithHandshake(
	t *testing.T,
	responder responsesProfileWebSocketResponder,
	handshake func(http.ResponseWriter, *http.Request, int) bool,
) responsesProfileWebSocketFixture {
	t.Helper()
	t.Setenv("NO_PROXY", "127.0.0.1,::1")
	t.Setenv("no_proxy", "127.0.0.1,::1")
	coreBinary := findCoreBinary(t)
	captures := make(chan responsesProfileWebSocketCapture, 16)
	var responseNumber int
	var connectionNumber int
	var responseMu sync.Mutex
	upstream := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		responseMu.Lock()
		connectionNumber++
		number := connectionNumber
		providerRequestID := responsesProfileProviderRequestID(number)
		responseMu.Unlock()
		writer.Header().Set("X-Request-Id", providerRequestID)
		writer.Header().Set("Openai-Request-Id", "secondary-"+providerRequestID)
		writer.Header().Set("Openai-Model", "gpt-loopback-ws")
		writer.Header().Set("X-Codex-Installation-Id", "must-not-cross")
		writer.Header().Set("X-Unrecognized-Provider-Extension", "must-not-cross")
		if handshake != nil && !handshake(writer, request, number) {
			return
		}
		connection, err := websocket.Accept(writer, request, &websocket.AcceptOptions{CompressionMode: websocket.CompressionDisabled})
		if err != nil {
			return
		}
		defer connection.CloseNow()
		connection.SetReadLimit(int64(protocolv1.MustInferenceLimits().RequestBytes))
		for {
			messageType, payload, err := connection.Read(context.Background())
			if err != nil || messageType != websocket.MessageText {
				return
			}
			responseMu.Lock()
			responseNumber++
			identifier := responseNumber
			responseMu.Unlock()
			responseID := responsesProfileResponseID(identifier)
			captures <- responsesProfileWebSocketCapture{
				Headers: request.Header.Clone(), Frame: append([]byte(nil), payload...),
				ResponseID: responseID, ProviderRequestID: providerRequestID,
			}
			responder(connection, payload, responseID)
		}
	}))
	t.Cleanup(upstream.Close)
	assertLoopbackURL(t, upstream.URL)

	stateDir := t.TempDir()
	coreStateDir := filepath.Join(stateDir, "core-codex")
	apiMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "add-api-key", "--state-dir", coreStateDir,
		"--upstream-url", upstream.URL + "/responses", "--secret-stdin",
	}, upstreamAPIKey+"\n")
	accountID := "profile-ws-loopback-account"
	authFile := filepath.Join(stateDir, "codex-auth.json")
	authJSON := mustRequestJSON(t, map[string]any{
		"auth_mode": "chatgpt",
		"tokens": map[string]string{
			"id_token": testJWT(&accountID, 3600), "access_token": testJWT(nil, 3600),
			"refresh_token": "not-imported-profile-ws", "account_id": accountID,
		},
	})
	if err := os.WriteFile(authFile, authJSON, 0o600); err != nil {
		t.Fatal(err)
	}
	oauthMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "import-codex-auth", "--state-dir", coreStateDir,
		"--auth-file", authFile, "--issuer", upstream.URL,
		"--client-id", "profile-ws-loopback-client", "--upstream-url", upstream.URL + "/responses",
	}, "")
	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	apiCredential := persistCredential(t, store, "Profile WebSocket API", apiMetadata)
	subscriptionCredential := persistCredential(t, store, "Profile WebSocket subscription", oauthMetadata)
	apiKey := createDownstreamKey(t, store, apiCredential.ID, "Profile WebSocket API client")
	subscriptionKey := createDownstreamKey(t, store, subscriptionCredential.ID, "Profile WebSocket subscription client")
	supervisor, err := adapter.Start(context.Background(), adapter.Config{Binary: coreBinary, StateDir: coreStateDir})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = supervisor.Close() })
	handler := httpapi.NewHandler(store, supervisor, nil)
	public := httptest.NewServer(handler)
	t.Cleanup(func() {
		handler.ShutdownWebSockets()
		public.Close()
	})
	return responsesProfileWebSocketFixture{
		supervisor: supervisor,
		apiKey:     apiKey.Secret, apiKeyID: apiKey.ID,
		subscriptionKey: subscriptionKey.Secret, subscriptionKeyID: subscriptionKey.ID,
		public: public, captures: captures, store: store, coreStateDir: coreStateDir,
	}
}
