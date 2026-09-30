//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"testing"
)

// Exercise the actual CLI with the advertised capability explicitly enabled. The
// shipped static catalog leaves this opt-in feature off; all endpoints remain local.
func TestNative1580ReasoningUpdates(t *testing.T) {
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
				callerWires := wires
				if route == "subscription" {
					packets := gateway.tap.packets(t)
					if len(packets) != len(wires) {
						t.Fatal("native effort update request count changed")
					}
					callerWires = make([]nativeWire, len(packets))
					for i, packet := range packets {
						raw := nativePacketJSON(t, packet)
						if json.Unmarshal(raw, &callerWires[i].value) != nil {
							t.Fatal("native effort input JSON")
						}
						// The gateway pins the native default (feature off). Remove only that
						// experimental control from the expectation, preserving every other byte.
						packet.payload = withoutExperimentalUpdates(t, raw)
						packet.headers = packet.headers.Clone()
						packet.headers.Del("Content-Encoding")
						assertNativeMessageParity(t, packet, wires[i])
					}
				}
				foundHigh := false
				for _, wire := range callerWires {
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
				t.Logf("actual v0.159.2 captured %d requests; native_high_update=true gateway_feature_off=%t", len(wires), route == "subscription")
			})
		}
	}
}

// Preserve JSON order, scalar spelling and all other request fields in this one policy test.
func withoutExperimentalUpdates(t *testing.T, raw []byte) []byte {
	t.Helper()
	decoder := json.NewDecoder(bytes.NewReader(raw))
	if token, err := decoder.Token(); err != nil || token != json.Delim('{') {
		t.Fatal("native effort request object")
	}
	for decoder.More() {
		key, err := decoder.Token()
		if err != nil {
			t.Fatal("native effort request key")
		}
		var value json.RawMessage
		if decoder.Decode(&value) != nil {
			t.Fatal("native effort request value")
		}
		if key != "input" {
			continue
		}
		var items []json.RawMessage
		if json.Unmarshal(value, &items) != nil {
			t.Fatal("native effort input array")
		}
		var kept [][]byte
		for _, item := range items {
			var kind struct {
				Type string `json:"type"`
			}
			if json.Unmarshal(item, &kind) != nil {
				t.Fatal("native effort input type")
			}
			if kind.Type != "configuration_update" {
				kept = append(kept, item)
			}
		}
		end := int(decoder.InputOffset())
		result := append([]byte(nil), raw[:end-len(value)]...)
		result = append(result, '[')
		result = append(result, bytes.Join(kept, []byte(","))...)
		result = append(result, ']')
		return append(result, raw[end:]...)
	}
	t.Fatal("native effort request input missing")
	return nil
}
