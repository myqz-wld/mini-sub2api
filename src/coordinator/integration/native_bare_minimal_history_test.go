//go:build nativeparity

package integration

import (
	"bytes"
	"fmt"
	"testing"
)

// Start with only model/input (plus transport controls): no instructions, tools, IDs or metadata.
func TestNativeBareMinimalHistory(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, delivery := range []string{"http-json", "http-sse", "ws-reconnect"} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				for _, reduced := range []bool{false, true} {
					t.Run(fmt.Sprintf("subscription=%t/%s/%s/reduced=%t", subscription, delivery, model, reduced), func(t *testing.T) {
						capture := newNativeCapture(t)
						capture.business = 1 // The responder emits only assistant text for this fixture.
						gateway := newNativeGateway(t, capture.server.URL, subscription)
						request := map[string]any{"model": model, "input": "first minimal user"}
						history := []any{map[string]any{"role": "user", "content": "first minimal user"}}
						for step := 0; step < 3; step++ {
							client := ordinaryClient(t, gateway, delivery == "ws-reconnect", delivery != "http-json", nil)
							result := client.send(request)
							if client.ws != nil {
								_ = client.ws.CloseNow()
							}
							output, _ := result["output"].([]any)
							if len(output) != 1 {
								t.Fatal("minimal bare response output missing")
							}
							item, _ := output[0].(map[string]any)
							if item["type"] != "message" {
								t.Fatal("minimal bare responder selected an undeclared tool")
							}
							if reduced {
								item = reducedBareHistoryItem(t, item)
							}
							history = append(history, item, map[string]any{"role": "user", "content": "next minimal user"})
							request["input"] = history
						}
						packets, wires := gateway.tap.packets(t), businessWires(capture.snapshot())
						if len(packets) != 3 || len(wires) != 3 {
							t.Fatal("minimal bare business request count changed")
						}
						for i, packet := range packets {
							caller := decodeNativePacket(t, packet)
							for _, field := range []string{"client_metadata", "previous_response_id", "instructions", "tools", "prompt_cache_key", "conversation"} {
								if caller[field] != nil {
									t.Fatalf("minimal bare fixture unexpectedly supplied %s", field)
								}
							}
							if packet.headers.Get("Session-Id") != "" || packet.headers.Get("Originator") != "" {
								t.Fatal("minimal bare fixture supplied a session or caller marker")
							}
							if !subscription && !bytes.Equal(packet.payload, wires[i].encodedBody) {
								t.Fatal("minimal API-key payload changed")
							}
						}
						if subscription {
							assertBareHistoryIdentity(t, wires, false, delivery == "ws-reconnect")
						}
					})
				}
			}
		}
	}
}
