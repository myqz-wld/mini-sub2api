package integration

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
	"mini-sub2api/src/coordinator/internal/storage"
)

func interrupt1592Fixture(t *testing.T, discard, footer, hold bool) responsesProfileWebSocketFixture {
	t.Helper()
	type peerState struct {
		id      string
		creates int
	}
	var states sync.Map
	return newResponsesProfileWebSocketFixtureWithResponder(t, func(conn *websocket.Conn, raw []byte, responseID string) {
		var request map[string]any
		if json.Unmarshal(raw, &request) != nil {
			return
		}
		entry, _ := states.LoadOrStore(conn, &peerState{})
		state := entry.(*peerState)
		write := func(event any) {
			_ = conn.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(event))
		}
		if request["type"] == "response.create" {
			if request["generate"] == false {
				writeResponsesProfileEvents(conn, responseID, true)
				return
			}
			state.creates++
			state.id = responseID
			if state.creates > 1 {
				writeResponsesProfileEvents(conn, responseID, false)
				return
			}
			write(map[string]any{"type": "response.created", "response": map[string]any{"id": responseID}})
			write(map[string]any{"type": "response.output_item.added", "response_id": responseID, "output_index": 0,
				"item": map[string]any{"type": "reasoning", "id": "reason_partial", "summary": []any{}}})
			return
		}
		if request["type"] != "response.interrupt" || request["response_id"] != state.id {
			return
		}
		write(map[string]any{"type": "response.interrupt.accepted", "response_id": state.id})
		if hold {
			return
		}
		var output []any
		if discard {
			write(map[string]any{"type": "response.output_item.interrupted", "response_id": state.id, "output_index": 0, "item_id": "reason_partial"})
		} else {
			reasoning := map[string]any{"type": "reasoning", "id": "reason_partial", "summary": []any{}, "encrypted_content": "synthetic finished ciphertext"}
			write(map[string]any{"type": "response.output_item.done", "response_id": state.id, "output_index": 0, "item": reasoning})
			output = append(output, reasoning)
		}
		message := map[string]any{"type": "message", "id": "msg_interrupt_kept", "role": "assistant", "status": "completed",
			"content": []any{map[string]any{"type": "output_text", "text": "synthetic kept output"}}}
		write(map[string]any{"type": "response.output_item.done", "response_id": state.id, "output_index": 1, "item": message})
		output = append(output, message)
		if !footer {
			output = []any{}
		}
		write(map[string]any{"type": "response.incomplete", "response": map[string]any{"id": state.id, "status": "incomplete",
			"incomplete_details": map[string]any{"reason": "interrupted"}, "output": output,
			"usage": map[string]any{"input_tokens": 40, "output_tokens": 2, "total_tokens": 42}}})
	})
}

func interrupt1592Start(t *testing.T, fixture responsesProfileWebSocketFixture, secret, model string) (*websocket.Conn, string, string) {
	t.Helper()
	conn := dialResponsesProfileWebSocket(t, fixture.public, secret, http.Header{"Originator": {"synthetic-interrupt-client"}})
	t.Cleanup(func() { _ = conn.CloseNow() })
	writeE2EWebSocketText(t, conn, string(mustRequestJSON(t, map[string]any{"type": "response.create", "model": model,
		"input": []any{map[string]any{"type": "message", "role": "user", "content": []any{map[string]any{"type": "input_text", "text": "synthetic first"}}}}})))
	created := decodeRequestObject(t, []byte(readE2EWebSocketText(t, conn)))
	partial := decodeRequestObject(t, []byte(readE2EWebSocketText(t, conn)))
	if created["type"] != "response.created" || partial["type"] != "response.output_item.added" {
		t.Fatal("initial interrupt response shape")
	}
	id, _ := created["response"].(map[string]any)["id"].(string)
	itemID, _ := partial["item"].(map[string]any)["id"].(string)
	if id == "" || itemID == "" {
		t.Fatal("missing interrupt identity")
	}
	return conn, id, itemID
}

func TestResponsesInterrupt1592ContinuationAndUsage(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, discard := range []bool{false, true} {
			for _, footer := range []bool{false, true} {
				t.Run(fmt.Sprintf("subscription=%t/discard=%t/footer=%t", subscription, discard, footer), func(t *testing.T) {
					fixture := interrupt1592Fixture(t, discard, footer, false)
					secret, keyID := fixture.apiKey, fixture.apiKeyID
					if subscription {
						secret, keyID = fixture.subscriptionKey, fixture.subscriptionKeyID
					}
					conn, id, itemID := interrupt1592Start(t, fixture, secret, "gpt-6-sol")
					initial := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
					control := mustRequestJSON(t, map[string]any{"type": "response.interrupt", "response_id": id, "mode": "discard_partial_items"})
					writeE2EWebSocketText(t, conn, string(control))
					for i := 0; i < 4; i++ {
						event := decodeRequestObject(t, []byte(readE2EWebSocketText(t, conn)))
						if i == 0 && (event["type"] != "response.interrupt.accepted" || event["response_id"] != id) {
							t.Fatal("interrupt acknowledgement identity changed")
						}
						if i == 1 && discard && (event["type"] != "response.output_item.interrupted" || event["item_id"] != itemID) {
							t.Fatal("discarded item alias changed")
						}
						if i == 3 {
							response, _ := event["response"].(map[string]any)
							details, _ := response["incomplete_details"].(map[string]any)
							if event["type"] != "response.incomplete" || response["id"] != id || details["reason"] != "interrupted" {
								t.Fatal("native interrupted terminal changed")
							}
						}
					}
					captured := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
					upstream := decodeRequestObject(t, captured.Frame)
					if len(upstream) != 3 || upstream["type"] != "response.interrupt" || upstream["response_id"] != initial.ResponseID || upstream["mode"] != "discard_partial_items" {
						t.Fatal("control was dropped or scoped ID was not reversed")
					}
					if !subscription && string(captured.Frame) != string(control) {
						t.Fatal("API-key control bytes changed")
					}
					next := map[string]any{"type": "response.create", "model": "gpt-6-sol", "previous_response_id": id,
						"input": []any{map[string]any{"type": "message", "role": "user", "content": []any{map[string]any{"type": "input_text", "text": "synthetic follow-up"}}}}}
					writeE2EWebSocketText(t, conn, string(mustRequestJSON(t, next)))
					readResponsesProfileTerminalEvents(t, conn)
					follow := decodeRequestObject(t, waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0].Frame)
					if follow["previous_response_id"] != initial.ResponseID || len(follow["input"].([]any)) != 1 {
						t.Fatal("interrupted response did not support same-socket incremental continuation")
					}
					deadline := time.Now().Add(3 * time.Second)
					for {
						records, err := fixture.store.History(context.Background(), keyID, nil, 10)
						if err != nil {
							t.Fatal("read synthetic request accounting")
						}
						if len(records) == 2 && records[0].CompletedAt != nil && records[1].CompletedAt != nil {
							var total int64
							for _, record := range records {
								if record.Status != storage.RequestCompleted || record.Usage == nil {
									t.Fatal("controlled interruption misclassified as upstream error")
								}
								total += record.Usage.TotalTokens
							}
							if total != 44 {
								t.Fatal("interrupt control duplicated or lost usage")
							}
							break
						}
						if time.Now().After(deadline) {
							t.Fatal("interrupt usage accounting deadline")
						}
						time.Sleep(10 * time.Millisecond)
					}
				})
			}
		}
	}
}

func TestResponsesInterrupt1592RejectsInvalidAndForeignControls(t *testing.T) {
	for _, kind := range []string{"wrong-id", "missing-id", "empty-id", "null-id", "mode", "extra", "ordinary", "duplicate", "foreign-socket", "foreign-key"} {
		t.Run(kind, func(t *testing.T) {
			fixture := interrupt1592Fixture(t, true, false, true)
			model := "gpt-6-sol"
			if kind == "ordinary" {
				model = "gpt-5.5"
			}
			conn, id, _ := interrupt1592Start(t, fixture, fixture.subscriptionKey, model)
			_ = waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)
			if kind == "foreign-socket" || kind == "foreign-key" {
				secret := fixture.subscriptionKey
				if kind == "foreign-key" {
					route, err := fixture.store.AuthenticateConnection(context.Background(), secret)
					if err != nil {
						t.Fatal("synthetic key route")
					}
					secret = createDownstreamKey(t, fixture.store, route.CredentialID, "Other interrupt scope").Secret
				}
				conn, _, _ = interrupt1592Start(t, fixture, secret, model)
				_ = waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)
			}
			control := map[string]any{"type": "response.interrupt", "response_id": id, "mode": "discard_partial_items"}
			switch kind {
			case "wrong-id":
				control["response_id"] = "unknown-response"
			case "missing-id":
				delete(control, "response_id")
			case "empty-id":
				control["response_id"] = ""
			case "null-id":
				control["response_id"] = nil
			case "mode":
				control["mode"] = "unsupported"
			case "extra":
				control["input"] = []any{}
			case "duplicate":
				writeE2EWebSocketText(t, conn, string(mustRequestJSON(t, control)))
				_ = readE2EWebSocketText(t, conn)
				_ = waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)
			}
			writeE2EWebSocketText(t, conn, string(mustRequestJSON(t, control)))
			ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
			defer cancel()
			_, _, err := conn.Read(ctx)
			if websocket.CloseStatus(err) != websocket.StatusProtocolError {
				t.Fatalf("invalid interrupt close=%d", websocket.CloseStatus(err))
			}
			select {
			case <-fixture.captures:
				t.Fatal("invalid control reached upstream")
			default:
			}
		})
	}
}
