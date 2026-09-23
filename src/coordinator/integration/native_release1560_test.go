//go:build nativeparity

package integration

import (
	"fmt"
	"testing"
)

// Exercise the actual CLI with the advertised capability explicitly enabled. The
// shipped static catalog leaves this opt-in feature off; all endpoints remain local.
func TestNative1560ReasoningUpdates(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, ws := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/ws=%t", route, ws), func(t *testing.T) {
				capture := newNativeCapture(t)
				options := nativeOptions{endpoint: capture.server.URL, model: "gpt-6-astra", ws: ws,
					base: "Synthetic reasoning update capture", reasoningEffortUpdates: true,
					configOverrides: map[string]string{"features.reasoning_effort_override": "true"}}
				var gateway nativeGateway
				if route != "direct" {
					gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
					options.endpoint, options.bearer = gateway.server.URL, gateway.secret
				}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				for _, effort := range []string{"low", "high"} {
					client.turnWith(thread, map[string]any{"input": []any{map[string]any{"type": "text", "text": "Synthetic effort change"}}, "effort": effort})
				}
				wires := capture.snapshot()
				foundHigh := false
				for _, wire := range wires {
					items, _ := wire.value["input"].([]any)
					for _, raw := range items {
						item, _ := raw.(map[string]any)
						if item["type"] == "configuration_update" {
							reasoning, _ := item["reasoning"].(map[string]any)
							if reasoning["effort"] == nil || len(item) != 2 {
								t.Fatal("native configuration update lost its typed payload")
							}
							foundHigh = foundHigh || reasoning["effort"] == "high"
						}
					}
				}
				if !foundHigh {
					t.Fatal("actual native CLI did not emit the requested effort update")
				}
				if route == "subscription" {
					packets := gateway.tap.packets(t)
					if len(packets) != len(wires) {
						t.Fatal("native effort update request count changed")
					}
					for i := range packets {
						assertNativeMessageParity(t, packets[i], wires[i])
					}
				}
				t.Logf("actual v0.156.0 captured %d requests with preserved high-effort history control", len(wires))
			})
		}
	}
}
