//go:build nativeparity

package integration

import (
	"bytes"
	"fmt"
	"testing"
)

func TestInBandCompactionHTTPAndWSContinuation(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, ws := range []bool{false, true} {
			for _, format := range []string{"ordinary", "converted-lite", "native-lite"} {
				t.Run(fmt.Sprintf("subscription=%t/ws=%t/%s", subscription, ws, format), func(t *testing.T) {
					model := "gpt-5.4"
					if format != "ordinary" {
						model = "gpt-5.6-sol"
					}
					capture := newNativeCompactionCapture(t, "in-band", false)
					gateway := newNativeGateway(t, capture.server.URL, subscription)
					client := ordinaryClient(t, gateway, ws, true, nil)
					first := ordinaryCompactionRequest(model, []any{compactionUser("covered source history")}, nil)
					base := first["instructions"].(string)
					if format == "native-lite" {
						first["input"] = append([]any{
							map[string]any{"type": "additional_tools", "role": "developer", "tools": []any{}},
							map[string]any{"type": "message", "role": "developer", "content": []any{map[string]any{"type": "input_text", "text": base}}},
						}, first["input"].([]any)...)
						delete(first, "instructions")
						delete(first, "tools")
					}
					first["context_management"] = []any{map[string]any{"type": "compaction", "compact_threshold": 100}}
					checkpoint := client.send(first)
					if len(checkpoint["output"].([]any)) != 2 {
						t.Fatal("in-band compaction result lost its trailing output")
					}
					httpClient := ordinaryClient(t, gateway, false, false, nil)
					delta := compactionDelta(first, checkpoint["id"], "new inline continuation")
					delete(delta, "instructions") // Prior top-level base remains a per-call setting.
					continued := httpClient.send(delta)
					if ws {
						client.send(compactionDelta(first, continued["id"], "full WS continuation"))
					}
					wires := businessWires(capture.snapshot())
					if len(wires) != 2+boolToInt(ws) {
						t.Fatal("in-band compaction continuation inference count")
					}
					if subscription {
						for index, wire := range wires[1:] {
							if wire.value["previous_response_id"] != nil {
								t.Fatal("in-band HTTP/WS full reconstruction retained a reference")
							}
							types, texts := compactionItemCounts(wire)
							if types["compaction"] != 1 || types["compaction_trigger"] != 0 || texts["covered source history"] != 0 || texts["new inline continuation"] != 1 || texts["synthetic precompaction assistant"] != 1+index {
								t.Fatal("in-band replacement window lost suffixes or revived covered input")
							}
							if format == "native-lite" {
								input := wire.value["input"].([]any)
								if input[0].(map[string]any)["type"] != "additional_tools" || texts[base] != 1 {
									t.Fatal("in-band native Lite lost its caller setup prefix")
								}
							} else if index == 0 {
								assertScenarioBaseValue(t, wire, model == "gpt-5.6-sol", "")
							} else {
								assertScenarioBaseValue(t, wire, model == "gpt-5.6-sol", base)
							}
							for _, raw := range wire.value["input"].([]any) {
								item, _ := raw.(map[string]any)
								if item["type"] == "compaction" && item["encrypted_content"] != "synthetic_inline_state" {
									t.Fatal("in-band compaction ciphertext changed")
								}
							}
						}
					} else {
						assertCompactionPassthroughPackets(t, gateway.tap.packets(t), wires)
					}
				})
			}
		}
	}
}

func assertCompactionPassthroughPackets(t *testing.T, packets []nativePacket, wires []nativeWire) {
	t.Helper()
	if len(packets) != len(wires) {
		t.Fatal("API-key compaction capture count changed")
	}
	// The tap groups TCP connections; application captures use delivery order. Compare each
	// transport's sequential messages so HTTP between two frames on one WS is not mispaired.
	for _, method := range []string{"GET", "POST"} {
		var incoming [][]byte
		for _, packet := range packets {
			if packet.method == method {
				incoming = append(incoming, packet.payload)
			}
		}
		index := 0
		for _, wire := range wires {
			if wire.method != method {
				continue
			}
			if index >= len(incoming) || !bytes.Equal(incoming[index], wire.encodedBody) {
				t.Fatal("API-key compaction packet changed within transport sequence")
			}
			index++
		}
		if index != len(incoming) {
			t.Fatal("API-key compaction transport count changed")
		}
	}
}
