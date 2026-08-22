package httpapi

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"

	"mini-sub2api/src/coordinator/internal/storage"
)

type lifecycleCapture struct {
	mu     sync.Mutex
	frames []string
}

func TestWebSocketPersistsSequentialTurnsAndExcludesPrewarmFromAggregates(t *testing.T) {
	capture := &lifecycleCapture{}
	coreServer := newLifecycleCoreServer(t, capture)
	store, _, key := setupHTTPTest(t)
	core := &loopbackWebSocketCore{url: coreServer.URL}
	_, publicServer := startPublicWebSocketServer(t, store, core)
	connection, _, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer connection.CloseNow()

	prewarm := `{"type":"response.create","model":"prewarm","generate":false}`
	writeWebSocketText(t, connection, prewarm)
	if event := readWebSocketText(t, connection); !containsEventType(event, "response.created") {
		t.Fatalf("prewarm first event = %q", event)
	}
	if event := readWebSocketText(t, connection); !containsEventType(event, "response.completed") {
		t.Fatalf("prewarm terminal event = %q", event)
	}

	inference := `{"type":"response.create","model":"control","previous_response_id":"resp_warm"}`
	control := `{"type":"response.append_input_item","item":{"type":"input_text","text":"more"}}`
	writeWebSocketText(t, connection, inference)
	if event := readWebSocketText(t, connection); !containsEventType(event, "response.created") {
		t.Fatalf("inference first event = %q", event)
	}
	writeWebSocketText(t, connection, control)
	if event := readWebSocketText(t, connection); !containsEventType(event, "response.output_text.delta") {
		t.Fatalf("control event response = %q", event)
	}
	if event := readWebSocketText(t, connection); !containsEventType(event, "response.completed") {
		t.Fatalf("inference terminal event = %q", event)
	}

	history := waitForHistory(t, store, key.ID, 2)
	var prewarmRecord, inferenceRecord *storage.RequestRecord
	for index := range history {
		record := &history[index]
		switch record.OperationKind {
		case storage.OperationWebSocketPrewarm:
			prewarmRecord = record
		case storage.OperationInference:
			inferenceRecord = record
		}
	}
	if prewarmRecord == nil || inferenceRecord == nil ||
		prewarmRecord.Transport != storage.TransportWebSocket ||
		inferenceRecord.Transport != storage.TransportWebSocket ||
		prewarmRecord.Status != storage.RequestCompleted ||
		inferenceRecord.Status != storage.RequestCompleted ||
		prewarmRecord.TTFBMilliseconds == nil || inferenceRecord.TTFBMilliseconds == nil {
		t.Fatalf("history = %#v", history)
	}
	stats, err := store.Stats(context.Background(), key.ID, "", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(stats) != 1 || stats[0].RequestCount != 1 || stats[0].Usage == nil ||
		stats[0].Usage.TotalTokens != 30 {
		t.Fatalf("inference-only stats = %#v", stats)
	}
	capture.mu.Lock()
	defer capture.mu.Unlock()
	if len(capture.frames) != 3 || capture.frames[0] != prewarm ||
		capture.frames[1] != inference || capture.frames[2] != control {
		t.Fatalf("relayed frames = %#v", capture.frames)
	}
}

func TestWebSocketRevocationBlocksTheNextCreateWithoutForwarding(t *testing.T) {
	capture := &lifecycleCapture{}
	coreServer := newLifecycleCoreServer(t, capture)
	store, _, key := setupHTTPTest(t)
	core := &loopbackWebSocketCore{url: coreServer.URL}
	_, publicServer := startPublicWebSocketServer(t, store, core)
	connection, _, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer connection.CloseNow()

	writeWebSocketText(t, connection, `{"type":"response.create","model":"prewarm","generate":false}`)
	_ = readWebSocketText(t, connection)
	_ = readWebSocketText(t, connection)
	if err := store.RevokeAPIKey(context.Background(), key.ID); err != nil {
		t.Fatal(err)
	}
	writeWebSocketText(t, connection, `{"type":"response.create","model":"must-not-cross"}`)
	readContext, cancelRead := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancelRead()
	_, _, readErr := connection.Read(readContext)
	if websocket.CloseStatus(readErr) != websocket.StatusPolicyViolation {
		t.Fatalf("close error = %v", readErr)
	}
	history := waitForHistory(t, store, key.ID, 1)
	if history[0].OperationKind != storage.OperationWebSocketPrewarm {
		t.Fatalf("history = %#v", history)
	}
	time.Sleep(25 * time.Millisecond)
	capture.mu.Lock()
	defer capture.mu.Unlock()
	if len(capture.frames) != 1 {
		t.Fatalf("revoked create reached core: %#v", capture.frames)
	}
}

func TestWebSocketOverlappingCreateClosesAndFinalizesActiveTurn(t *testing.T) {
	firstReceived := make(chan struct{})
	release := make(chan struct{})
	coreServer := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		connection, err := websocket.Accept(writer, request, nil)
		if err != nil {
			return
		}
		defer connection.CloseNow()
		if _, _, err := connection.Read(context.Background()); err != nil {
			return
		}
		close(firstReceived)
		<-release
	}))
	t.Cleanup(func() {
		select {
		case <-release:
		default:
			close(release)
		}
		coreServer.Close()
	})
	store, _, key := setupHTTPTest(t)
	core := &loopbackWebSocketCore{url: coreServer.URL}
	_, publicServer := startPublicWebSocketServer(t, store, core)
	connection, _, err := dialPublicWebSocket(t, publicServer.URL, key.Secret, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer connection.CloseNow()

	writeWebSocketText(t, connection, `{"type":"response.create","model":"first"}`)
	select {
	case <-firstReceived:
	case <-time.After(2 * time.Second):
		t.Fatal("first create did not reach core")
	}
	writeWebSocketText(t, connection, `{"type":"response.create","model":"overlap"}`)
	readContext, cancelRead := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancelRead()
	_, _, readErr := connection.Read(readContext)
	if websocket.CloseStatus(readErr) != websocket.StatusPolicyViolation {
		t.Fatalf("close error = %v", readErr)
	}
	close(release)
	history := waitForHistory(t, store, key.ID, 1)
	if history[0].Status != storage.RequestDisconnected {
		t.Fatalf("history = %#v", history)
	}
}

func newLifecycleCoreServer(t *testing.T, capture *lifecycleCapture) *httptest.Server {
	t.Helper()
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		connection, err := websocket.Accept(writer, request, &websocket.AcceptOptions{
			CompressionMode: websocket.CompressionDisabled,
		})
		if err != nil {
			return
		}
		defer connection.CloseNow()
		controlActive := false
		for {
			messageType, payload, err := connection.Read(context.Background())
			if err != nil || messageType != websocket.MessageText {
				return
			}
			capture.mu.Lock()
			capture.frames = append(capture.frames, string(payload))
			capture.mu.Unlock()
			var event struct {
				Type     string `json:"type"`
				Model    string `json:"model"`
				Generate *bool  `json:"generate"`
			}
			if json.Unmarshal(payload, &event) != nil {
				return
			}
			if event.Type == "response.create" {
				_ = connection.Write(context.Background(), websocket.MessageText, []byte(
					`{"type":"response.created","response":{"id":"resp_test"}}`,
				))
				if event.Model == "control" {
					controlActive = true
					continue
				}
				_ = writeCompletedEvent(connection, 3)
				continue
			}
			if controlActive {
				_ = connection.Write(context.Background(), websocket.MessageText, []byte(
					`{"type":"response.output_text.delta","delta":"ok"}`,
				))
				_ = writeCompletedEvent(connection, 30)
				controlActive = false
			}
		}
	}))
	t.Cleanup(server.Close)
	return server
}

func writeCompletedEvent(connection *websocket.Conn, total int) error {
	payload := []byte(`{"type":"response.completed","response":{"usage":{` +
		`"input_tokens":10,"output_tokens":20,"total_tokens":` + jsonNumber(total) + `}}}`)
	return connection.Write(context.Background(), websocket.MessageText, payload)
}

func jsonNumber(value int) string {
	data, _ := json.Marshal(value)
	return string(data)
}

func containsEventType(payload, eventType string) bool {
	var event struct {
		Type string `json:"type"`
	}
	return json.Unmarshal([]byte(payload), &event) == nil && event.Type == eventType
}
