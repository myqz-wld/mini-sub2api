package httpapi

import (
	"context"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"

	"mini-sub2api/src/coordinator/internal/storage"
)

type loopbackWebSocketCore struct {
	url     string
	mu      sync.Mutex
	calls   int
	headers http.Header
}

func (*loopbackWebSocketCore) Forward(
	context.Context,
	string,
	string,
	http.Header,
	[]byte,
) (*http.Response, error) {
	return nil, errors.New("HTTP forwarding is not expected")
}

func (c *loopbackWebSocketCore) DialWebSocket(
	ctx context.Context,
	_, _ string,
	headers http.Header,
) (*websocket.Conn, *http.Response, error) {
	c.mu.Lock()
	c.calls++
	c.headers = headers.Clone()
	c.mu.Unlock()
	return websocket.Dial(ctx, c.url, &websocket.DialOptions{
		HTTPHeader: headers, CompressionMode: websocket.CompressionDisabled,
	})
}

type countingWebSocketCore struct {
	mu    sync.Mutex
	calls int
}

func (*countingWebSocketCore) Forward(
	context.Context,
	string,
	string,
	http.Header,
	[]byte,
) (*http.Response, error) {
	return nil, errors.New("HTTP forwarding is not expected")
}

func (c *countingWebSocketCore) DialWebSocket(
	context.Context,
	string,
	string,
	http.Header,
) (*websocket.Conn, *http.Response, error) {
	c.mu.Lock()
	c.calls++
	c.mu.Unlock()
	return nil, nil, errors.New("unexpected WebSocket dial")
}

func startPublicWebSocketServer(
	t *testing.T,
	store *storage.Store,
	core Core,
) (*Handler, *httptest.Server) {
	t.Helper()
	handler := NewHandler(store, core, nil)
	server := httptest.NewServer(handler)
	t.Cleanup(func() {
		handler.ShutdownWebSockets()
		server.Close()
	})
	return handler, server
}

func dialPublicWebSocket(
	t *testing.T,
	serverURL, secret string,
	headers http.Header,
) (*websocket.Conn, *http.Response, error) {
	t.Helper()
	if headers == nil {
		headers = make(http.Header)
	} else {
		headers = headers.Clone()
	}
	headers.Set("Authorization", "Bearer "+secret)
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	return websocket.Dial(ctx, serverURL+"/v1/responses", &websocket.DialOptions{
		HTTPHeader: headers, CompressionMode: websocket.CompressionContextTakeover,
	})
}

func writeWebSocketText(t *testing.T, connection *websocket.Conn, payload string) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := connection.Write(ctx, websocket.MessageText, []byte(payload)); err != nil {
		t.Fatal(err)
	}
}

func readWebSocketText(t *testing.T, connection *websocket.Conn) string {
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

func waitForHistory(
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
			allTerminal := true
			for _, record := range history {
				allTerminal = allTerminal && record.Status != storage.RequestInProgress
			}
			if allTerminal {
				return history
			}
		}
		if time.Now().After(deadline) {
			t.Fatalf("history count = %d, want %d: %#v", len(history), count, history)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func responseBody(t *testing.T, response *http.Response) string {
	t.Helper()
	if response == nil || response.Body == nil {
		t.Fatal("response body is unavailable")
	}
	data, err := io.ReadAll(response.Body)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}
