//go:build nativeparity && liveparity

package integration

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"
)

func TestLiveSubscriptionBare(t *testing.T) {
	gateway := newLiveSubscriptionGateway(t)
	for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
		for _, delivery := range []string{"json", "sse", "ws"} {
			t.Run(fmt.Sprintf("%s/%s", model, delivery), func(t *testing.T) {
				client := ordinaryClient(t, gateway, delivery == "ws", delivery != "json", nil)
				first := liveResponse(t, client, map[string]any{"model": model, "input": "Reply with exactly the single word PASS. Do not use tools.", "reasoning": map[string]any{"effort": "low"}})
				if strings.TrimSpace(liveText(first)) != "PASS" {
					describeLiveResponse(t, first, "PASS")
					t.Fatal("live minimal text task did not return the requested answer")
				}
				second := liveResponse(t, client, map[string]any{"model": model, "previous_response_id": first["id"], "input": "Reply with exactly the single word NEXT. Do not use tools.", "reasoning": map[string]any{"effort": "low"}})
				if strings.TrimSpace(liveText(second)) != "NEXT" {
					t.Fatal("live incremental text task did not return the requested answer")
				}
			})
		}
	}
}

func TestLiveSubscriptionToolAndSchema(t *testing.T) {
	gateway := newLiveSubscriptionGateway(t)
	for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
		for _, delivery := range []string{"sse", "ws"} {
			t.Run(fmt.Sprintf("%s/%s", model, delivery), func(t *testing.T) {
				client := ordinaryClient(t, gateway, delivery == "ws", true, nil)
				tools := []any{map[string]any{"type": "function", "name": "probe", "description": "Return a synthetic test value.", "parameters": map[string]any{"type": "object", "properties": map[string]any{}, "additionalProperties": false}, "strict": true}}
				base := "Call probe once when asked. After receiving its result, reply with exactly PASS. Do not use other tools."
				first := liveResponse(t, client, map[string]any{"model": model, "instructions": base, "input": "Call probe now.", "tools": tools, "reasoning": map[string]any{"effort": "low"}})
				items, _ := first["output"].([]any)
				var call map[string]any
				for _, raw := range items {
					item, _ := raw.(map[string]any)
					if item["type"] == "function_call" {
						if call != nil {
							t.Fatal("live model emitted more than the requested tool call")
						}
						call = item
					}
				}
				if call == nil || call["name"] != "probe" || call["call_id"] == nil {
					t.Fatal("live caller tool was not selected")
				}
				second := liveResponse(t, client, map[string]any{"model": model, "instructions": base, "previous_response_id": first["id"], "input": []any{map[string]any{"type": "function_call_output", "call_id": call["call_id"], "output": "synthetic success"}}, "tools": tools, "reasoning": map[string]any{"effort": "low"}})
				if strings.TrimSpace(liveText(second)) != "PASS" {
					t.Fatal("live tool continuation did not complete the requested task")
				}
				format := map[string]any{"type": "json_schema", "name": "verdict", "strict": true, "schema": map[string]any{"type": "object", "properties": map[string]any{"ok": map[string]any{"type": "boolean"}}, "required": []any{"ok"}, "additionalProperties": false}}
				structured := liveResponse(t, client, map[string]any{"model": model, "instructions": "Return the requested JSON object.", "input": "Return an object with ok set to true.", "text": map[string]any{"format": format}, "reasoning": map[string]any{"effort": "low"}})
				var value map[string]any
				if json.Unmarshal([]byte(liveText(structured)), &value) != nil || len(value) != 1 || value["ok"] != true {
					t.Fatal("live structured output contract did not hold")
				}
			})
		}
	}
}
