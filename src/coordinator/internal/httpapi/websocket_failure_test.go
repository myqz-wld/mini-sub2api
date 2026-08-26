package httpapi

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"

	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestWebSocketFirstFrameAndInterTurnTimeoutsAreCoordinatorOwned(t *testing.T) {
	coreServer := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		connection, err := websocket.Accept(writer, request, nil)
		if err != nil {
			return
		}
		defer connection.CloseNow()
		for {
			messageType, _, err := connection.Read(context.Background())
			if err != nil || messageType != websocket.MessageText {
				return
			}
			_ = writeCompletedEvent(connection, 3)
		}
	}))
	t.Cleanup(coreServer.Close)
	store, _, key := setupHTTPTest(t)
	core := &loopbackWebSocketCore{url: coreServer.URL}
	handler, publicServer := startPublicWebSocketServer(t, store, core)
	handler.wsTimeouts = websocketTimeouts{
		firstFrame: 40 * time.Millisecond,
		interTurn:  50 * time.Millisecond,
		write:      time.Second,
	}

	first, _, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
	if err != nil {
		t.Fatal(err)
	}
	readContext, cancelRead := context.WithTimeout(context.Background(), time.Second)
	_, _, readErr := first.Read(readContext)
	cancelRead()
	first.CloseNow()
	if websocket.CloseStatus(readErr) != websocket.StatusPolicyViolation {
		t.Fatalf("first-frame close = %v", readErr)
	}

	idle, _, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer idle.CloseNow()
	writeWebSocketText(t, idle, `{"type":"response.create","generate":false}`)
	if event := readWebSocketText(t, idle); !containsEventType(event, "response.completed") {
		t.Fatalf("terminal event = %q", event)
	}
	idleContext, cancelIdle := context.WithTimeout(context.Background(), time.Second)
	_, _, idleErr := idle.Read(idleContext)
	cancelIdle()
	if websocket.CloseStatus(idleErr) != websocket.StatusPolicyViolation {
		t.Fatalf("inter-turn close = %v", idleErr)
	}
	history := waitForHistory(t, store, key.ID, 1)
	if history[0].Status != storage.RequestCompleted ||
		history[0].OperationKind != storage.OperationWebSocketPrewarm {
		t.Fatalf("history = %#v", history)
	}
}

func TestWebSocketClientDisconnectFinalizesActiveTurn(t *testing.T) {
	coreServer, received, release := blockingCoreServer(t)
	defer close(release)
	store, _, key := setupHTTPTest(t)
	core := &loopbackWebSocketCore{url: coreServer.URL}
	_, publicServer := startPublicWebSocketServer(t, store, core)
	connection, _, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
	if err != nil {
		t.Fatal(err)
	}
	writeWebSocketText(t, connection, `{"type":"response.create","model":"hold"}`)
	waitSignal(t, received, "active create")
	connection.CloseNow()
	history := waitForHistory(t, store, key.ID, 1)
	if history[0].Status != storage.RequestDisconnected {
		t.Fatalf("history = %#v", history)
	}
}

func TestWebSocketShutdownFinalizesActiveTurnAsUpstreamError(t *testing.T) {
	coreServer, received, release := blockingCoreServer(t)
	store, _, key := setupHTTPTest(t)
	core := &loopbackWebSocketCore{url: coreServer.URL}
	handler, publicServer := startPublicWebSocketServer(t, store, core)
	connection, _, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer connection.CloseNow()
	writeWebSocketText(t, connection, `{"type":"response.create","model":"hold"}`)
	waitSignal(t, received, "active create")
	handler.ShutdownWebSockets()
	close(release)
	history := waitForHistory(t, store, key.ID, 1)
	if history[0].Status != storage.RequestUpstreamErr {
		t.Fatalf("history = %#v", history)
	}
}

func TestWebSocketFailureCloseTracksDeliveryState(t *testing.T) {
	for _, test := range []struct {
		name          string
		idle          bool
		malformed     bool
		invalidClose  bool
		structured    bool
		retryAdvice   protocolv1.RetryAdvice
		phase         protocolv1.FailurePhase
		deliveryState protocolv1.DeliveryState
	}{
		{name: "idle core close", idle: true, retryAdvice: protocolv1.RetrySafe, phase: protocolv1.PhaseWebSocketRelay, deliveryState: protocolv1.DeliveryNotDelivered},
		{name: "core close after dispatch", retryAdvice: protocolv1.RetryAmbiguous, phase: protocolv1.PhaseWebSocketRelay, deliveryState: protocolv1.DeliveryPossiblyDelivered},
		{name: "structured core failure", structured: true, retryAdvice: protocolv1.RetrySafe, phase: protocolv1.PhaseUpstreamConnect, deliveryState: protocolv1.DeliveryNotDelivered},
		{name: "invalid core failure payload", invalidClose: true, retryAdvice: protocolv1.RetryAmbiguous, phase: protocolv1.PhaseWebSocketRelay, deliveryState: protocolv1.DeliveryPossiblyDelivered},
		{name: "malformed upstream event", malformed: true, retryAdvice: protocolv1.RetryNever, phase: protocolv1.PhaseWebSocketRelay, deliveryState: protocolv1.DeliveryDelivered},
	} {
		t.Run(test.name, func(t *testing.T) {
			coreServer := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
				connection, err := websocket.Accept(writer, request, nil)
				if err != nil {
					return
				}
				defer connection.CloseNow()
				if !test.idle {
					if _, _, err := connection.Read(context.Background()); err != nil {
						return
					}
				}
				if test.invalidClose {
					_ = connection.Close(websocket.StatusCode(protocolv1.FailureCloseCode), "not-json")
					return
				}
				if test.structured {
					reason, ok := failureCloseReason(protocolv1.FailureMetadata{
						RetryAdvice: protocolv1.RetrySafe, Phase: protocolv1.PhaseUpstreamConnect,
						DeliveryState: protocolv1.DeliveryNotDelivered,
					})
					if !ok {
						return
					}
					_ = connection.Close(websocket.StatusCode(protocolv1.FailureCloseCode), reason)
					return
				}
				if test.malformed {
					_ = connection.Write(context.Background(), websocket.MessageText, []byte("not-json"))
				}
			}))
			t.Cleanup(coreServer.Close)
			store, _, key := setupHTTPTest(t)
			core := &loopbackWebSocketCore{url: coreServer.URL}
			_, publicServer := startPublicWebSocketServer(t, store, core)
			connection, _, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
			if err != nil {
				t.Fatal(err)
			}
			defer connection.CloseNow()
			if !test.idle {
				writeWebSocketText(t, connection, `{"type":"response.create","model":"test"}`)
			}
			readContext, cancelRead := context.WithTimeout(context.Background(), 2*time.Second)
			_, _, readErr := connection.Read(readContext)
			cancelRead()
			if websocket.CloseStatus(readErr) != websocket.StatusCode(protocolv1.FailureCloseCode) {
				t.Fatalf("public close = %v", readErr)
			}
			metadata, ok := parseCoreFailureClose(readErr)
			if !ok || metadata.RetryAdvice != test.retryAdvice || metadata.Phase != test.phase ||
				metadata.DeliveryState != test.deliveryState {
				t.Fatalf("failure metadata = %#v, %v", metadata, readErr)
			}
			if !test.idle {
				history := waitForHistory(t, store, key.ID, 1)
				if history[0].Status != storage.RequestUpstreamErr {
					t.Fatalf("history = %#v", history)
				}
			}
		})
	}
}

func TestWebSocketRejectsInvalidIdleAndOversizedApplicationMessages(t *testing.T) {
	var coreMessages atomic.Int64
	coreServer := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		connection, err := websocket.Accept(writer, request, nil)
		if err != nil {
			return
		}
		defer connection.CloseNow()
		for {
			if _, _, err := connection.Read(context.Background()); err != nil {
				return
			}
			coreMessages.Add(1)
		}
	}))
	t.Cleanup(coreServer.Close)
	store, _, key := setupHTTPTest(t)
	core := &loopbackWebSocketCore{url: coreServer.URL}
	_, publicServer := startPublicWebSocketServer(t, store, core)

	for _, test := range []struct {
		name     string
		message  websocket.MessageType
		payload  []byte
		wantCode websocket.StatusCode
	}{
		{name: "invalid JSON", message: websocket.MessageText, payload: []byte("not-json"), wantCode: websocket.StatusProtocolError},
		{name: "idle control", message: websocket.MessageText, payload: []byte(`{"type":"response.append_input_item"}`), wantCode: websocket.StatusPolicyViolation},
		{name: "binary", message: websocket.MessageBinary, payload: []byte("binary"), wantCode: websocket.StatusUnsupportedData},
	} {
		t.Run(test.name, func(t *testing.T) {
			connection, _, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
			if err != nil {
				t.Fatal(err)
			}
			writeContext, cancelWrite := context.WithTimeout(context.Background(), time.Second)
			err = connection.Write(writeContext, test.message, test.payload)
			cancelWrite()
			if err != nil {
				t.Fatal(err)
			}
			readContext, cancelRead := context.WithTimeout(context.Background(), time.Second)
			_, _, readErr := connection.Read(readContext)
			cancelRead()
			connection.CloseNow()
			if websocket.CloseStatus(readErr) != test.wantCode {
				t.Fatalf("close = %v", readErr)
			}
		})
	}

	oversized, _, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
	if err != nil {
		t.Fatal(err)
	}
	payload := []byte(`{"type":"response.create","padding":"` +
		strings.Repeat("a", maxRequestBytes) + `"}`)
	writeContext, cancelWrite := context.WithTimeout(context.Background(), 5*time.Second)
	writeErr := oversized.Write(writeContext, websocket.MessageText, payload)
	cancelWrite()
	if writeErr == nil {
		readContext, cancelRead := context.WithTimeout(context.Background(), 2*time.Second)
		_, _, writeErr = oversized.Read(readContext)
		cancelRead()
	}
	oversized.CloseNow()
	if websocket.CloseStatus(writeErr) != websocket.StatusMessageTooBig {
		t.Fatalf("oversized close = %v", writeErr)
	}
	if coreMessages.Load() != 0 {
		t.Fatalf("core received %d rejected messages", coreMessages.Load())
	}
}

func blockingCoreServer(t *testing.T) (*httptest.Server, <-chan struct{}, chan struct{}) {
	t.Helper()
	received := make(chan struct{})
	release := make(chan struct{})
	var once sync.Once
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		connection, err := websocket.Accept(writer, request, nil)
		if err != nil {
			return
		}
		defer connection.CloseNow()
		if _, _, err := connection.Read(context.Background()); err != nil {
			return
		}
		once.Do(func() { close(received) })
		<-release
	}))
	t.Cleanup(func() {
		select {
		case <-release:
		default:
			close(release)
		}
		server.Close()
	})
	return server, received, release
}

func waitSignal(t *testing.T, signal <-chan struct{}, description string) {
	t.Helper()
	select {
	case <-signal:
	case <-time.After(2 * time.Second):
		t.Fatalf("timed out waiting for %s", description)
	}
}
