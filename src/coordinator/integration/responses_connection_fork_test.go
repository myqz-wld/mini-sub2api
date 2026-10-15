package integration

import (
	"encoding/json"
	"net/http"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestSeparateWebSocketConnectionsForkOverlappingExplicitTurn(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
		t.Run(model, func(t *testing.T) {
			started := make(chan int, 2)
			firstGate, secondGate := make(chan struct{}), make(chan struct{})
			finishFirst := sync.OnceFunc(func() { close(firstGate) })
			finishSecond := sync.OnceFunc(func() { close(secondGate) })
			var calls atomic.Int32
			fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(connection *websocket.Conn, _ []byte, id string) {
				n := int(calls.Add(1))
				if n <= 2 {
					started <- n
					if n == 1 {
						<-firstGate
					} else {
						<-secondGate
					}
				}
				writeResponsesProfileEvents(connection, id, false)
			})
			defer finishFirst()
			defer finishSecond()
			headers := http.Header{"Originator": {"codex_cli_rs"}}
			first := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, headers)
			defer first.CloseNow()
			second := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, headers)
			defer second.CloseNow()
			payload := string(mustRequestJSON(t, map[string]any{
				"type": "response.create", "model": model, "input": "synthetic shared task",
				"client_metadata": map[string]any{"session_id": "shared-session", "thread_id": "shared-session", "turn_id": "shared-turn"},
			}))
			writeE2EWebSocketText(t, first, payload)
			waitConnectionForkStart(t, started, 1)
			writeE2EWebSocketText(t, second, payload)
			waitConnectionForkStart(t, started, 2)
			finishSecond()
			secondID := readConnectionForkCompletion(t, second)
			finishFirst()
			firstID := readConnectionForkCompletion(t, first)
			if firstID == secondID || calls.Load() != 2 {
				t.Fatal("overlapping connections did not execute independently exactly once")
			}
		})
	}
}

func waitConnectionForkStart(t *testing.T, started <-chan int, want int) {
	t.Helper()
	select {
	case got := <-started:
		if got != want {
			t.Fatal("unexpected upstream request order")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("overlapping connection did not reach loopback upstream")
	}
}

func readConnectionForkCompletion(t *testing.T, connection *websocket.Conn) string {
	t.Helper()
	for i := 0; i < 8; i++ {
		var event struct {
			Type     string `json:"type"`
			Response struct {
				ID string `json:"id"`
			} `json:"response"`
		}
		if err := json.Unmarshal([]byte(readE2EWebSocketText(t, connection)), &event); err != nil {
			t.Fatal("invalid WebSocket response")
		}
		if event.Type == "response.completed" && event.Response.ID != "" {
			return event.Response.ID
		}
	}
	t.Fatal("missing completed WebSocket response")
	return ""
}
