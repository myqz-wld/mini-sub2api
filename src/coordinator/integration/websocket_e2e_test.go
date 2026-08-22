package integration

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/httpapi"
	"mini-sub2api/src/coordinator/internal/storage"
)

type websocketE2EUpstream struct {
	server  *httptest.Server
	mu      sync.Mutex
	headers http.Header
	frames  []string
}

func TestCrossLanguageWebSocketPassthroughAndHTTPIndependence(t *testing.T) {
	t.Setenv("NO_PROXY", "127.0.0.1,::1")
	t.Setenv("no_proxy", "127.0.0.1,::1")
	coreBinary := findCoreBinary(t)
	upstream := newWebSocketE2EUpstream(t)
	defer upstream.server.Close()
	stateDir := t.TempDir()
	coreStateDir := filepath.Join(stateDir, "core-codex")
	metadata := createCoreCredential(t, coreBinary, []string{
		"credential", "add-api-key", "--state-dir", coreStateDir,
		"--upstream-url", upstream.server.URL + "/responses", "--secret-stdin",
	}, upstreamAPIKey+"\n")
	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	credential := persistCredential(t, store, "WebSocket API", metadata)
	websocketKey := createDownstreamKey(t, store, credential.ID, "WebSocket client")
	httpKey := createDownstreamKey(t, store, credential.ID, "HTTP client")
	supervisor, err := adapter.Start(context.Background(), adapter.Config{
		Binary: coreBinary, StateDir: coreStateDir,
	})
	if err != nil {
		t.Fatal(err)
	}
	defer supervisor.Close()
	handler := httpapi.NewHandler(store, supervisor, nil)
	public := httptest.NewServer(handler)
	defer func() {
		handler.ShutdownWebSockets()
		public.Close()
	}()

	connection, response, err := websocket.Dial(
		context.Background(), public.URL+"/v1/responses",
		&websocket.DialOptions{
			HTTPHeader: http.Header{
				"Authorization":     []string{"Bearer " + websocketKey.Secret},
				"Openai-Beta":       []string{"responses_websockets=2026-02-06"},
				"User-Agent":        []string{"codex_exec/e2e"},
				"X-Forwarded-For":   []string{"203.0.113.50"},
				"X-Stainless-Other": []string{"must-not-cross"},
			},
			CompressionMode: websocket.CompressionContextTakeover,
		},
	)
	if err != nil {
		t.Fatal(err)
	}
	defer connection.CloseNow()
	if response.StatusCode != http.StatusSwitchingProtocols ||
		!strings.Contains(response.Header.Get("Sec-WebSocket-Extensions"), "permessage-deflate") ||
		response.Header.Get("X-Models-Etag") != "e2e-etag" {
		t.Fatalf("public handshake = %d/%#v", response.StatusCode, response.Header)
	}

	prewarm := `{"type":"response.create","model":"prewarm","generate":false}`
	inference := `{"type":"response.create","model":"e2e","previous_response_id":"resp_warm"}`
	for _, frame := range []string{prewarm, inference} {
		writeE2EWebSocketText(t, connection, frame)
		if event := readE2EWebSocketText(t, connection); !strings.Contains(event, "response.created") {
			t.Fatalf("first event = %q", event)
		}
		if event := readE2EWebSocketText(t, connection); !strings.Contains(event, "response.completed") {
			t.Fatalf("terminal event = %q", event)
		}
	}
	assertExactStats(t, store, websocketKey.ID, 9)
	history := waitForE2EHistory(t, store, websocketKey.ID, 2)
	seenPrewarm, seenInference := false, false
	for _, record := range history {
		if record.Transport != storage.TransportWebSocket || record.Status != storage.RequestCompleted {
			t.Fatalf("WebSocket history = %#v", history)
		}
		seenPrewarm = seenPrewarm || record.OperationKind == storage.OperationWebSocketPrewarm
		seenInference = seenInference || record.OperationKind == storage.OperationInference
	}
	if !seenPrewarm || !seenInference {
		t.Fatalf("operation kinds = %#v", history)
	}

	status, body, _ := publicRequest(t, public, httpKey.Secret, `{"model":"http-e2e","stream":false}`)
	if status != http.StatusOK || !strings.Contains(body, `"total_tokens":4`) {
		t.Fatalf("HTTP fallback path = %d %q", status, body)
	}
	assertExactStats(t, store, httpKey.ID, 4)

	upstream.mu.Lock()
	defer upstream.mu.Unlock()
	if upstream.headers.Get("Authorization") != "Bearer "+upstreamAPIKey ||
		upstream.headers.Get("Openai-Beta") != "responses_websockets=2026-02-06" ||
		upstream.headers.Get("Sec-WebSocket-Extensions") != "" ||
		upstream.headers.Get("X-Forwarded-For") != "" ||
		upstream.headers.Get("X-Stainless-Other") != "" {
		t.Fatalf("upstream headers = %#v", upstream.headers)
	}
	if len(upstream.frames) != 2 || upstream.frames[0] != prewarm || upstream.frames[1] != inference {
		t.Fatalf("upstream frames = %#v", upstream.frames)
	}
	for _, frame := range upstream.frames {
		if strings.Contains(frame, websocketKey.Secret) || strings.Contains(frame, httpKey.Secret) {
			t.Fatal("downstream key crossed the WebSocket boundary")
		}
	}
}

func newWebSocketE2EUpstream(t *testing.T) *websocketE2EUpstream {
	t.Helper()
	upstream := &websocketE2EUpstream{}
	upstream.server = httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		if request.Method == http.MethodPost {
			if request.Header.Get("Authorization") != "Bearer "+upstreamAPIKey {
				writer.WriteHeader(http.StatusUnauthorized)
				return
			}
			_, _ = io.Copy(io.Discard, request.Body)
			writer.Header().Set("Content-Type", "application/json")
			_, _ = io.WriteString(writer, `{"id":"resp_http","usage":{"input_tokens":3,"output_tokens":1,"total_tokens":4}}`)
			return
		}
		upstream.mu.Lock()
		upstream.headers = request.Header.Clone()
		upstream.mu.Unlock()
		writer.Header().Set("X-Models-Etag", "e2e-etag")
		connection, err := websocket.Accept(writer, request, &websocket.AcceptOptions{
			CompressionMode: websocket.CompressionDisabled,
		})
		if err != nil {
			return
		}
		defer connection.CloseNow()
		for {
			messageType, payload, err := connection.Read(context.Background())
			if err != nil || messageType != websocket.MessageText {
				return
			}
			upstream.mu.Lock()
			upstream.frames = append(upstream.frames, string(payload))
			upstream.mu.Unlock()
			var frame struct {
				Model string `json:"model"`
			}
			if json.Unmarshal(payload, &frame) != nil {
				return
			}
			_ = connection.Write(context.Background(), websocket.MessageText, []byte(
				`{"type":"response.created","response":{"id":"resp_e2e"}}`,
			))
			total := 9
			if frame.Model == "prewarm" {
				total = 3
			}
			terminal := `{"type":"response.completed","response":{"usage":{` +
				`"input_tokens":2,"output_tokens":1,"total_tokens":` + e2eJSONNumber(total) + `}}}`
			_ = connection.Write(context.Background(), websocket.MessageText, []byte(terminal))
		}
	}))
	assertLoopbackURL(t, upstream.server.URL)
	return upstream
}

func writeE2EWebSocketText(t *testing.T, connection *websocket.Conn, payload string) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := connection.Write(ctx, websocket.MessageText, []byte(payload)); err != nil {
		t.Fatal(err)
	}
}

func readE2EWebSocketText(t *testing.T, connection *websocket.Conn) string {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	messageType, payload, err := connection.Read(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if messageType != websocket.MessageText {
		t.Fatalf("message type = %d", messageType)
	}
	return string(payload)
}

func waitForE2EHistory(
	t *testing.T,
	store *storage.Store,
	apiKeyID string,
	count int,
) []storage.RequestRecord {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for {
		history, err := store.History(context.Background(), apiKeyID, nil, 100)
		if err != nil {
			t.Fatal(err)
		}
		if len(history) == count {
			return history
		}
		if time.Now().After(deadline) {
			t.Fatalf("history = %#v", history)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func e2eJSONNumber(value int) string {
	data, _ := json.Marshal(value)
	return string(data)
}
