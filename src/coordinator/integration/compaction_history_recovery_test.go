package integration

import (
	"encoding/json"
	"fmt"
	"net/http"
	"sync/atomic"
	"testing"
)

func TestCompactionFullHistoryRecoveryHTTP(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
		for _, state := range []string{"retained-anonymous", "restarted-anonymous", "restarted-explicit"} {
			for _, kind := range []string{"turn", "compaction-local", "compaction-v2"} {
				t.Run(fmt.Sprintf("%s/%s/%s", model, state, kind), func(t *testing.T) {
					var calls atomic.Int32
					fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, body []byte) (string, string) {
						n := calls.Add(1)
						id := fmt.Sprintf("resp_compaction_recovery_%d", n)
						writeCompactionRecoveryResult(t, w, id, historyImportTurn(t, body), n == 1, kind)
						return id, "synthetic-provider"
					})
					firstUser := map[string]any{"role": "user", "content": "Synthetic source"}
					body := map[string]any{
						"model": model, "stream": false, "include": []any{"reasoning.encrypted_content"},
						"input": []any{firstUser},
						"tools": []any{map[string]any{"type": "function", "name": "probe", "parameters": map[string]any{"type": "object", "properties": map[string]any{}}}},
					}
					if state == "restarted-explicit" {
						body["client_metadata"] = map[string]any{"session_id": "synthetic-recovery-session"}
					}
					status, raw, _ := publicRequestWithHeaders(t, fixture.public, fixture.subscriptionKey, string(mustRequestJSON(t, body)), nil)
					if status != http.StatusOK {
						t.Fatalf("seed status=%d", status)
					}
					waitForRoutingCapture(t, fixture.captures)
					var response map[string]any
					if json.Unmarshal([]byte(raw), &response) != nil {
						t.Fatal("invalid seed response")
					}
					output, ok := response["output"].([]any)
					if !ok || len(output) != 2 {
						t.Fatal("seed output missing")
					}
					if output[0].(map[string]any)["encrypted_content"] != "synthetic-ciphertext" {
						t.Fatal("reasoning ciphertext missing")
					}
					call := output[1].(map[string]any)
					input := append([]any{firstUser}, output...)
					input = append(input, map[string]any{"type": "function_call_output", "call_id": call["call_id"], "output": "synthetic paired result"})
					body["input"] = input
					metadata := map[string]any{}
					if state == "restarted-explicit" {
						metadata["session_id"] = "synthetic-recovery-session"
					}
					turnMetadata := map[string]any{"request_kind": "turn"}
					if kind != "turn" {
						implementation := "responses"
						if kind == "compaction-v2" {
							implementation = "responses_compaction_v2"
							body["input"] = append(input, map[string]any{"type": "compaction_trigger"})
						}
						turnMetadata = map[string]any{"request_kind": "compaction", "compaction": map[string]any{"implementation": implementation}}
					}
					metadata["x-codex-turn-metadata"] = string(mustRequestJSON(t, turnMetadata))
					body["client_metadata"] = metadata
					if state != "retained-anonymous" {
						fixture.restartRuntime(t)
					}
					status, raw, _ = publicRequestWithHeaders(t, fixture.public, fixture.subscriptionKey, string(mustRequestJSON(t, body)), nil)
					if status != http.StatusOK || calls.Load() != 2 {
						t.Fatalf("complete history recovery: status=%d upstream_calls=%d", status, calls.Load())
					}
					capture := waitForRoutingCapture(t, fixture.captures)
					assertCompactionRecoveryWire(t, decodeRequestObject(t, capture.Body), kind)
					if json.Unmarshal([]byte(raw), &response) != nil || response["status"] != "completed" {
						t.Fatal("recovered request did not complete")
					}
				})
			}
		}
	}
}

func writeCompactionRecoveryResult(t *testing.T, w http.ResponseWriter, id, turn string, seed bool, kind string) {
	t.Helper()
	var output []any
	if seed {
		output = []any{
			map[string]any{"type": "reasoning", "id": "rs_compaction_recovery", "summary": []any{}, "encrypted_content": "synthetic-ciphertext",
				"internal_chat_message_metadata_passthrough": map[string]any{"turn_id": turn}},
			map[string]any{"type": "function_call", "id": "fc_compaction_recovery", "call_id": "call_compaction_recovery", "name": "probe", "arguments": "{}",
				"internal_chat_message_metadata_passthrough": map[string]any{"turn_id": turn}},
		}
	} else if kind == "compaction-v2" {
		output = []any{map[string]any{"type": "compaction", "id": "cmp_compaction_recovery", "encrypted_content": "synthetic-checkpoint"}}
	} else {
		output = []any{map[string]any{"type": "message", "id": "msg_compaction_recovery", "role": "assistant", "status": "completed",
			"content": []any{map[string]any{"type": "output_text", "text": "synthetic result"}}}}
	}
	w.Header().Set("Content-Type", "text/event-stream")
	emit := func(value any) { fmt.Fprintf(w, "data: %s\n\n", mustRequestJSONValue(value)) }
	emit(map[string]any{"type": "response.created", "response": map[string]any{"id": id, "status": "in_progress", "output": []any{}}})
	for index, item := range output {
		emit(map[string]any{"type": "response.output_item.done", "output_index": index, "item": item})
	}
	emit(map[string]any{"type": "response.completed", "response": map[string]any{"id": id, "status": "completed", "output": output}})
}

func assertCompactionRecoveryWire(t *testing.T, wire map[string]any, kind string) {
	t.Helper()
	items, _ := wire["input"].([]any)
	found := map[string]int{}
	for _, raw := range items {
		item := raw.(map[string]any)
		typ, _ := item["type"].(string)
		found[typ]++
		switch typ {
		case "reasoning":
			if item["id"] != "rs_compaction_recovery" || item["encrypted_content"] != "synthetic-ciphertext" {
				t.Fatal("recovery changed reasoning identity or ciphertext")
			}
		case "function_call", "function_call_output":
			if item["call_id"] != "call_compaction_recovery" {
				t.Fatal("recovery changed the paired tool identity")
			}
		}
	}
	if found["reasoning"] != 1 || found["function_call"] != 1 || found["function_call_output"] != 1 {
		t.Fatal("recovery lost complete tool or reasoning history")
	}
	wantTrigger := 0
	if kind == "compaction-v2" {
		wantTrigger = 1
	}
	if found["compaction_trigger"] != wantTrigger {
		t.Fatal("recovery changed the compaction trigger")
	}
}
