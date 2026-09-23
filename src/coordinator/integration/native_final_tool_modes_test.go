//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
)

// The native tool plan has a direct-only namespace override even for code_mode_only models.
// The responder emits text only, so this probe does not fake calls to hidden tools.
func TestNativeFinalToolExposure(t *testing.T) {
	for _, host := range []bool{false, true} {
		for _, direct := range []bool{false, true} {
			for _, ws := range []bool{false, true} {
				for _, route := range []string{"direct", "api-key", "subscription"} {
					t.Run(fmt.Sprintf("host=%t/direct=%t/ws=%t/%s", host, direct, ws, route), func(t *testing.T) {
						capture := newNativeFailureCapture(t, "completed", 0, 0)
						endpoint, bearer := capture.server.URL, ""
						var gateway nativeGateway
						if route != "direct" {
							gateway = newNativeGateway(t, endpoint, route == "subscription")
							endpoint, bearer = gateway.server.URL, gateway.secret
						}
						overrides := map[string]string{"features.code_mode": "true", "features.code_mode_host": fmt.Sprint(host)}
						if direct {
							overrides["features.code_mode"] = `{enabled=true,direct_only_tool_namespaces=["functions"]}`
						}
						options := nativeOptions{endpoint: endpoint, bearer: bearer, model: "gpt-6-astra", ws: ws, base: "Answer the synthetic text task briefly.", configOverrides: overrides}
						client := startNativeClient(t, options)
						thread := client.thread(options)
						client.turn(thread, "Reply briefly without calling any tools.")
						wires := capture.snapshot()
						if len(businessWires(wires)) != 1 {
							t.Fatal("tool-exposure probe added business inference")
						}
						tools := finalTools(wires[0].value)
						exec, wait, probe, nested := false, false, false, false
						for _, tool := range tools {
							exec = exec || tool["name"] == "exec"
							wait = wait || tool["name"] == "wait"
							probe = probe || tool["name"] == "native_probe"
							if tool["name"] == "exec" {
								description, _ := tool["description"].(string)
								nested = strings.Contains(description, "native_probe")
							}
						}
						packets := capture.tap.packets(t)
						if route != "direct" {
							packets = gateway.tap.packets(t)
						}
						t.Logf("FINAL_TOOL_MODE host=%t direct=%t exec=%t wait=%t probe=%t nested=%t", host, direct, exec, wait, probe, nested)
						if !exec || !wait || probe != direct || nested == direct {
							t.Fatal("native tool exposure differs from source-backed profile")
						}
						for _, wire := range wires {
							if len(finalTools(wire.value)) != 0 {
								assertNativeLiteIDs(t, wire)
							}
						}
						if route != "direct" {
							packets := gateway.tap.packets(t)
							if len(packets) != len(wires) {
								t.Fatal("native tool exposure changed request count")
							}
							for i, wire := range wires {
								if route == "api-key" {
									if !bytes.Equal(packets[i].payload, wire.encodedBody) {
										t.Fatal("API-key tool exposure changed")
									}
								} else {
									assertNativeMessageParity(t, packets[i], wire)
								}
							}
						}
						saveFinalCapture(t, packets, wires)
					})
				}
			}
		}
	}
}

func TestNativeFinalCodeModeLoop(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, route := range []string{"direct", "api-key", "subscription"} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, route), func(t *testing.T) {
				capture := newNativeCapture(t)
				capture.mu.Lock()
				capture.codeModeLoop = true
				capture.mu.Unlock()
				endpoint, bearer := capture.server.URL, ""
				var gateway nativeGateway
				if route != "direct" {
					gateway = newNativeGateway(t, endpoint, route == "subscription")
					endpoint, bearer = gateway.server.URL, gateway.secret
				}
				calls := 0
				options := nativeOptions{endpoint: endpoint, bearer: bearer, model: "gpt-6-astra", ws: ws, base: "Synthetic code-mode tool loop.",
					configOverrides: map[string]string{"features.code_mode": "true", "features.code_mode_host": "true"},
					observe: func(event map[string]any) {
						if event["method"] == "item/tool/call" {
							calls++
						}
					},
				}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				client.turn(thread, "Run the synthetic nested probe.")
				client.turn(thread, "Next synthetic user turn.")
				wires := capture.snapshot()
				business := businessWires(wires)
				if calls != 1 || len(business) != 3 {
					t.Fatalf("code-mode nested tool/turn count differs: callbacks=%d requests=%d", calls, len(business))
				}
				found := false
				input, _ := business[1].value["input"].([]any)
				for _, raw := range input {
					item, _ := raw.(map[string]any)
					if item["type"] == "custom_tool_call_output" {
						output, _ := json.Marshal(item["output"])
						if !bytes.Contains(output, []byte("synthetic tool result")) {
							t.Fatal("code-mode host did not return the nested callback result")
						}
						found = true
					}
				}
				if !found {
					t.Fatal("code-mode tool output absent from actual continuation")
				}
				for _, wire := range wires {
					if len(finalTools(wire.value)) > 0 {
						assertNativeLiteIDs(t, wire)
					}
				}
				packets := capture.tap.packets(t)
				if route != "direct" {
					packets = gateway.tap.packets(t)
					if len(packets) != len(wires) {
						t.Fatal("code-mode gateway changed request count")
					}
					for i, wire := range wires {
						if route == "api-key" {
							if !bytes.Equal(packets[i].payload, wire.encodedBody) {
								t.Fatal("API-key code-mode frame changed")
							}
						} else {
							assertNativeMessageParity(t, packets[i], wire)
						}
					}
				}
				saveFinalCapture(t, packets, wires)
			})
		}
	}
}

func finalTools(value map[string]any) []map[string]any {
	raw := value["tools"]
	input, _ := value["input"].([]any)
	for _, rawItem := range input {
		item, _ := rawItem.(map[string]any)
		if item["type"] == "additional_tools" {
			raw = item["tools"]
			break
		}
	}
	var out []map[string]any
	var visit func(any)
	visit = func(raw any) {
		items, _ := raw.([]any)
		for _, raw := range items {
			tool, _ := raw.(map[string]any)
			if tool["type"] == "namespace" {
				visit(tool["tools"])
			} else if tool != nil {
				out = append(out, tool)
			}
		}
	}
	visit(raw)
	return out
}
