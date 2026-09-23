//go:build nativeparity && liveparity

package integration

import (
	"strings"
	"testing"
)

func TestLiveSubscriptionToolReconnect(t *testing.T) {
	for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
		t.Run(model, func(t *testing.T) {
			gateway := newLiveSubscriptionGateway(t)
			client := ordinaryClient(t, gateway, true, true, nil)
			tools := []any{map[string]any{"type": "function", "name": "probe", "description": "Return a synthetic test value.", "parameters": map[string]any{"type": "object", "properties": map[string]any{}, "additionalProperties": false}, "strict": true}}
			base := "Call probe exactly once when asked. After its tool result reply only PASS. For a subsequent plain text task reply with the requested word without calling tools."
			first := liveResponse(t, client, map[string]any{"model": model, "instructions": base, "input": "Call probe now.", "tools": tools, "reasoning": map[string]any{"effort": "low"}})
			var call map[string]any
			items, _ := first["output"].([]any)
			for _, raw := range items {
				item, _ := raw.(map[string]any)
				if item["type"] == "function_call" {
					if call != nil {
						t.Fatal("live reconnect tool-call bound exceeded")
					}
					call = item
				}
			}
			if call == nil || call["name"] != "probe" || call["call_id"] == nil {
				t.Fatal("live reconnect tool call missing")
			}
			_ = client.ws.CloseNow()
			client = ordinaryClient(t, gateway, true, true, nil)
			second := liveResponse(t, client, map[string]any{"model": model, "instructions": base, "previous_response_id": first["id"], "input": []any{map[string]any{"type": "function_call_output", "call_id": call["call_id"], "output": "synthetic success"}}, "tools": tools, "reasoning": map[string]any{"effort": "low"}})
			if strings.TrimSpace(liveText(second)) != "PASS" {
				t.Fatal("live reconnected tool return did not complete")
			}
			third := liveResponse(t, client, map[string]any{"model": model, "instructions": base, "previous_response_id": second["id"], "input": "Reply exactly NEXT without tools.", "tools": tools, "reasoning": map[string]any{"effort": "low"}})
			if strings.TrimSpace(liveText(third)) != "NEXT" {
				t.Fatal("live reconnected next turn did not complete")
			}
			t.Log("LIVE_RECONNECT original_socket_closed=true tool_reference_restored=true next_turn_verified=true")
		})
	}
}
