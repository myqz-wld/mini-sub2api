//go:build nativeparity

package integration

import (
	"encoding/json"
	"reflect"
	"testing"
)

func TestClassifierReferencePreservesContextAndRoleDefaults(t *testing.T) {
	for _, transport := range []string{"http", "same-ws", "reconnected-ws"} {
		for _, effort := range []struct {
			name, expectedString string
			supplied             any
		}{
			{name: "omitted", expectedString: "low"},
			{name: "explicit", supplied: "high", expectedString: "high"},
			{name: "numeric", supplied: 8192},
			{name: "numeric-string", supplied: "8192"},
		} {
			t.Run(transport+"/"+effort.name, func(t *testing.T) {
				capture := newNativeCapture(t)
				capture.respond = func(_ map[string]any, id string, _ bool) []map[string]any {
					item := map[string]any{"type": "message", "id": "msg_" + id, "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": "low"}}}
					return []map[string]any{
						{"type": "response.created", "response": map[string]any{"id": id}},
						{"type": "response.output_item.done", "output_index": 0, "item": item},
						{"type": "response.completed", "response": map[string]any{"id": id, "output": []any{item}}},
					}
				}
				gateway := newNativeGateway(t, capture.server.URL, true)
				sourceClient := ordinaryClient(t, gateway, false, false, nil)
				sourceClient.send(map[string]any{"model": "gpt-5.5", "input": "synthetic source", "client_metadata": map[string]any{"session_id": "reference-source", "thread_id": "reference-source", "turn_id": "source-turn"}})
				classifierBody := func(turnID string, input []any) map[string]any {
					turn := map[string]any{"session_id": "reference-source", "thread_id": "reference-lease", "turn_id": turnID, "parent_turn_id": "source-turn", "guardian_classifier_source_thread_id": "reference-source", "thread_source": "guardian_classifier", "turn_trigger": "guardian_classifier"}
					encoded, _ := json.Marshal(turn)
					body := map[string]any{"model": "gpt-5.6-luna", "include": []any{"reasoning.encrypted_content"}, "input": input,
						"client_metadata": map[string]any{"session_id": "reference-source", "thread_id": "reference-lease", "turn_id": turnID, "parent_turn_id": "source-turn", "x-codex-turn-metadata": string(encoded)}}
					if effort.supplied != nil {
						body["reasoning"] = map[string]any{"effort": effort.supplied}
					}
					return body
				}
				client := ordinaryClient(t, gateway, transport != "http", true, nil)
				// Supplied IDs have durable aliases; omitted message IDs are generated per turn.
				evidence := compactionUser("first synthetic evidence")
				evidence["id"] = "msg_reference_evidence"
				first := classifierBody("classify-one", []any{
					map[string]any{"type": "additional_tools", "id": "at_reference", "role": "developer", "tools": []any{}},
					map[string]any{"type": "message", "id": "msg_reference_base", "role": "developer", "content": []any{map[string]any{"type": "input_text", "text": "synthetic classifier base"}}},
					evidence,
				})
				completed := client.send(first)
				if transport == "reconnected-ws" {
					_ = client.ws.CloseNow()
					client = ordinaryClient(t, gateway, true, true, nil)
				}
				next := classifierBody("classify-two", []any{compactionUser("second synthetic evidence")})
				next["previous_response_id"] = completed["id"]
				client.send(next)
				wires := businessWires(capture.snapshot())
				if len(wires) != 3 {
					t.Fatalf("business request count=%d want=3", len(wires))
				}
				last := wires[2].value
				input, _ := last["input"].([]any)
				if len(input) != 5 || last["previous_response_id"] != nil {
					t.Fatalf("classifier reference input count=%d want=5; reference present=%t", len(input), last["previous_response_id"] != nil)
				}
				if !reflect.DeepEqual(input[:3], wires[1].value["input"]) {
					t.Fatal("classifier source setup or evidence changed")
				}
				expected := any(float64(8192))
				if effort.expectedString != "" {
					expected = effort.expectedString
				}
				for _, wire := range wires[1:] {
					reasoning, _ := wire.value["reasoning"].(map[string]any)
					if reasoning["effort"] != expected {
						t.Fatal("classifier effort changed across reference reconstruction")
					}
					if wire.value["tool_choice"] != "none" || wire.value["text"] != nil {
						t.Fatal("classifier controls changed across reconstruction")
					}
				}
			})
		}
	}
}
