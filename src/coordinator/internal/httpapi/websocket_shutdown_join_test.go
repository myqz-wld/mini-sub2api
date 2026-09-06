package httpapi

import (
	"context"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestWebSocketSessionJoinsTerminalFinalizationBeforeUnregistering(t *testing.T) {
	coreServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		connection, err := websocket.Accept(w, r, nil)
		if err != nil {
			return
		}
		defer connection.CloseNow()
		if _, _, err = connection.Read(context.Background()); err != nil {
			return
		}
		for _, event := range []string{
			`{"type":"response.created","response":{"id":"synthetic-response"}}`,
			`{"type":"response.completed","response":{"id":"synthetic-response","output":[]}}`,
		} {
			if connection.Write(context.Background(), websocket.MessageText, []byte(event)) != nil {
				return
			}
		}
		_, _, _ = connection.Read(context.Background())
	}))
	defer coreServer.Close()
	store, _, key := setupHTTPTest(t)
	handler := NewHandler(store, &loopbackWebSocketCore{url: coreServer.URL}, nil)
	finalizing, release := make(chan struct{}), make(chan struct{})
	var calls atomic.Int32
	handler.clock = func() time.Time {
		if calls.Add(1) == 3 {
			close(finalizing)
			<-release
		}
		return time.Now()
	}
	exited := make(chan struct{})
	publicServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		handler.ServeHTTP(w, r)
		close(exited)
	}))
	defer publicServer.Close()
	defer handler.ShutdownWebSockets()
	// Release before server/store cleanup even if an assertion fails.
	defer close(release)
	connection, _, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
	if err != nil {
		t.Fatal("dial controlled finalization fixture")
	}
	defer connection.CloseNow()
	writeWebSocketText(t, connection, `{"type":"response.create","model":"test"}`)
	select {
	case <-finalizing:
	case <-time.After(3 * time.Second):
		t.Fatal("terminal finalization barrier was not reached")
	}
	_ = connection.CloseNow()
	select {
	case <-exited:
		t.Error("session unregistered while its terminal finalization pump was still running")
	case <-time.After(100 * time.Millisecond):
	}
}
