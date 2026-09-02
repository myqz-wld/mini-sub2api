package integration

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/httpapi"
	"mini-sub2api/src/coordinator/internal/storage"
)

type responsesProfileHTTPFixture struct {
	apiKey                   string
	apiKeyID                 string
	apiCredentialID          string
	subscriptionKey          string
	subscriptionKeyID        string
	subscriptionCredentialID string
	public                   *httptest.Server
	captures                 <-chan routingMatrixCapture
	store                    *storage.Store
	coreBinary               string
	stateDir                 string
	coreStateDir             string
	supervisor               *adapter.Supervisor
	handler                  *httpapi.Handler
}

func newResponsesProfileHTTPFixture(t *testing.T) *responsesProfileHTTPFixture {
	t.Helper()
	t.Setenv("NO_PROXY", "127.0.0.1,::1")
	t.Setenv("no_proxy", "127.0.0.1,::1")
	coreBinary := findCoreBinary(t)
	captures := make(chan routingMatrixCapture, 8)
	upstream := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		body, err := readCapturedUpstreamBody(request)
		if err != nil {
			http.Error(writer, "capture body", http.StatusInternalServerError)
			return
		}
		responseID, providerRequestID := writeLoopbackResponsesResult(writer, body, "resp_profile")
		captures <- routingMatrixCapture{
			Headers: request.Header.Clone(), Body: body,
			ResponseID: responseID, ProviderRequestID: providerRequestID,
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
	accountID := "profile-loopback-account"
	authFile := filepath.Join(stateDir, "codex-auth.json")
	authJSON := mustRequestJSON(t, map[string]any{
		"auth_mode": "chatgpt",
		"tokens": map[string]string{
			"id_token": testJWT(&accountID, 3600), "access_token": testJWT(nil, 3600),
			"refresh_token": "not-imported-profile", "account_id": accountID,
		},
	})
	if err := os.WriteFile(authFile, authJSON, 0o600); err != nil {
		t.Fatal(err)
	}
	oauthMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "import-codex-auth", "--state-dir", coreStateDir,
		"--auth-file", authFile, "--issuer", upstream.URL,
		"--client-id", "profile-loopback-client", "--upstream-url", upstream.URL + "/responses",
	}, "")
	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	apiCredential := persistCredential(t, store, "Profile API key", apiMetadata)
	subscriptionCredential := persistCredential(t, store, "Profile subscription", oauthMetadata)
	apiKey := createDownstreamKey(t, store, apiCredential.ID, "Profile API client")
	subscriptionKey := createDownstreamKey(t, store, subscriptionCredential.ID, "Profile subscription client")
	fixture := &responsesProfileHTTPFixture{
		apiKey: apiKey.Secret, apiKeyID: apiKey.ID, apiCredentialID: apiCredential.ID,
		subscriptionKey: subscriptionKey.Secret, subscriptionKeyID: subscriptionKey.ID,
		subscriptionCredentialID: subscriptionCredential.ID,
		captures:                 captures, store: store, coreBinary: coreBinary,
		stateDir: stateDir, coreStateDir: coreStateDir,
	}
	fixture.startRuntime(t)
	t.Cleanup(fixture.closeRuntime)
	return fixture
}

func (fixture *responsesProfileHTTPFixture) startRuntime(t *testing.T) {
	t.Helper()
	supervisor, err := adapter.Start(context.Background(), adapter.Config{
		Binary: fixture.coreBinary, StateDir: fixture.coreStateDir,
	})
	if err != nil {
		t.Fatal(err)
	}
	fixture.supervisor = supervisor
	fixture.handler = httpapi.NewHandler(fixture.store, supervisor, nil)
	fixture.public = httptest.NewServer(fixture.handler)
}

func (fixture *responsesProfileHTTPFixture) closeRuntime() {
	if fixture.public != nil {
		fixture.public.Close()
		fixture.public = nil
	}
	if fixture.handler != nil {
		fixture.handler.ShutdownWebSockets()
		fixture.handler = nil
	}
	if fixture.supervisor != nil {
		_ = fixture.supervisor.Close()
		fixture.supervisor = nil
	}
}

func (fixture *responsesProfileHTTPFixture) restartRuntime(t *testing.T) {
	t.Helper()
	fixture.closeRuntime()
	fixture.startRuntime(t)
}
