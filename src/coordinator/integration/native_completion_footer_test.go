//go:build nativeparity

package integration

import (
	"fmt"
	"testing"
)

func TestNativeMetadataOnlyCompletion(t *testing.T) {
	for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
		for _, delivery := range []string{"json", "sse", "ws"} {
			t.Run(fmt.Sprintf("%s/%s", model, delivery), func(t *testing.T) {
				capture := newNativeCaptureWithFooter(t, true)
				gateway := newNativeGateway(t, capture.server.URL, true)
				client := ordinaryClient(t, gateway, delivery == "ws", delivery != "json", nil)
				tools := []any{map[string]any{"type": "function", "name": "native_probe", "parameters": map[string]any{"type": "object"}}}
				first := client.send(map[string]any{"model": model, "input": "Synthetic tool task.", "instructions": "Use only the supplied tool.", "tools": tools})
				items, _ := first["output"].([]any)
				if len(items) != 1 {
					t.Fatal("completed tool item missing from public response")
				}
				call := items[0].(map[string]any)
				if call["type"] != "function_call" {
					t.Fatal("completed tool item changed")
				}
				second := client.send(map[string]any{"model": model, "previous_response_id": first["id"], "instructions": "Use only the supplied tool.", "tools": tools, "input": []any{map[string]any{"type": "function_call_output", "call_id": call["call_id"], "output": "synthetic result"}}})
				items, _ = second["output"].([]any)
				if len(items) != 1 || items[0].(map[string]any)["type"] != "message" {
					t.Fatal("completed assistant output missing")
				}
				wires := businessWires(capture.snapshot())
				if len(wires) != 2 {
					t.Fatal("metadata-only footer added or lost an inference")
				}
				if delivery == "ws" {
					if wires[1].value["previous_response_id"] == nil {
						t.Fatal("metadata-only footer discarded the WS baseline")
					}
				} else {
					if wires[1].value["previous_response_id"] != nil || len(wires[1].value["input"].([]any)) < 3 {
						t.Fatal("metadata-only footer prevented full HTTP history reconstruction")
					}
				}
			})
		}
	}
}
