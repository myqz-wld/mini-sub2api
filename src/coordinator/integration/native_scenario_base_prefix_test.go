//go:build nativeparity

package integration

import (
	"fmt"
	"reflect"
	"testing"
)

func TestNativeScenarioBaseOrdinaryPrefixMutations(t *testing.T) {
	for _, ws := range []bool{false, true} {
		t.Run(fmt.Sprintf("ws=%t", ws), func(t *testing.T) {
			capture := newNativeCapture(t)
			gateway := newNativeGateway(t, capture.server.URL, true)
			client := ordinaryClient(t, gateway, ws, !ws, nil)
			tools := []any{
				map[string]any{"type": "function", "name": "native_probe", "description": "Synthetic first tool", "parameters": map[string]any{"type": "object", "properties": map[string]any{}}},
				map[string]any{"type": "function", "name": "second_probe", "description": "Synthetic second tool", "parameters": map[string]any{"type": "object", "properties": map[string]any{}}},
			}
			base := " base whitespace matters "
			history := []any{map[string]any{"type": "message", "role": "user", "content": "synthetic prefix seed"}}
			var first, previous nativeWire
			for step := 0; step < 5; step++ {
				if step == 2 {
					base = "base whitespace matters"
				}
				if step == 3 {
					tools[0].(map[string]any)["description"] = "Synthetic changed tool"
				}
				if step == 4 {
					tools[0], tools[1] = tools[1], tools[0]
				}
				response := client.send(map[string]any{"model": "gpt-5.6-sol", "instructions": base, "tools": tools, "input": history, "client_metadata": map[string]any{"session_id": "018f121a-0000-7000-8000-000000000001"}})
				business := businessWires(capture.snapshot())
				if len(business) != step+1 {
					t.Fatal("ordinary prefix mutation duplicated or omitted a sampling")
				}
				current := business[len(business)-1]
				if step == 0 {
					// A gateway warmup may already hold the initial full prefix.
					for _, wire := range capture.snapshot() {
						items, _ := wire.value["input"].([]any)
						if len(items) > 0 && items[0].(map[string]any)["type"] == "additional_tools" {
							first = wire
							break
						}
					}
					if first.value == nil {
						t.Fatal("ordinary prefix seed missing")
					}
					assertScenarioBaseValue(t, first, true, base)
					previous = first
				}
				if step == 1 && ws {
					if current.value["previous_response_id"] == nil {
						t.Fatal("unchanged ordinary full context did not reuse the WS baseline")
					}
				} else if step >= 2 {
					if current.value["previous_response_id"] != nil {
						t.Fatal("changed ordinary Lite prefix reused a stale upstream baseline")
					}
					assertScenarioBaseValue(t, current, true, base)
					oldMeta := previous.value["client_metadata"].(map[string]any)
					newMeta := current.value["client_metadata"].(map[string]any)
					if oldMeta["thread_id"] != newMeta["thread_id"] {
						t.Fatal("ordinary prefix mutation changed the thread namespace")
					}
					old := previous.value["input"].([]any)
					newItems := current.value["input"].([]any)
					toolChanged := old[0].(map[string]any)["id"] != newItems[0].(map[string]any)["id"]
					baseChanged := old[1].(map[string]any)["id"] != newItems[1].(map[string]any)["id"]
					if toolChanged != (step >= 3) || baseChanged != (step == 2) {
						t.Fatal("same-thread deterministic prefix IDs depend on the wrong payload")
					}
					if step == 2 && !reflect.DeepEqual(old[0].(map[string]any)["tools"], newItems[0].(map[string]any)["tools"]) {
						t.Fatal("base mutation changed tools")
					}
					previous = current
				}
				output, _ := response["output"].([]any)
				history = append(history, output...)
				if step == 0 {
					call := output[0].(map[string]any)
					history = append(history, map[string]any{"type": "function_call_output", "call_id": call["call_id"], "output": "synthetic tool completion"})
				} else {
					history = append(history, map[string]any{"type": "message", "role": "user", "content": "synthetic next prefix turn"})
				}
			}
		})
	}
}
