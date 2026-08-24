package httpapi

import (
	"context"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/coder/websocket"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestWebSocketHandshakeDialsCoreFirstAndTerminatesCompressionPublicly(t *testing.T) {
	var captured http.Header
	var captureMu sync.Mutex
	coreServer := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		captureMu.Lock()
		captured = request.Header.Clone()
		captureMu.Unlock()
		writer.Header().Set("Openai-Model", "test-model")
		writer.Header().Set("X-Models-Etag", "etag-test")
		writer.Header().Set("Set-Cookie", "must-not-cross=1")
		writer.Header().Set(protocolv1.CoreTTFBHeader, "7")
		connection, err := websocket.Accept(writer, request, &websocket.AcceptOptions{
			CompressionMode: websocket.CompressionDisabled,
		})
		if err != nil {
			return
		}
		defer connection.CloseNow()
		messageType, payload, err := connection.Read(context.Background())
		if err != nil || messageType != websocket.MessageText {
			return
		}
		_ = connection.Write(context.Background(), websocket.MessageText, []byte(
			`{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}}`,
		))
		_ = payload
	}))
	t.Cleanup(coreServer.Close)
	store, _, key := setupHTTPTest(t)
	core := &loopbackWebSocketCore{url: coreServer.URL}
	_, publicServer := startPublicWebSocketServer(t, store, core)

	connection, response, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, http.Header{
		"Openai-Beta":        []string{"responses_websockets=2026-02-06"},
		"User-Agent":         []string{"codex_exec/test"},
		"Version":            []string{"0.149.0"},
		"X-Openai-Subagent":  []string{"review"},
		"X-Forwarded-For":    []string{"203.0.113.40"},
		"X-Stainless-Secret": []string{"must-not-cross"},
	})
	if err != nil {
		t.Fatal(err)
	}
	defer connection.CloseNow()
	if response.StatusCode != http.StatusSwitchingProtocols ||
		!strings.Contains(response.Header.Get("Sec-WebSocket-Extensions"), "permessage-deflate") {
		t.Fatalf("public handshake = %d/%#v", response.StatusCode, response.Header)
	}
	if response.Header.Get("Openai-Model") != "test-model" ||
		response.Header.Get("X-Models-Etag") != "etag-test" ||
		response.Header.Get("Server-Timing") != "upstream_ttfb;dur=7" {
		t.Fatalf("safe upgrade metadata = %#v", response.Header)
	}
	if response.Header.Get("Set-Cookie") != "" || response.Header.Get(protocolv1.CoreTTFBHeader) != "" {
		t.Fatalf("unsafe upgrade metadata crossed: %#v", response.Header)
	}
	writeWebSocketText(t, connection, `{"type":"response.create","model":"test"}`)
	if event := readWebSocketText(t, connection); !strings.Contains(event, "response.completed") {
		t.Fatalf("event = %q", event)
	}

	captureMu.Lock()
	defer captureMu.Unlock()
	if captured.Get("Authorization") != "" || captured.Get("X-Forwarded-For") != "" ||
		captured.Get("X-Stainless-Secret") != "" || captured.Get("Sec-WebSocket-Extensions") != "" {
		t.Fatalf("internal headers = %#v", captured)
	}
	if captured.Get("Openai-Beta") != "responses_websockets=2026-02-06" ||
		captured.Get("User-Agent") != "codex_exec/test" || captured.Get("Version") != "0.149.0" ||
		captured.Get("X-Openai-Subagent") != "review" {
		t.Fatalf("missing internal metadata: %#v", captured)
	}
}

type rejectingWebSocketCore struct{}

func (*rejectingWebSocketCore) Forward(
	context.Context, string, string, http.Header, []byte,
) (*http.Response, error) {
	return nil, errors.New("HTTP forwarding is not expected")
}

func (*rejectingWebSocketCore) DialWebSocket(
	context.Context, string, string, http.Header,
) (*websocket.Conn, *http.Response, error) {
	return nil, &http.Response{
		StatusCode: http.StatusUpgradeRequired,
		Header: http.Header{
			"Content-Type": []string{"application/json"},
			"Set-Cookie":   []string{"must-not-cross=1"},
		},
		Body: io.NopCloser(strings.NewReader(`{"error":{"code":"websocket_required"}}`)),
	}, errors.New("upstream rejected handshake")
}

func TestWebSocketUpstreamRejectionRemainsHTTP(t *testing.T) {
	store, _, key := setupHTTPTest(t)
	_, publicServer := startPublicWebSocketServer(t, store, &rejectingWebSocketCore{})
	connection, response, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
	if err == nil || connection != nil {
		t.Fatal("rejected handshake unexpectedly upgraded")
	}
	if response == nil || response.StatusCode != http.StatusUpgradeRequired {
		t.Fatalf("response = %#v, %v", response, err)
	}
	if response.Header.Get("Set-Cookie") != "" ||
		responseBody(t, response) != `{"error":{"code":"websocket_required"}}` {
		t.Fatalf("rejection = %#v", response)
	}
}

func TestWebSocketAuthenticationAndOriginFailuresNeverDialCore(t *testing.T) {
	store, credential, key := setupHTTPTest(t)
	core := &countingWebSocketCore{}
	_, publicServer := startPublicWebSocketServer(t, store, core)

	_, invalidResponse, invalidErr := dialPublicWebSocket(
		t, publicServer.URL, "ms2a_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", nil,
	)
	if invalidErr == nil || invalidResponse.StatusCode != http.StatusUnauthorized {
		t.Fatalf("invalid key = %#v, %v", invalidResponse, invalidErr)
	}
	invalidBody := responseBody(t, invalidResponse)
	if err := store.RevokeAPIKey(context.Background(), key.ID); err != nil {
		t.Fatal(err)
	}
	_, revokedResponse, revokedErr := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
	if revokedErr == nil || revokedResponse.StatusCode != http.StatusUnauthorized {
		t.Fatalf("revoked key = %#v, %v", revokedResponse, revokedErr)
	}
	if revokedBody := responseBody(t, revokedResponse); revokedBody != invalidBody {
		t.Fatalf("generic failures differ: %q / %q", invalidBody, revokedBody)
	}
	second, err := store.CreateAPIKey(context.Background(), credential.ID, "Origin test")
	if err != nil {
		t.Fatal(err)
	}
	_, originResponse, originErr := dialPublicWebSocket(t, publicServer.URL, second.Secret, http.Header{
		"Origin": []string{"https://untrusted.example"},
	})
	if originErr == nil || originResponse.StatusCode != http.StatusForbidden {
		t.Fatalf("origin response = %#v, %v", originResponse, originErr)
	}
	request, err := http.NewRequest(http.MethodGet, publicServer.URL+"/v1/responses", nil)
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("Authorization", "Bearer "+second.Secret)
	malformedResponse, err := publicServer.Client().Do(request)
	if err != nil {
		t.Fatal(err)
	}
	malformedResponse.Body.Close()
	if malformedResponse.StatusCode != http.StatusUpgradeRequired {
		t.Fatalf("malformed handshake status = %d", malformedResponse.StatusCode)
	}
	core.mu.Lock()
	defer core.mu.Unlock()
	if core.calls != 0 {
		t.Fatalf("core called %d times", core.calls)
	}
}

func TestWebSocketManagerEnforcesEightConnectionsPerKey(t *testing.T) {
	manager := newWebSocketManager(8)
	for index := 0; index < 8; index++ {
		if !manager.acquire("key_one") {
			t.Fatalf("connection %d was rejected", index+1)
		}
	}
	if manager.acquire("key_one") {
		t.Fatal("ninth connection was accepted")
	}
	if !manager.acquire("key_two") {
		t.Fatal("another key shared the first key's limit")
	}
	manager.release("key_one")
	if !manager.acquire("key_one") {
		t.Fatal("released slot was not reusable")
	}
}
