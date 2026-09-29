//go:build nativeparity

package integration

import (
	"encoding/json"
	"fmt"
	"testing"
)

// Compare actual 0.158.0 serialization with both gateway credential paths.
// Configuration-update items and turn metadata still use the selected string.
func TestNative1580NumericReasoningEffort(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, ws := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/ws=%t", route, ws), func(t *testing.T) {
				capture := newNativeCapture(t)
				options := nativeOptions{endpoint: capture.server.URL, model: "gpt-6-astra", ws: ws,
					base:            "Synthetic numeric effort capture",
					configOverrides: map[string]string{"model_reasoning_effort": `"8192"`}}
				var gateway nativeGateway
				if route != "direct" {
					gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
					options.endpoint, options.bearer = gateway.server.URL, gateway.secret
				}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				client.turn(thread, "First numeric effort probe")
				client.turn(thread, "Next numeric effort probe")
				wires := capture.snapshot()
				if len(businessWires(wires)) < 2 {
					t.Fatal("numeric effort capture missed multi-turn traffic")
				}
				for _, wire := range wires {
					reasoning, _ := wire.value["reasoning"].(map[string]any)
					if reasoning["effort"] != float64(8192) {
						t.Fatal("native numeric effort was changed or serialized as a string")
					}
					metadata, _ := wire.value["client_metadata"].(map[string]any)
					raw, _ := metadata["x-codex-turn-metadata"].(string)
					var turn map[string]any
					if json.Unmarshal([]byte(raw), &turn) != nil {
						t.Fatal("numeric effort turn metadata missing")
					}
					if turn["request_kind"] != "prewarm" && turn["reasoning_effort"] != "8192" {
						t.Fatal("numeric wire effort changed the selected metadata value")
					}
				}
				if route == "subscription" {
					packets := gateway.tap.packets(t)
					if len(packets) != len(wires) {
						t.Fatal("numeric effort request sequence changed")
					}
					for i := range packets {
						assertNativeMessageParity(t, packets[i], wires[i])
					}
				}
				t.Logf("NUMERIC_EFFORT_CAPTURE requests=%d numeric_wire=true string_metadata=true", len(wires))
			})
		}
	}
}
