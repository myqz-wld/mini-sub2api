//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"testing"
	"time"
)

func nativeClassifier(value map[string]any) bool {
	metadata, _ := value["client_metadata"].(map[string]any)
	raw, _ := metadata["x-codex-turn-metadata"].(string)
	var turn map[string]any
	_ = json.Unmarshal([]byte(raw), &turn)
	return turn["thread_source"] == "guardian_classifier"
}

func TestNative1620GuardianModes(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, mode := range []string{"snapshot", "conversation"} {
			for _, route := range []string{"direct", "api-key", "subscription"} {
				t.Run(fmt.Sprintf("ws=%t/%s/%s", ws, mode, route), func(t *testing.T) {
					capture := newNativeCapture(t)
					sampling := 0
					capture.respond = func(value map[string]any, id string, prewarm bool) []map[string]any {
						output := []any{}
						events := []map[string]any{{"type": "response.created", "response": map[string]any{"id": id}}}
						if nativeClassifier(value) {
							flat := value["client_metadata"].(map[string]any)
							metadata := map[string]any{"turn_id": flat["turn_id"]}
							output = append(output, map[string]any{"type": "reasoning", "id": "rs_" + id, "summary": []any{}, "encrypted_content": "synthetic-guardian-cipher", "internal_chat_message_metadata_passthrough": metadata},
								map[string]any{"type": "message", "id": "msg_" + id, "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": "low"}}, "internal_chat_message_metadata_passthrough": metadata})
							events = append(events, map[string]any{"type": "response.output_text.delta", "output_index": 1, "content_index": 0, "item_id": "msg_" + id, "delta": "low"})
						} else if !prewarm {
							sampling++
							if sampling <= 2 {
								output = append(output, map[string]any{"type": "function_call", "id": "fc_" + id, "call_id": "call_" + id, "name": "native_probe", "arguments": "{}"})
							} else {
								output = append(output, map[string]any{"type": "message", "id": "msg_" + id, "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": "synthetic answer"}}})
							}
						}
						for i, item := range output {
							events = append(events, map[string]any{"type": "response.output_item.done", "output_index": i, "item": item})
						}
						return append(events, map[string]any{"type": "response.completed", "response": map[string]any{"id": id, "output": output, "usage": map[string]any{"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}})
					}
					options := nativeOptions{endpoint: capture.server.URL, model: "gpt-5.5", ws: ws, base: "Synthetic Guardian tool loop",
						threadParams:    map[string]any{"approvalsReviewer": "auto_review"},
						configOverrides: map[string]string{"features.guardianv2": fmt.Sprintf(`{enabled=true,review_scope={computer_use_only=false},async_classifier_mode=%q}`, mode)}}
					var gateway nativeGateway
					if route != "direct" {
						gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
						options.endpoint, options.bearer = gateway.server.URL, gateway.secret
					}
					callbacks := 0
					options.observe = func(event map[string]any) {
						if event["method"] != "item/tool/call" {
							return
						}
						callbacks++
						if callbacks > 1 {
							return
						}
						deadline := time.Now().Add(5 * time.Second)
						for {
							count := 0
							for _, wire := range capture.snapshot() {
								if nativeClassifier(wire.value) {
									count++
								}
							}
							if count >= callbacks {
								break
							}
							if time.Now().After(deadline) {
								t.Fatalf("native classifier did not run: callbacks=%d count=%d", callbacks, count)
							}
							time.Sleep(20 * time.Millisecond)
						}
						time.Sleep(100 * time.Millisecond)
					}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					client.turn(thread, "Run two synthetic callbacks in this turn.")
					deadline := time.Now().Add(5 * time.Second)
					for {
						count := 0
						for _, wire := range capture.snapshot() {
							if nativeClassifier(wire.value) {
								count++
							}
						}
						if count >= 2 {
							break
						}
						if time.Now().After(deadline) {
							break
						}
						time.Sleep(20 * time.Millisecond)
					}
					wires := capture.snapshot()
					var classifiers []nativeWire
					for _, wire := range wires {
						if nativeClassifier(wire.value) {
							classifiers = append(classifiers, wire)
						}
					}
					if len(classifiers) != 2 {
						t.Fatal("native classifier sent an unexpected attempt count")
					}
					for _, wire := range classifiers {
						if wire.value["previous_response_id"] != nil {
							t.Fatal("native classifier used a delta")
						}
						include, _ := wire.value["include"].([]any)
						if (mode == "snapshot" && len(include) != 0) || (mode == "conversation" && (len(include) != 1 || include[0] != "reasoning.encrypted_content")) {
							t.Fatal("classifier include mode differs")
						}
						input := wire.value["input"].([]any)
						prefix := input[0].(map[string]any)
						if prefix["type"] != "additional_tools" || len(prefix["tools"].([]any)) != 0 {
							t.Fatal("classifier dedicated empty tools absent")
						}
					}
					retained := bytes.Contains(classifiers[1].body, []byte("synthetic-guardian-cipher"))
					if retained != (mode == "conversation") {
						t.Fatal("native retained mode did not preserve completed ciphertext")
					}
					packets := capture.tap.packets(t)
					if route != "direct" {
						packets = gateway.tap.packets(t)
						if len(packets) != len(wires) {
							t.Fatal("Guardian gateway count mismatch")
						}
						// Sampling and background classifiers can arrive in a different global
						// order on the two taps. Each producer's own sequence remains ordered.
						for _, classifier := range []bool{false, true} {
							var ingress []nativePacket
							var egress []nativeWire
							for _, packet := range packets {
								var value map[string]any
								if json.Unmarshal(nativePacketJSON(t, packet), &value) != nil {
									t.Fatal("native packet JSON")
								}
								if nativeClassifier(value) == classifier {
									ingress = append(ingress, packet)
								}
							}
							for _, wire := range wires {
								if nativeClassifier(wire.value) == classifier {
									egress = append(egress, wire)
								}
							}
							if len(ingress) != len(egress) {
								t.Fatal("Guardian producer count changed")
							}
							for i, wire := range egress {
								if route == "api-key" {
									if !bytes.Equal(ingress[i].payload, wire.encodedBody) {
										t.Fatal("API-key Guardian bytes changed")
									}
								} else {
									assertNativeMessageParity(t, ingress[i], wire)
								}
							}
						}

					}
					saveFinalCapture(t, packets, wires)
					t.Logf("native_classifier_requests=2 retained=%t full_requests=true", retained)
				})
			}
		}
	}
}
