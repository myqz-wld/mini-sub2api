//go:build nativeparity

package integration

import (
	"bytes"
	"fmt"
	"testing"
)

// This is a source-backed ordinary caller probe, not a native-generated request.
// Distinct prewarm and business tokens make late adoption observable in memory.
func TestNativeOrdinaryPrewarmRouting(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, format := range []string{"ordinary", "converted-lite"} {
			for _, explicit := range []bool{false, true} {
				t.Run(fmt.Sprintf("subscription=%t/%s/explicit-prewarm=%t", subscription, format, explicit), func(t *testing.T) {
					capture := newNativeTransportCapture(t, false, false)
					capture.prewarmToken = true
					gateway := newNativeGateway(t, capture.server.URL, subscription)
					client := ordinaryClient(t, gateway, true, true, nil)
					if explicit {
						prewarm := ordinaryRequest(format, "session")
						prewarm["input"], prewarm["generate"] = []any{}, false
						client.send(prewarm)
					}
					first := ordinaryRequest(format, "turn")
					result := client.send(first)
					for step := 0; step < 2; step++ {
						var call any
						output, _ := result["output"].([]any)
						for _, raw := range output {
							item, _ := raw.(map[string]any)
							if item["type"] == "function_call" {
								call = item["call_id"]
							}
						}
						if call == nil {
							t.Fatal("probe missing completed tool reference")
						}
						next := map[string]any{"model": first["model"], "instructions": first["instructions"], "tools": first["tools"], "previous_response_id": result["id"], "input": []any{map[string]any{"type": "function_call_output", "call_id": call, "output": "synthetic boundary result"}}}
						result = client.send(next)
					}
					wires := businessWires(capture.snapshot())
					if len(wires) != 3 {
						t.Fatal("probe business count changed")
					}
					for i, wire := range wires {
						metadata, _ := wire.value["client_metadata"].(map[string]any)
						if subscription && metadata["x-codex-turn-state"] != "transport-prewarm-token" {
							t.Errorf("prewarm routing token missing or replaced at business index=%d", i)
						}
					}
					packets, raw := gateway.tap.packets(t), capture.tap.packets(t)
					all := capture.snapshot()
					if len(raw) != len(all) {
						t.Fatal("probe raw/application count differs")
					}
					for i := range raw {
						if !bytes.Equal(raw[i].payload, all[i].encodedBody) {
							t.Fatal("probe raw/application payload differs")
						}
					}
					if !subscription {
						if len(packets) != len(all) {
							t.Fatal("API-key added setup")
						}
						for i := range packets {
							if !bytes.Equal(packets[i].payload, all[i].encodedBody) {
								t.Fatal("API-key probe payload changed")
							}
						}
					}
					t.Logf("bounded prewarm probe: business=%d upstream_requests=%d ingress_requests=%d", len(wires), len(all), len(packets))
				})
			}
		}
	}
}

func TestNativePrewarmThreadIntervention(t *testing.T) {
	for _, intervene := range []bool{false, true} {
		t.Run(fmt.Sprintf("child-intervenes=%t", intervene), func(t *testing.T) {
			capture := newNativeTransportCapture(t, false, false)
			capture.prewarmToken = true
			gateway := newNativeGateway(t, capture.server.URL, true)
			client := ordinaryClient(t, gateway, true, true, nil)
			prewarm := ordinaryRequest("ordinary", "session")
			prewarm["input"], prewarm["generate"] = []any{}, false
			prewarm["client_metadata"] = map[string]any{"session_id": "ordinary-session", "thread_id": "warmed-child", "parent_thread_id": "ordinary-session"}
			client.send(prewarm)
			if intervene {
				child := ordinaryRequest("ordinary", "turn")
				child["client_metadata"] = map[string]any{"session_id": "ordinary-session", "thread_id": "intervening-child", "parent_thread_id": "ordinary-session", "turn_id": "child-business"}
				client.send(child)
			}
			owner := ordinaryRequest("ordinary", "turn")
			owner["client_metadata"] = map[string]any{"session_id": "ordinary-session", "thread_id": "warmed-child", "parent_thread_id": "ordinary-session", "turn_id": "owner-first-business"}
			client.send(owner)
			wires := businessWires(capture.snapshot())
			all := capture.snapshot()
			if intervene && transportMetadata(t, wires[0].value)["x-codex-turn-state"] != nil {
				t.Error("intervening thread adopted another thread startup token")
			}
			last := wires[len(wires)-1]
			prewarmMetadata := transportMetadata(t, all[0].value)
			lastMetadata := transportMetadata(t, last.value)
			if lastMetadata["thread_id"] != prewarmMetadata["thread_id"] || last.connection != all[0].connection {
				t.Fatal("owner did not return on the prewarm thread and physical socket")
			}
			childIsolated := true
			if intervene {
				childMetadata := transportMetadata(t, wires[0].value)
				childIsolated = childMetadata["x-codex-turn-state"] == nil && childMetadata["thread_id"] != lastMetadata["thread_id"] && childMetadata["session_id"] == lastMetadata["session_id"]
				if !childIsolated {
					t.Error("thread isolation relationship changed")
				}
			}
			if transportMetadata(t, last.value)["x-codex-turn-state"] != "transport-prewarm-token" {
				t.Error("owning thread lost startup token before its first business admission")
			}
			t.Logf("thread startup probe: business=%d", len(wires))
		})
	}
}
