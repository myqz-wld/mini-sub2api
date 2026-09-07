//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"testing"
)

// Synthetic bare Responses calls, with no client session or previous-response locator.
// WS reconnects for every full submission so a bound socket cannot mask a failed prefix lookup.
func TestNativeBareHistoryAssociation(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, delivery := range []string{"http-json", "http-sse", "ws-reconnect"} {
			for _, format := range []string{"ordinary", "converted-lite", "native-lite", "forced-lite"} {
				for _, tool := range []bool{false, true} {
					for _, reduced := range []bool{false, true} {
						t.Run(fmt.Sprintf("subscription=%t/%s/%s/tool=%t/reduced=%t", subscription, delivery, format, tool, reduced), func(t *testing.T) {
							capture := newNativeCapture(t)
							if !tool {
								capture.business = 1 // Select text-only replies before any inference starts.
							}
							gateway := newNativeGateway(t, capture.server.URL, subscription)
							requestFormat := format
							if format == "forced-lite" {
								requestFormat = "native-lite"
							}
							request := ordinaryRequest(requestFormat, "anonymous")
							if format == "forced-lite" {
								request["model"] = "gpt-5.4"
							}
							for step := 0; step < 3; step++ {
								client := ordinaryClient(t, gateway, delivery == "ws-reconnect", delivery != "http-json", nil)
								response := client.send(request)
								if client.ws != nil {
									_ = client.ws.CloseNow()
								}
								output, ok := response["output"].([]any)
								if !ok || len(output) != 1 {
									t.Fatal("bare history response output missing")
								}
								item, _ := output[0].(map[string]any)
								if reduced {
									item = reducedBareHistoryItem(t, item)
								}
								input := append(request["input"].([]any), item)
								if step == 0 && tool {
									if item["type"] != "function_call" || item["call_id"] == nil {
										t.Fatal("bare tool call missing its reference")
									}
									input = append(input, map[string]any{"type": "function_call_output", "call_id": item["call_id"], "output": "synthetic bare result"})
								} else {
									if item["type"] != "message" {
										t.Fatal("bare text response missing")
									}
									input = append(input, map[string]any{"role": "user", "content": "next synthetic bare user"})
								}
								request["input"] = input
							}
							packets := gateway.tap.packets(t)
							all := capture.snapshot()
							wires := businessWires(all)
							if len(packets) != 3 || len(wires) != 3 {
								t.Fatal("bare full-history request count changed")
							}
							for i, packet := range packets {
								caller := decodeNativePacket(t, packet)
								if caller["client_metadata"] != nil || caller["previous_response_id"] != nil ||
									packet.headers.Get("Session-Id") != "" || packet.headers.Get("Originator") != "" {
									t.Fatal("bare history fixture supplied an explicit locator")
								}
								if !subscription && !bytes.Equal(packet.payload, wires[i].encodedBody) {
									t.Fatal("API-key bare history payload changed")
								}
							}
							if subscription {
								assertBareHistoryIdentity(t, wires, tool, delivery == "ws-reconnect")
								for _, wire := range all {
									items, _ := wire.value["input"].([]any)
									if len(items) > 0 && items[0].(map[string]any)["type"] == "additional_tools" {
										if format == "converted-lite" {
											assertNativeLiteIDs(t, wire)
										} else {
											assertBareFormedLitePrefix(t, wire)
										}
									}
								}
							}
						})
					}
				}
			}
		}
	}
}

func assertBareFormedLitePrefix(t *testing.T, wire nativeWire) {
	t.Helper()
	var body struct {
		Input []struct {
			ID    string          `json:"id"`
			Tools json.RawMessage `json:"tools"`
		} `json:"input"`
	}
	if json.Unmarshal(wire.body, &body) != nil || len(body.Input) < 2 {
		t.Fatal("bare formed Lite prefix missing")
	}
	metadata, _ := wire.value["client_metadata"].(map[string]any)
	thread, _ := metadata["thread_id"].(string)
	oid, _ := hex.DecodeString("6ba7b8129dad11d180b400c04fd430c8")
	if thread == "" || body.Input[0].ID != "at_"+uuidText(uuid5(uuid5(oid, []byte(thread)), body.Input[0].Tools)) {
		t.Fatal("bare formed Lite tools ID differs from its exact thread/schema bytes")
	}
	// This fixture's existing developer instruction has no claimed native-base ID/provenance.
	// Only a Core-generated base or proven native prefix is eligible for a regenerated base ID.
	if body.Input[1].ID != "" {
		t.Fatal("bare formed Lite instruction received an invented native-base ID")
	}
}

func reducedBareHistoryItem(t *testing.T, item map[string]any) map[string]any {
	t.Helper()
	item = cloneOrdinaryContext(t, item)
	delete(item, "id")
	delete(item, "status")
	content, _ := item["content"].([]any)
	for _, raw := range content {
		part, _ := raw.(map[string]any)
		for _, field := range []string{"annotations", "logprobs"} {
			if value, ok := part[field].([]any); ok && len(value) == 0 {
				delete(part, field)
			}
		}
	}
	return item
}

func assertBareHistoryIdentity(t *testing.T, wires []nativeWire, tool, reconnect bool) {
	t.Helper()
	first, _ := wires[0].value["client_metadata"].(map[string]any)
	previousTurn := first["turn_id"]
	for i, wire := range wires {
		metadata, _ := wire.value["client_metadata"].(map[string]any)
		for _, field := range []string{"session_id", "thread_id"} {
			value, ok := metadata[field].(string)
			if !ok || value == "" || value != first[field] {
				t.Fatalf("bare full-history continuation changed %s", field)
			}
		}
		turn, ok := metadata["turn_id"].(string)
		if !ok || turn == "" || wire.headers.Get("Session-Id") != metadata["session_id"] {
			t.Fatal("bare history identity carriers disagree")
		}
		if i > 0 {
			if (metadata["turn_id"] == previousTurn) != (tool && i == 1) {
				t.Fatal("bare history tool/user turn transition changed")
			}
			if reconnect && wire.connection == wires[i-1].connection {
				t.Fatal("bare WS history test did not replace its upstream connection")
			}
		}
		previousTurn = metadata["turn_id"]
	}
}
