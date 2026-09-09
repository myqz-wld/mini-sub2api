package integration

import (
	"context"
	"encoding/json"
	"net/http"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestResponsesIntegrityWebSocketRetryKeepsFirstRoutingToken(t *testing.T) {
	for _, outcome := range []string{"error-before-created", "error-after-created", "completed"} {
		t.Run(outcome, func(t *testing.T) {
			var calls atomic.Int32
			fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(conn *websocket.Conn, _ []byte, responseID string) {
				call := calls.Add(1)
				token := "first-routing-token"
				if call > 1 {
					token = "second-routing-token"
				}
				send := func(value any) {
					_ = conn.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(value))
				}
				send(map[string]any{"type": "response.metadata", "headers": map[string]any{"x-codex-turn-state": token}})
				if outcome != "error-before-created" || call > 1 {
					send(map[string]any{"type": "response.created", "response": map[string]any{"id": responseID}})
				}
				if outcome != "completed" && call == 1 {
					send(map[string]any{"type": "error", "error": map[string]any{"code": "synthetic"}})
				} else {
					send(map[string]any{"type": "response.completed", "response": map[string]any{"id": responseID, "output": []any{}}})
				}
			})
			payload := mustRequestJSON(t, map[string]any{"type": "response.create", "model": "gpt-5.4", "input": "retry same turn", "client_metadata": map[string]any{"session_id": "retry-session", "thread_id": "retry-thread", "turn_id": "retry-turn"}})
			for attempt := 0; attempt < 2; attempt++ {
				conn := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, http.Header{"Originator": []string{"codex_exec"}})
				ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
				if err := conn.Write(ctx, websocket.MessageText, payload); err != nil {
					t.Fatal("retry frame failed")
				}
				for {
					_, raw, err := conn.Read(ctx)
					if err != nil {
						t.Fatal("retry did not reach a terminal event")
					}
					var event struct {
						Type string `json:"type"`
					}
					if json.Unmarshal(raw, &event) != nil {
						t.Fatal("invalid public event")
					}
					if event.Type == "error" || event.Type == "response.completed" {
						break
					}
				}
				_ = conn.Close(websocket.StatusNormalClosure, "")
				cancel()
			}
			captures := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 2)
			second := decodeResponsesProfileWebSocketFrame(t, captures[1].Frame)
			metadata, _ := second["client_metadata"].(map[string]any)
			if metadata["x-codex-turn-state"] != "first-routing-token" {
				t.Fatal("same-turn reconnect lost the first routing token")
			}
			if captures[1].Headers.Get("X-Codex-Turn-State") != "" {
				t.Fatal("Subscription routing token must stay in WS create metadata")
			}
		})
	}
}

func TestResponsesIntegrityWebSocketRejectsConflictingFooter(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(conn *websocket.Conn, _ []byte, id string) {
		for _, event := range []any{
			map[string]any{"type": "response.created", "response": map[string]any{"id": id}},
			map[string]any{"type": "response.output_item.done", "output_index": 0, "item": map[string]any{"id": "msg_ws", "type": "message", "role": "assistant", "content": []any{}}},
			map[string]any{"type": "response.completed", "response": map[string]any{"id": id, "output": []any{map[string]any{"id": "msg_ws", "type": "message", "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": "conflict"}}}}}},
		} {
			_ = conn.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(event))
		}
	})
	conn := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, http.Header{"Originator": []string{"codex_exec"}})
	defer conn.CloseNow()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := conn.Write(ctx, websocket.MessageText, []byte(`{"type":"response.create","model":"gpt-5.4","input":"footer probe"}`)); err != nil {
		t.Fatal("create failed")
	}
	done := false
	for {
		_, raw, err := conn.Read(ctx)
		if err != nil {
			if ctx.Err() != nil || !done {
				t.Fatal("invalid footer did not fail after its validated prefix")
			}
			break
		}
		var event struct {
			Type string `json:"type"`
		}
		if json.Unmarshal(raw, &event) != nil {
			t.Fatal("invalid public event")
		}
		if event.Type == "response.completed" {
			t.Fatal("conflicting footer completed publicly")
		}
		done = done || event.Type == "response.output_item.done"
	}
}
