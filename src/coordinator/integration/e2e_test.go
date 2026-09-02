package integration

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/httpapi"
	"mini-sub2api/src/coordinator/internal/storage"
)

const upstreamAPIKey = "upstream-api-key-e2e-test"

type coreMetadata struct {
	AccountRef        string  `json:"accountRef"`
	AuthKind          string  `json:"authKind"`
	UpstreamAccountID *string `json:"upstreamAccountId"`
}

type mockUpstream struct {
	server        *httptest.Server
	oldAccess     string
	newAccess     string
	newID         string
	refreshes     atomic.Int64
	revokes       atomic.Int64
	revokeEntered chan struct{}
	revokeRelease chan struct{}
	revokeGate    sync.Once
	mu            sync.Mutex
	captures      []capturedRequest
	cancelled     chan struct{}
	cancelledOnce sync.Once
}

func TestCrossLanguageSubscriptionAndAPIKeyService(t *testing.T) {
	t.Setenv("NO_PROXY", "127.0.0.1,::1")
	t.Setenv("no_proxy", "127.0.0.1,::1")
	coreBinary := findCoreBinary(t)
	upstream := newMockUpstream(t)
	defer upstream.server.Close()
	stateDir := t.TempDir()
	coreStateDir := filepath.Join(stateDir, "core-codex")

	apiMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "add-api-key", "--state-dir", coreStateDir,
		"--upstream-url", upstream.server.URL + "/responses", "--secret-stdin",
	}, upstreamAPIKey+"\n")
	oauthMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "login", "--state-dir", coreStateDir, "--flow", "device",
		"--issuer", upstream.server.URL, "--client-id", "client-e2e-test",
		"--upstream-url", upstream.server.URL + "/responses",
	}, "")
	if apiMetadata.AuthKind != "openai_api_key" || oauthMetadata.AuthKind != "codex_oauth" {
		t.Fatalf("credential kinds = %#v / %#v", apiMetadata, oauthMetadata)
	}

	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	apiCredential := persistCredential(t, store, "API", apiMetadata)
	oauthCredential := persistCredential(t, store, "OAuth", oauthMetadata)
	apiKey := createDownstreamKey(t, store, apiCredential.ID, "API client")
	oauthKeyOne := createDownstreamKey(t, store, oauthCredential.ID, "OAuth client one")
	oauthKeyTwo := createDownstreamKey(t, store, oauthCredential.ID, "OAuth client two")

	supervisor, err := adapter.Start(context.Background(), adapter.Config{
		Binary: coreBinary, StateDir: coreStateDir,
	})
	if err != nil {
		t.Fatal(err)
	}
	defer supervisor.Close()
	public := httptest.NewServer(httpapi.NewHandler(store, supervisor, nil))
	defer public.Close()

	apiStatus, apiBody, _ := publicRequestWithHeaders(
		t, public, apiKey.Secret, `{"model":"e2e","stream":false}`,
		http.Header{
			"Accept":                      []string{"application/json"},
			"User-Agent":                  []string{"OpenAI/Go 3.52.0"},
			"OpenAI-Organization":         []string{"org-e2e"},
			"OpenAI-Project":              []string{"proj-e2e"},
			"X-Stainless-Lang":            []string{"go"},
			"X-Stainless-Package-Version": []string{"3.52.0"},
			"X-Stainless-Unreviewed":      []string{"must-not-cross"},
		},
	)
	if apiStatus != http.StatusOK || !strings.Contains(apiBody, `"total_tokens":5`) {
		t.Fatalf("API-key response = %d %q", apiStatus, apiBody)
	}
	oauthStatus, oauthBodyOne, _ := publicRequest(t, public, oauthKeyOne.Secret, `{"model":"e2e","stream":true}`)
	if oauthStatus != http.StatusOK {
		t.Fatalf("OAuth response status = %d", oauthStatus)
	}
	expectedSSE := completedSSE(11)
	if oauthBodyOne != expectedSSE {
		t.Fatalf("OAuth stream changed: %q", oauthBodyOne)
	}
	_, oauthBodyTwo, _ := publicRequest(t, public, oauthKeyTwo.Secret, `{"model":"e2e","stream":true}`)
	if oauthBodyTwo != expectedSSE {
		t.Fatalf("second OAuth stream changed: %q", oauthBodyTwo)
	}
	if upstream.refreshes.Load() != 1 {
		t.Fatalf("refresh calls = %d, want 1", upstream.refreshes.Load())
	}
	assertExactStats(t, store, apiKey.ID, 5)
	assertExactStats(t, store, oauthKeyOne.ID, 11)
	assertExactStats(t, store, oauthKeyTwo.ID, 11)
	upstream.assertAuthBoundaries(t, []string{apiKey.Secret, oauthKeyOne.Secret, oauthKeyTwo.Secret})

	if err := store.RevokeAPIKey(context.Background(), oauthKeyTwo.ID); err != nil {
		t.Fatal(err)
	}
	status, body, _ := publicRequest(t, public, oauthKeyTwo.Secret, `{}`)
	if status != http.StatusUnauthorized || !strings.Contains(body, "invalid_api_key") {
		t.Fatalf("revoked key response = %d %q", status, body)
	}

	testCancellation(t, public, store, apiKey, upstream)
	restartCoreAndWait(t, supervisor, public, apiKey.Secret)
}

func newMockUpstream(t *testing.T) *mockUpstream {
	t.Helper()
	mock := &mockUpstream{
		oldAccess:     testJWT(nil, 3600),
		newAccess:     testJWT(nil, 7200),
		cancelled:     make(chan struct{}),
		revokeEntered: make(chan struct{}),
		revokeRelease: make(chan struct{}),
	}
	account := "chatgpt-e2e-account"
	mock.newID = testJWT(&account, 7200)
	mux := http.NewServeMux()
	mux.HandleFunc("/api/accounts/deviceauth/usercode", func(writer http.ResponseWriter, _ *http.Request) {
		writeJSON(writer, map[string]any{
			"device_auth_id": "device-e2e", "user_code": "E2E-CODE", "interval": "0",
		})
	})
	mux.HandleFunc("/api/accounts/deviceauth/token", func(writer http.ResponseWriter, _ *http.Request) {
		writeJSON(writer, map[string]any{
			"authorization_code": "authorization-e2e", "code_challenge": "challenge-e2e",
			"code_verifier": "verifier-e2e",
		})
	})
	mux.HandleFunc("/oauth/token", mock.tokenHandler)
	mux.HandleFunc("/oauth/revoke", func(writer http.ResponseWriter, _ *http.Request) {
		call := mock.revokes.Add(1)
		if call == 1 {
			mock.revokeGate.Do(func() { close(mock.revokeEntered) })
			<-mock.revokeRelease
		}
		writer.WriteHeader(http.StatusOK)
	})
	mux.HandleFunc("/responses", mock.responsesHandler)
	mock.server = httptest.NewServer(mux)
	assertLoopbackURL(t, mock.server.URL)
	return mock
}

func (m *mockUpstream) tokenHandler(writer http.ResponseWriter, request *http.Request) {
	if strings.HasPrefix(request.Header.Get("Content-Type"), "application/json") {
		var refresh struct {
			ClientID     string `json:"client_id"`
			GrantType    string `json:"grant_type"`
			RefreshToken string `json:"refresh_token"`
		}
		if json.NewDecoder(request.Body).Decode(&refresh) != nil ||
			refresh.ClientID != "client-e2e-test" || refresh.GrantType != "refresh_token" ||
			refresh.RefreshToken != "refresh-old-e2e" {
			http.Error(writer, "invalid refresh request", http.StatusBadRequest)
			return
		}
		m.refreshes.Add(1)
		writeJSON(writer, map[string]any{
			"id_token": m.newID, "access_token": m.newAccess, "refresh_token": "refresh-new-e2e",
		})
		return
	}
	if request.ParseForm() != nil || request.Form.Get("grant_type") != "authorization_code" ||
		request.Form.Get("client_id") != "client-e2e-test" ||
		request.Form.Get("code") != "authorization-e2e" ||
		request.Form.Get("code_verifier") != "verifier-e2e" {
		http.Error(writer, "invalid authorization-code exchange", http.StatusBadRequest)
		return
	}
	writeJSON(writer, map[string]any{
		"id_token":     testJWT(stringPointer("chatgpt-e2e-account"), 3600),
		"access_token": m.oldAccess, "refresh_token": "refresh-old-e2e",
	})
}

func (m *mockUpstream) responsesHandler(writer http.ResponseWriter, request *http.Request) {
	body, err := readCapturedUpstreamBody(request)
	if err != nil {
		http.Error(writer, err.Error(), http.StatusBadRequest)
		return
	}
	capture := capturedRequest{
		Authorization: request.Header.Get("Authorization"),
		AccountID:     request.Header.Get("ChatGPT-Account-ID"),
		Originator:    request.Header.Get("originator"),
		Encoding:      request.Header.Get("Content-Encoding"),
		Organization:  request.Header.Get("OpenAI-Organization"),
		Project:       request.Header.Get("OpenAI-Project"),
		SDKLanguage:   request.Header.Get("X-Stainless-Lang"),
		SDKVersion:    request.Header.Get("X-Stainless-Package-Version"),
		SDKUnreviewed: request.Header.Get("X-Stainless-Unreviewed"),
		Body:          string(body),
	}
	m.mu.Lock()
	m.captures = append(m.captures, capture)
	m.mu.Unlock()
	if capture.Authorization == "Bearer "+m.oldAccess {
		writer.WriteHeader(http.StatusUnauthorized)
		_, _ = io.WriteString(writer, `{"error":{"message":"expired test token"}}`)
		return
	}
	if strings.Contains(capture.Body, `"model":"cancel-e2e"`) {
		writer.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(writer, "data: first\n\n")
		writer.(http.Flusher).Flush()
		<-request.Context().Done()
		m.cancelledOnce.Do(func() { close(m.cancelled) })
		return
	}
	total := int64(0)
	switch capture.Authorization {
	case "Bearer " + upstreamAPIKey:
		total = 5
	case "Bearer " + m.newAccess:
		total = 11
	default:
		writer.WriteHeader(http.StatusUnauthorized)
		return
	}
	if strings.Contains(capture.Body, `"stream":true`) {
		writer.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(writer, completedSSE(total))
		return
	}
	writer.Header().Set("Content-Type", "application/json")
	_, _ = io.WriteString(writer, fmt.Sprintf(
		`{"id":"resp_e2e","usage":{"input_tokens":%d,"output_tokens":1,"total_tokens":%d}}`,
		total-1, total,
	))
}

func (m *mockUpstream) assertAuthBoundaries(t *testing.T, downstreamSecrets []string) {
	t.Helper()
	m.mu.Lock()
	defer m.mu.Unlock()
	for _, capture := range m.captures {
		for _, secret := range downstreamSecrets {
			if strings.Contains(capture.Authorization, secret) || strings.Contains(capture.Body, secret) {
				t.Fatalf("downstream secret crossed upstream boundary")
			}
		}
		if capture.Authorization == "Bearer "+m.oldAccess ||
			capture.Authorization == "Bearer "+m.newAccess {
			if capture.AccountID != "chatgpt-e2e-account" || capture.Originator != "codex-tui" ||
				capture.Encoding != "zstd" {
				t.Fatalf("OAuth headers = %#v", capture)
			}
		} else if capture.Authorization == "Bearer "+upstreamAPIKey {
			if capture.AccountID != "" || capture.Organization != "org-e2e" ||
				capture.Project != "proj-e2e" || capture.SDKLanguage != "go" ||
				capture.SDKVersion != "3.52.0" || capture.SDKUnreviewed != "" ||
				capture.Encoding != "" {
				t.Fatalf("API-key headers = %#v", capture)
			}
		}
	}
}

func createCoreCredential(t *testing.T, binary string, arguments []string, input string) coreMetadata {
	t.Helper()
	command := exec.Command(binary, arguments...)
	command.Stdin = strings.NewReader(input)
	var stdout, stderr bytes.Buffer
	command.Stdout = &stdout
	command.Stderr = &stderr
	if err := command.Run(); err != nil {
		t.Fatalf("core credential command: %v; stderr=%s", err, stderr.String())
	}
	for _, secret := range []string{upstreamAPIKey, "refresh-old-e2e"} {
		if strings.Contains(stdout.String(), secret) || strings.Contains(stderr.String(), secret) {
			t.Fatalf("credential command leaked secret")
		}
	}
	var metadata coreMetadata
	if err := json.Unmarshal(stdout.Bytes(), &metadata); err != nil {
		t.Fatal(err)
	}
	return metadata
}

func persistCredential(t *testing.T, store *storage.Store, name string, metadata coreMetadata) storage.Credential {
	t.Helper()
	credential, err := store.CreateCredential(
		context.Background(), name, "codex", metadata.AuthKind,
		metadata.AccountRef, metadata.UpstreamAccountID,
	)
	if err != nil {
		t.Fatal(err)
	}
	return credential
}

func createDownstreamKey(t *testing.T, store *storage.Store, credentialID, name string) storage.CreatedAPIKey {
	t.Helper()
	key, err := store.CreateAPIKey(context.Background(), credentialID, name)
	if err != nil {
		t.Fatal(err)
	}
	return key
}

func assertExactStats(t *testing.T, store *storage.Store, keyID string, totalTokens int64) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for {
		stats, err := store.Stats(context.Background(), keyID, "", "")
		if err != nil {
			t.Fatal(err)
		}
		if len(stats) == 1 && stats[0].Usage != nil {
			if stats[0].RequestCount != 1 || stats[0].Usage.TotalTokens != totalTokens {
				t.Fatalf("stats for %s = %#v", keyID, stats)
			}
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for stats for %s", keyID)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func testCancellation(
	t *testing.T,
	server *httptest.Server,
	store *storage.Store,
	key storage.CreatedAPIKey,
	upstream *mockUpstream,
) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	request, _ := http.NewRequestWithContext(
		ctx, http.MethodPost, server.URL+"/v1/responses",
		strings.NewReader(`{"model":"cancel-e2e","stream":true}`),
	)
	request.Header.Set("Authorization", "Bearer "+key.Secret)
	response, err := server.Client().Do(request)
	if err != nil {
		t.Fatal(err)
	}
	buffer := make([]byte, 32)
	if _, err := response.Body.Read(buffer); err != nil {
		t.Fatal(err)
	}
	cancel()
	response.Body.Close()
	select {
	case <-upstream.cancelled:
	case <-time.After(3 * time.Second):
		t.Fatal("public cancellation did not reach loopback upstream")
	}
	deadline := time.Now().Add(2 * time.Second)
	for {
		history, err := store.History(context.Background(), key.ID, nil, 100)
		if err != nil {
			t.Fatal(err)
		}
		for _, record := range history {
			if record.Status == storage.RequestDisconnected {
				return
			}
		}
		if time.Now().After(deadline) {
			t.Fatal("disconnected history row was not recorded")
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func restartCoreAndWait(t *testing.T, supervisor *adapter.Supervisor, server *httptest.Server, secret string) {
	t.Helper()
	readiness, ok := supervisor.Readiness()
	if !ok {
		t.Fatal("core is not ready before restart test")
	}
	process, err := os.FindProcess(readiness.PID)
	if err != nil || process.Kill() != nil {
		t.Fatalf("kill core: %v", err)
	}
	deadline := time.Now().Add(8 * time.Second)
	for {
		status, _, _ := publicRequest(t, server, secret, `{"model":"restart-e2e","stream":false}`)
		if status == http.StatusOK {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("core did not recover; last status %d", status)
		}
		time.Sleep(50 * time.Millisecond)
	}
}

func completedSSE(total int64) string {
	return fmt.Sprintf(
		"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":%d,\"output_tokens\":1,\"total_tokens\":%d}}}\n\n",
		total-1, total,
	)
}

func writeJSON(writer http.ResponseWriter, value any) {
	writer.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(writer).Encode(value)
}

func testJWT(accountID *string, expiresIn int64) string {
	claims := map[string]any{"exp": time.Now().Unix() + expiresIn}
	if accountID != nil {
		claims["https://api.openai.com/auth"] = map[string]string{"chatgpt_account_id": *accountID}
	}
	payload, _ := json.Marshal(claims)
	return "test." + base64.RawURLEncoding.EncodeToString(payload) + ".signature"
}

func stringPointer(value string) *string { return &value }

func assertLoopbackURL(t *testing.T, raw string) {
	t.Helper()
	parsed, err := url.Parse(raw)
	if err != nil {
		t.Fatal(err)
	}
	ip := net.ParseIP(parsed.Hostname())
	if ip == nil || !ip.IsLoopback() {
		t.Fatalf("integration endpoint is not loopback: %s", raw)
	}
}
