//go:build nativeparity

package integration

import (
	"bytes"
	"fmt"
	"strings"
	"testing"
)

func TestNative1620IncrementalDeclarations(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, route := range []string{"direct", "api-key", "subscription"} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, route), func(t *testing.T) {
				capture := newNativeCapture(t)
				capture.business = 1 // Text-only responses; the native client owns catalog changes.
				options := nativeOptions{endpoint: capture.server.URL, model: "gpt-6-astra", ws: ws, base: "Synthetic incremental tools",
					configOverrides: map[string]string{"features.incremental_tools": "true", "features.code_mode": "true"},
					catalogTransform: func(catalog map[string]any) {
						for _, raw := range catalog["models"].([]any) {
							model := raw.(map[string]any)
							if model["slug"] == "gpt-6-sol" {
								messages := model["model_messages"].(map[string]any)
								messages["tools"] = map[string]any{"code_mode": map[string]any{"exec": map[string]any{"description": "Synthetic changed execution description"}}}
							}
							if model["slug"] == "gpt-6-luna" {
								model["tool_mode"] = "direct"
							}
						}
					},
				}
				var gateway nativeGateway
				if route != "direct" {
					gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
					options.endpoint, options.bearer = gateway.server.URL, gateway.secret
				}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				for _, model := range []string{"gpt-6-astra", "gpt-6-astra", "gpt-6-sol", "gpt-6-sol", "gpt-6-luna", "gpt-6-astra"} {
					client.turnWith(thread, map[string]any{"model": model, "input": []any{map[string]any{"type": "text", "text": "Synthetic tool catalog probe; reply briefly."}}})
				}
				wires := capture.snapshot()
				if len(businessWires(wires)) != 6 {
					t.Fatal("incremental declaration sampling count changed")
				}
				changed, removed := false, false
				for _, wire := range wires {
					items, _ := wire.value["input"].([]any)
					for _, raw := range items {
						item, _ := raw.(map[string]any)
						if item["type"] == "additional_tools" {
							changed = changed || bytes.Contains(mustRequestJSON(t, item), []byte("Synthetic changed execution description"))
						}
						if text, ok := developerMessageText(item); ok {
							removed = removed || strings.Contains(text, "no longer available")
						}
					}
				}
				if !changed || !removed {
					t.Fatalf("native incremental variants absent: changed=%t removal=%t", changed, removed)
				}
				packets := capture.tap.packets(t)
				if route != "direct" {
					packets = gateway.tap.packets(t)
					if len(packets) != len(wires) {
						t.Fatal("incremental request count changed at gateway")
					}
					for i, wire := range wires {
						if route == "api-key" {
							if !bytes.Equal(packets[i].payload, wire.encodedBody) {
								t.Fatal("API-key incremental bytes changed")
							}
						} else {
							assertNativeMessageParity(t, packets[i], wire)
						}
					}
				}
				saveFinalCapture(t, packets, wires)
				t.Logf("native_incremental_requests=%d redefinition=true removal=true", len(wires))
			})
		}
	}
}
