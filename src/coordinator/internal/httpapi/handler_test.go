package httpapi

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

type fakeCore struct {
	mu       sync.Mutex
	calls    int
	headers  http.Header
	body     []byte
	response func(context.Context, string) (*http.Response, error)
}

func (c *fakeCore) Forward(
	ctx context.Context,
	_ string,
	requestID string,
	headers http.Header,
	body []byte,
) (*http.Response, error) {
	c.mu.Lock()
	c.calls++
	c.headers = headers.Clone()
	c.body = append([]byte(nil), body...)
	c.mu.Unlock()
	return c.response(ctx, requestID)
}

func TestResponsesStreamsAndRecordsUsageByDownstreamKey(t *testing.T) {
	store, credential, key := setupHTTPTest(t)
	sse := "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n" +
		"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{" +
		"\"input_tokens\":11,\"input_tokens_details\":{\"cached_tokens\":4}," +
		"\"output_tokens\":5,\"output_tokens_details\":{\"reasoning_tokens\":2},\"total_tokens\":16}}}\n\n"
	core := &fakeCore{response: func(context.Context, string) (*http.Response, error) {
		headers := make(http.Header)
		headers.Set("Content-Type", "text/event-stream")
		headers.Set("Server-Timing", "provider;dur=1")
		headers.Set(protocolv1.CoreTTFBHeader, "12")
		headers.Set("Set-Cookie", "must-not-cross=1")
		headers.Set("X-Mini-Sub2Api-Forged-Upstream", "no")
		headers.Set("Connection", "X-Hop-Test")
		headers.Set("X-Hop-Test", "must-not-cross")
		return &http.Response{
			StatusCode: http.StatusOK,
			Header:     headers,
			Body: &chunkReader{chunks: [][]byte{
				[]byte(sse[:37]), []byte(sse[37:111]), []byte(sse[111:]),
			}},
		}, nil
	}}
	server := httptest.NewServer(NewHandler(store, core, nil))
	t.Cleanup(server.Close)
	requestBody := []byte(`{"model":"test","stream":true}`)
	request, err := http.NewRequest(http.MethodPost, server.URL+"/v1/responses", bytes.NewReader(requestBody))
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("Authorization", "Bearer "+key.Secret)
	request.Header.Set("Accept", "text/event-stream")
	request.Header.Set("Content-Encoding", "zstd")
	request.Header.Set("Content-Type", "application/json")
	request.Header.Set("Originator", "codex_exec")
	request.Header.Set("Session-Id", "session-test")
	request.Header.Set("Thread-Id", "thread-test")
	request.Header.Set("X-Codex-Turn-State", "turn-test")
	request.Header.Set("X-OpenAI-Internal-Codex-Responses-Lite", "true")
	request.Header.Set("X-Forwarded-For", "203.0.113.1")
	response, err := server.Client().Do(request)
	if err != nil {
		t.Fatal(err)
	}
	responseBody, err := io.ReadAll(response.Body)
	response.Body.Close()
	if err != nil {
		t.Fatal(err)
	}
	if string(responseBody) != sse {
		t.Fatalf("stream changed\ngot:  %q\nwant: %q", responseBody, sse)
	}
	if requestID := response.Header.Get("X-Mini-Sub2Api-Request-Id"); !strings.HasPrefix(requestID, "req_") {
		t.Fatalf("request id = %q", requestID)
	}
	if timing := response.Header.Get("Server-Timing"); timing != "provider;dur=1, upstream_ttfb;dur=12" {
		t.Fatalf("Server-Timing = %q", timing)
	}
	if response.Header.Get("Set-Cookie") != "" || response.Header.Get(protocolv1.CoreTTFBHeader) != "" ||
		response.Header.Get("X-Hop-Test") != "" {
		t.Fatalf("unsafe response headers crossed: %#v", response.Header)
	}
	core.mu.Lock()
	if core.calls != 1 || !bytes.Equal(core.body, requestBody) {
		t.Fatalf("core calls/body = %d/%q", core.calls, core.body)
	}
	if core.headers.Get("Authorization") != "" || core.headers.Get("X-Forwarded-For") != "" {
		t.Fatalf("forwarded headers = %#v", core.headers)
	}
	for name, expected := range map[string]string{
		"Accept": "text/event-stream", "Content-Encoding": "zstd",
		"Content-Type": "application/json", "Originator": "codex_exec",
		"Session-Id": "session-test", "Thread-Id": "thread-test",
		"X-Codex-Turn-State":                     "turn-test",
		"X-OpenAI-Internal-Codex-Responses-Lite": "true",
	} {
		if got := core.headers.Get(name); got != expected {
			t.Fatalf("forwarded %s = %q, want %q", name, got, expected)
		}
	}
	core.mu.Unlock()

	stats := waitForStats(t, store, key.ID)
	if len(stats) != 1 || stats[0].APIKeyID != key.ID || stats[0].Usage == nil ||
		stats[0].Usage.TotalTokens != 16 || stats[0].Usage.ReasoningOutputTokens != 2 {
		t.Fatalf("stats = %#v (credential %s)", stats, credential.ID)
	}
}

func TestInvalidAndRevokedKeysShareGenericFailure(t *testing.T) {
	store, _, key := setupHTTPTest(t)
	core := &fakeCore{response: func(context.Context, string) (*http.Response, error) {
		return nil, errors.New("must not be called")
	}}
	server := httptest.NewServer(NewHandler(store, core, nil))
	t.Cleanup(server.Close)
	invalidBody := callPublic(t, server, "ms2a_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
	if err := store.RevokeAPIKey(context.Background(), key.ID); err != nil {
		t.Fatal(err)
	}
	revokedBody := callPublic(t, server, key.Secret)
	if invalidBody != revokedBody || !strings.Contains(invalidBody, "invalid_api_key") {
		t.Fatalf("generic failures differ: %q / %q", invalidBody, revokedBody)
	}
	core.mu.Lock()
	defer core.mu.Unlock()
	if core.calls != 0 {
		t.Fatalf("core called %d times", core.calls)
	}
}

func TestOnlyExactResponsesRouteIsPublic(t *testing.T) {
	store, _, _ := setupHTTPTest(t)
	core := &fakeCore{response: func(context.Context, string) (*http.Response, error) {
		return nil, errors.New("must not be called")
	}}
	server := httptest.NewServer(NewHandler(store, core, nil))
	t.Cleanup(server.Close)
	for _, target := range []string{"/v1/responses?debug=1", "/v1/chat/completions", "/health"} {
		request, _ := http.NewRequest(http.MethodPost, server.URL+target, strings.NewReader(`{}`))
		response, err := server.Client().Do(request)
		if err != nil {
			t.Fatal(err)
		}
		response.Body.Close()
		if response.StatusCode != http.StatusNotFound {
			t.Fatalf("%s status = %d", target, response.StatusCode)
		}
	}
}

func TestCoreRequiresLoginIsMappedAndCredentialIsMarked(t *testing.T) {
	store, credential, key := setupHTTPTest(t)
	core := &fakeCore{response: func(_ context.Context, requestID string) (*http.Response, error) {
		body, _ := json.Marshal(protocolv1.ErrorEnvelope{Error: protocolv1.CoreError{
			Code: "credential_requires_login", Message: "The selected credential requires sign-in.",
			RequestID: requestID,
		}})
		return &http.Response{
			StatusCode: http.StatusUnauthorized,
			Header:     http.Header{"Content-Type": []string{"application/json"}},
			Body:       io.NopCloser(bytes.NewReader(body)),
		}, nil
	}}
	server := httptest.NewServer(NewHandler(store, core, nil))
	t.Cleanup(server.Close)
	body := callPublic(t, server, key.Secret)
	if !strings.Contains(body, `"code":"credential_requires_login"`) || strings.Contains(body, "retryable") {
		t.Fatalf("public error = %q", body)
	}
	stored, err := store.Credential(context.Background(), credential.ID)
	if err != nil {
		t.Fatal(err)
	}
	if stored.Status != storage.CredentialRequiresLogin {
		t.Fatalf("credential status = %q", stored.Status)
	}
}

func TestClientCancellationClosesCoreBodyAndRecordsPartial(t *testing.T) {
	store, _, key := setupHTTPTest(t)
	closed := make(chan struct{})
	core := &fakeCore{response: func(ctx context.Context, _ string) (*http.Response, error) {
		return &http.Response{
			StatusCode: http.StatusOK,
			Header:     http.Header{"Content-Type": []string{"text/event-stream"}},
			Body:       &cancellationBody{ctx: ctx, closed: closed},
		}, nil
	}}
	server := httptest.NewServer(NewHandler(store, core, nil))
	t.Cleanup(server.Close)
	ctx, cancel := context.WithCancel(context.Background())
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, server.URL+"/v1/responses", strings.NewReader(`{}`))
	if err != nil {
		t.Fatal(err)
	}
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
	case <-closed:
	case <-time.After(2 * time.Second):
		t.Fatal("core response body was not closed")
	}
	stats := waitForStats(t, store, key.ID)
	if len(stats) != 1 || stats[0].DisconnectedCount != 1 || stats[0].Usage != nil {
		t.Fatalf("cancellation stats = %#v", stats)
	}
}

func setupHTTPTest(t *testing.T) (*storage.Store, storage.Credential, storage.CreatedAPIKey) {
	t.Helper()
	store, err := storage.Open(context.Background(), t.TempDir(), time.Now)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	credential, err := store.CreateCredential(
		context.Background(), "Test credential", "codex", "openai_api_key", "acct_http_test", nil,
	)
	if err != nil {
		t.Fatal(err)
	}
	key, err := store.CreateAPIKey(context.Background(), credential.ID, "Test key")
	if err != nil {
		t.Fatal(err)
	}
	return store, credential, key
}

func callPublic(t *testing.T, server *httptest.Server, secret string) string {
	t.Helper()
	request, err := http.NewRequest(http.MethodPost, server.URL+"/v1/responses", strings.NewReader(`{}`))
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("Authorization", "Bearer "+secret)
	response, err := server.Client().Do(request)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusUnauthorized {
		t.Fatalf("status = %d", response.StatusCode)
	}
	body, err := io.ReadAll(response.Body)
	if err != nil {
		t.Fatal(err)
	}
	return string(body)
}

func waitForStats(t *testing.T, store *storage.Store, keyID string) []storage.DailyUsage {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for {
		stats, err := store.Stats(context.Background(), keyID, "", "")
		if err != nil {
			t.Fatal(err)
		}
		if len(stats) > 0 {
			return stats
		}
		if time.Now().After(deadline) {
			t.Fatal("timed out waiting for usage stats")
		}
		time.Sleep(10 * time.Millisecond)
	}
}

type chunkReader struct {
	chunks [][]byte
}

func (r *chunkReader) Read(destination []byte) (int, error) {
	if len(r.chunks) == 0 {
		return 0, io.EOF
	}
	chunk := r.chunks[0]
	r.chunks = r.chunks[1:]
	return copy(destination, chunk), nil
}

func (*chunkReader) Close() error { return nil }

type cancellationBody struct {
	ctx    context.Context
	closed chan struct{}
	once   sync.Once
	first  bool
}

func (b *cancellationBody) Read(destination []byte) (int, error) {
	if !b.first {
		b.first = true
		return copy(destination, []byte("data: first\n\n")), nil
	}
	<-b.ctx.Done()
	return 0, b.ctx.Err()
}

func (b *cancellationBody) Close() error {
	b.once.Do(func() { close(b.closed) })
	return nil
}
