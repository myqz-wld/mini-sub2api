package integration

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"mini-sub2api/src/coordinator/internal/adapter"
)

func restartHistoryFixtureCore(t *testing.T, supervisor *adapter.Supervisor) {
	t.Helper()
	before, ok := supervisor.Readiness()
	if !ok || before.PID <= 1 {
		t.Fatal("fixture Core is not ready")
	}
	process, err := os.FindProcess(before.PID)
	if err != nil || process.Kill() != nil {
		t.Fatal("restart owned fixture Core")
	}
	deadline := time.Now().Add(8 * time.Second)
	for time.Now().Before(deadline) {
		if after, ready := supervisor.Readiness(); ready && after.PID != before.PID {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("fixture Core restart timed out")
}

func historyImportTurn(t *testing.T, body []byte) string {
	t.Helper()
	request := decodeRequestObject(t, body)
	metadata, _ := request["client_metadata"].(map[string]any)
	raw, _ := metadata["x-codex-turn-metadata"].(string)
	var turn map[string]any
	if json.Unmarshal([]byte(raw), &turn) != nil {
		t.Fatal("turn metadata missing")
	}
	id, _ := turn["turn_id"].(string)
	if id == "" {
		t.Fatal("business turn ID missing")
	}
	return id
}

func historyImportEvents(id, turn string) []map[string]any {
	item := map[string]any{"type": "message", "id": "msg_" + id, "role": "assistant", "status": "completed",
		"content": []any{map[string]any{"type": "output_text", "text": "synthetic answer", "annotations": []any{}, "logprobs": []any{}}},
		"internal_chat_message_metadata_passthrough": map[string]any{"turn_id": turn}}
	return []map[string]any{
		{"type": "response.created", "response": map[string]any{"id": id, "status": "in_progress", "output": []any{}}},
		{"type": "response.output_item.done", "output_index": 0, "item": item},
		{"type": "response.completed", "response": map[string]any{"id": id, "status": "completed", "output": []any{item}}},
	}
}

func historyImportCompleted(t *testing.T, messages []string) map[string]any {
	t.Helper()
	for _, message := range messages {
		message = strings.TrimPrefix(message, "data: ")
		var event map[string]any
		if json.Unmarshal([]byte(message), &event) == nil && event["type"] == "response.completed" {
			response, _ := event["response"].(map[string]any)
			return response
		}
	}
	t.Fatal("completion missing")
	return nil
}

func assertHistoryImportContinuity(t *testing.T, wires []map[string]any) {
	t.Helper()
	metadata := func(value map[string]any) map[string]any { m, _ := value["client_metadata"].(map[string]any); return m }
	if metadata(wires[0])["thread_id"] == metadata(wires[1])["thread_id"] || metadata(wires[1])["thread_id"] != metadata(wires[2])["thread_id"] {
		t.Fatal("restart import did not establish one independent continuing thread")
	}
	var imported any
	for i, wire := range wires[1:] {
		items, _ := wire["input"].([]any)
		found := false
		for _, raw := range items {
			item, _ := raw.(map[string]any)
			if item["role"] != "assistant" {
				continue
			}
			m, _ := item["internal_chat_message_metadata_passthrough"].(map[string]any)
			if i == 0 {
				imported = m["turn_id"]
			} else if m["turn_id"] != imported {
				t.Fatal("imported turn changed during full replay")
			}
			found = true
			break
		}
		if !found || imported == nil {
			t.Fatal("imported historical turn missing")
		}
	}
}

func TestResponsesFullHistoryImportsOldTurnsAfterCoreRestartHTTP(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
		for _, stream := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/stream=%t", model, stream), func(t *testing.T) {
				fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, body []byte) (string, string) {
					id := fmt.Sprintf("resp_import_%d", loopbackResponseSequence.Add(1))
					w.Header().Set("Content-Type", "text/event-stream")
					for _, event := range historyImportEvents(id, historyImportTurn(t, body)) {
						fmt.Fprintf(w, "data: %s\n\n", mustRequestJSONValue(event))
					}
					return id, "synthetic-provider"
				})
				input := []any{map[string]any{"role": "user", "content": "Synthetic first turn"}}
				var wires []map[string]any
				for step := 0; step < 3; step++ {
					if step == 1 {
						restartHistoryFixtureCore(t, fixture.supervisor)
					}
					body := map[string]any{"model": model, "stream": stream, "input": input}
					status, raw, _ := publicRequestWithHeaders(t, fixture.public, fixture.subscriptionKey, string(mustRequestJSON(t, body)), nil)
					if status != 200 {
						t.Fatalf("history replay status=%d step=%d", status, step)
					}
					response := map[string]any{}
					if stream {
						response = historyImportCompleted(t, strings.Split(raw, "\n"))
					} else if json.Unmarshal([]byte(raw), &response) != nil {
						t.Fatal("invalid public response")
					}
					output, _ := response["output"].([]any)
					input = append(input, output...)
					input = append(input, map[string]any{"role": "user", "content": fmt.Sprintf("Synthetic next turn %d", step)})
					capture := waitForRoutingCapture(t, fixture.captures)
					wires = append(wires, decodeRequestObject(t, capture.Body))
				}
				assertHistoryImportContinuity(t, wires)
			})
		}
	}
}

func TestResponsesFullHistoryImportsOldTurnsAfterCoreRestartWebSocket(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
		t.Run(model, func(t *testing.T) {
			fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(c *websocket.Conn, body []byte, id string) {
				for _, event := range historyImportEvents(id, historyImportTurn(t, body)) {
					_ = c.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(event))
				}
			})
			connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, http.Header{"Originator": []string{"codex_exec"}})
			defer func() { _ = connection.CloseNow() }()
			input := []any{map[string]any{"role": "user", "content": "Synthetic first turn"}}
			var wires []map[string]any
			for step := 0; step < 3; step++ {
				if step == 1 {
					_ = connection.CloseNow()
					restartHistoryFixtureCore(t, fixture.supervisor)
					connection = dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, http.Header{"Originator": []string{"codex_exec"}})
				}
				writeE2EWebSocketText(t, connection, string(mustRequestJSON(t, map[string]any{"type": "response.create", "model": model, "input": input})))
				response := historyImportCompleted(t, readResponsesProfileTerminalEvents(t, connection))
				output, _ := response["output"].([]any)
				input = append(input, output...)
				input = append(input, map[string]any{"role": "user", "content": fmt.Sprintf("Synthetic next turn %d", step)})
				capture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
				wires = append(wires, decodeResponsesProfileWebSocketFrame(t, capture.Frame))
			}
			assertHistoryImportContinuity(t, wires)
		})
	}
}
