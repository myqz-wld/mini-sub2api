//go:build nativeparity

package integration

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"time"
)

func TestNative1580ObservationBudgetAtUpstream(t *testing.T) {
	const meta = "internal_chat_message_metadata_passthrough"
	for _, ws := range []bool{false, true} {
		for _, kind := range []string{"prompt", "message"} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, kind), func(t *testing.T) {
				if kind == "message" {
					// Keep a full logical baseline plus the next 15 MiB assembly resident.
					// Default cache eviction is covered separately; it legitimately forces full WS.
					t.Setenv("MINI_SUB2API_LIMITS", `{"sessionBytes":536870912}`)
				}
				capture := newNativeCapture(t)
				capture.business = 1
				gateway := newNativeGateway(t, capture.server.URL, true)
				client := ordinaryClient(t, gateway, ws, true, nil)
				client.timeout = 90 * time.Second
				result := map[string]any{"openai/resource_access": map[string]any{"id": "synthetic-resource"}}
				argument := "small"
				content := "synthetic"
				if kind == "prompt" {
					result["large"] = strings.Repeat("x", 2*1024*1024)
				}
				if kind == "message" {
					content = strings.Repeat("p", 15*1024*1024-6000)
					argument = strings.Repeat("a", 8000)
				}
				input := []any{map[string]any{"role": "user", "content": content},
					map[string]any{"type": "function_call", "call_id": "budget-call", "name": "probe", "arguments": "{}"},
					map[string]any{"type": "function_call_output", "call_id": "budget-call", "output": "synthetic tool result", meta: map[string]any{
						"cell_id": "budget-cell", "executed_tool_calls": []any{map[string]any{"name": "tools.probe", "arguments": map[string]any{"data": argument}, "tool_result_metadata": result}}, "tool_calls_complete": true}}}
				body := map[string]any{"model": "gpt-5.5", "input": input, "tools": []any{map[string]any{"type": "function", "name": "probe", "parameters": map[string]any{"type": "object"}}}}
				response := client.send(body)
				wires := businessWires(capture.snapshot())
				if len(wires) != 1 {
					t.Fatal("observation fixture missed its business request")
				}
				if len(wires[0].body) > 15*1024*1024 {
					remaining := 0
					for _, raw := range wires[0].value["input"].([]any) {
						item, _ := raw.(map[string]any)
						if metadata, ok := item[meta]; ok {
							encoded, _ := json.Marshal(metadata)
							remaining += len(encoded)
						}
					}
					t.Fatalf("optional observations exceeded message budget: bytes=%d metadata_bytes=%d", len(wires[0].body), remaining)
				}
				items := wires[0].value["input"].([]any)
				last := items[len(items)-1].(map[string]any)
				observation := last[meta].(map[string]any)
				call := observation["executed_tool_calls"].([]any)[0].(map[string]any)
				if call["tool_result_metadata"].(map[string]any)["openai/resource_access"] == nil || last["output"] != "synthetic tool result" {
					t.Fatal("budgeting lost resource evidence or business output")
				}
				if kind == "prompt" {
					if call["tool_result_metadata"].(map[string]any)["large"] != nil || observation["tool_calls_complete"] != true {
						t.Fatal("prompt budget changed inventory completeness")
					}
					return
				}
				if call["arguments"].(map[string]any)["_codex_executed_tool_call_truncated"] == nil || observation["tool_calls_complete"] != nil {
					t.Fatal("message inventory loss was not marked")
				}
				output := response["output"].([]any)
				input = append(input, output...)
				input = append(input, map[string]any{"type": "function_call", "call_id": "wait-call", "name": "probe", "arguments": "{}"},
					map[string]any{"type": "function_call_output", "call_id": "wait-call", "output": "synthetic wait", meta: map[string]any{"cell_id": "budget-cell", "executed_tool_calls": []any{}, "tool_calls_complete": true}},
					map[string]any{"role": "user", "content": "next synthetic turn"})
				body["input"] = input
				client.send(body)
				wires = businessWires(capture.snapshot())
				if len(wires) != 2 {
					t.Fatal("observation continuation count changed")
				}
				if ws && (wires[1].value["previous_response_id"] == nil || len(wires[1].body) > 64*1024) {
					t.Fatalf("wire-only shedding corrupted the reusable logical WS history: previous=%t bytes=%d", wires[1].value["previous_response_id"] != nil, len(wires[1].body))
				}
				for _, raw := range wires[1].value["input"].([]any) {
					item := raw.(map[string]any)
					if item["type"] == "function_call_output" {
						metadata, _ := item[meta].(map[string]any)
						if metadata["tool_calls_complete"] != nil {
							t.Fatal("later output restored revoked cell completeness")
						}
					}
				}
				encoded, _ := json.Marshal(wires[1].value)
				t.Logf("OBSERVATION_CAPTURE ws=%t continuation_bytes=%d logical_history_preserved=true", ws, len(encoded))
			})
		}
	}
}
