//go:build nativeparity

package integration

import (
	"bytes"
	"fmt"
	"testing"
)

func compactionUser(text string) map[string]any {
	return map[string]any{"type": "message", "role": "user", "content": []any{map[string]any{"type": "input_text", "text": text}}}
}

func compactionDelta(first map[string]any, previous any, text string) map[string]any {
	request := map[string]any{"model": first["model"], "previous_response_id": previous, "input": []any{compactionUser(text)}}
	for _, field := range []string{"instructions", "tools"} {
		if value, exists := first[field]; exists {
			request[field] = value
		}
	}
	return request
}

func TestCompactionResponseHTTPContinuationMatrix(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, delivery := range []string{"http-json", "http-sse", "ws"} {
			for _, format := range []string{"ordinary", "converted-lite", "native-lite"} {
				t.Run(fmt.Sprintf("subscription=%t/%s/%s", subscription, delivery, format), func(t *testing.T) {
					capture := newNativeCompactionCapture(t, "v2", false)
					gateway := newNativeGateway(t, capture.server.URL, subscription)
					client := ordinaryClient(t, gateway, delivery == "ws", delivery != "http-json", nil)
					first := ordinaryRequest(format, "anonymous")
					items := first["input"].([]any)
					items = append(items,
						map[string]any{"type": "message", "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": "discarded assistant"}}},
						map[string]any{"type": "function_call", "call_id": "call_closed", "name": "native_probe", "arguments": "{}"},
						map[string]any{"type": "function_call_output", "call_id": "call_closed", "output": "discarded tool result"},
						map[string]any{"type": "compaction", "encrypted_content": "discarded checkpoint"},
						map[string]any{"type": "compaction_trigger"})
					first["input"] = items
					first["client_metadata"] = map[string]any{"x-codex-turn-metadata": string(mustRequestJSON(t, map[string]any{"request_kind": "compaction", "compaction": map[string]any{"implementation": "responses_compaction_v2"}}))}
					checkpoint := client.send(first)
					if client.ws != nil {
						_ = client.ws.CloseNow()
					}
					if len(checkpoint["output"].([]any)) != 1 {
						t.Fatal("compaction checkpoint output count")
					}
					httpClient := ordinaryClient(t, gateway, false, delivery != "http-json", nil)
					second := httpClient.send(compactionDelta(first, checkpoint["id"], "first incremental user"))
					third := httpClient.send(compactionDelta(first, second["id"], "second incremental user"))
					if delivery == "ws" {
						reconnected := ordinaryClient(t, gateway, true, true, nil)
						reconnected.send(compactionDelta(first, third["id"], "reconnected user"))
					}
					wires := businessWires(capture.snapshot())
					want := 3
					if delivery == "ws" {
						want++
					}
					if len(wires) != want {
						t.Fatal("compaction continuation added or lost an inference")
					}
					for i, wire := range wires[1:] {
						if !subscription {
							if wire.value["previous_response_id"] == nil || len(wire.value["input"].([]any)) != 1 {
								t.Fatal("API-key compaction continuation was expanded")
							}
							continue
						}
						if wire.value["previous_response_id"] != nil {
							t.Fatal("HTTP or reconnected WS compaction history was not reconstructed")
						}
						types, texts := compactionItemCounts(wire)
						if types["compaction"] != 1 || types["compaction_trigger"] != 0 || types["function_call"] != 0 || types["function_call_output"] != 0 || texts["discarded assistant"] != 0 {
							t.Fatal("reconstruction revived compacted history")
						}
						if texts["first ordinary user"] != 1 || texts["duplicate developer"] != 2 || texts["system fixture"] != 1 || texts["first incremental user"] != 1 || (i > 0 && texts["second incremental user"] != 1) {
							t.Fatal("compaction continuation lost caller context or a later suffix")
						}
						for _, raw := range wire.value["input"].([]any) {
							item, _ := raw.(map[string]any)
							if item["type"] == "compaction" && item["encrypted_content"] != "synthetic_compacted_state" {
								t.Fatal("compaction ciphertext changed")
							}
						}
						if format == "ordinary" && wire.value["instructions"] != first["instructions"] {
							t.Fatal("current ordinary instructions changed after compaction")
						}
						if format == "converted-lite" {
							assertNativeLiteIDs(t, wire)
						}
					}
					if !subscription {
						packets := gateway.tap.packets(t)
						if len(packets) != len(wires) {
							t.Fatal("API-key compaction capture count changed")
						}
						for i := range wires {
							if !bytes.Equal(packets[i].payload, wires[i].encodedBody) {
								t.Fatal("API-key compaction packet changed")
							}
						}
					}
				})
			}
		}
	}
}

func TestCompactionResponseUnavailableWindows(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, kind := range []string{"local-summary", "unknown-input", "unresolved-call"} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, kind), func(t *testing.T) {
				mode, implementation := "v2", "responses_compaction_v2"
				items := []any{compactionUser("source")}
				if kind == "local-summary" {
					mode, implementation = "local", "responses"
				} else {
					if kind == "unknown-input" {
						items = append(items, map[string]any{"type": "future_context", "data": "unknown semantics"})
					} else {
						items = append(items, map[string]any{"type": "function_call", "call_id": "call_pending", "name": "native_probe", "arguments": "{}"})
					}
					items = append(items, map[string]any{"type": "compaction_trigger"})
				}
				capture := newNativeCompactionCapture(t, mode, false)
				gateway := newNativeGateway(t, capture.server.URL, true)
				client := ordinaryClient(t, gateway, ws, true, nil)
				first := ordinaryCompactionRequest("gpt-5.4", items, nil)
				first["client_metadata"] = map[string]any{"x-codex-turn-metadata": string(mustRequestJSON(t, map[string]any{"request_kind": "compaction", "compaction": map[string]any{"implementation": implementation}}))}
				checkpoint := client.send(first)
				before := len(capture.snapshot())
				status, code := compactionHTTPError(t, gateway, compactionDelta(first, checkpoint["id"], "next"))
				if status != 503 || code != "state_unavailable" || len(capture.snapshot()) != before {
					t.Fatal("unknown compaction window allowed HTTP inference")
				}
			})
		}
	}
}
