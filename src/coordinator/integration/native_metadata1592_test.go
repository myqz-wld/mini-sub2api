//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"reflect"
	"testing"
)

// The built-in provider opts in even at an overridden loopback destination
// (model-provider-info/src/lib.rs, model-provider/src/provider.rs). A fresh
// thread has {"status":"none"}; protocol/src/mcp.rs omits empty sources.
// Dynamic tools cannot return Apps result metadata, so this measures provider
// admission and ordinary output preservation, not populated Apps MCP provenance.
func TestNative1592ProviderMetadataOptIn(t *testing.T) {
	for _, builtin := range []bool{false, true} {
		for _, ws := range []bool{false, true} {
			for _, subscription := range []bool{false, true} {
				t.Run(fmt.Sprintf("builtin=%t/ws=%t/subscription=%t", builtin, ws, subscription), func(t *testing.T) {
					capture := newNativeCapture(t)
					// The built-in provider supports WS independently of custom-provider
					// configuration. Reject its handshake to exercise native HTTP fallback.
					gateway := newNativeGatewayForWireShape(t, capture.server.URL, subscription, !ws)
					calls := 0
					options := nativeOptions{
						endpoint: gateway.server.URL, metadataEndpoint: capture.server.URL,
						bearer: gateway.secret, model: "gpt-5.5", ws: ws, builtinOpenAI: builtin,
						// Keep the OpenAI family on both routes so this varies the internal
						// metadata opt-in without also disabling ordinary item metadata.
						providerName: "OpenAI", base: "Synthetic provider metadata probe.",
						observe: func(event map[string]any) {
							if event["method"] == "item/tool/call" {
								calls++
							}
						},
					}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					client.turn(thread, "Run the synthetic dynamic tool.")
					client.turn(thread, "Continue the synthetic metadata probe.")
					packets, wires := gateway.tap.packets(t), capture.snapshot()
					if calls != 1 || len(businessWires(wires)) != 3 || len(packets) != len(wires) {
						t.Fatal("native metadata probe did not complete the expected tool loop and follow-up")
					}
					method := http.MethodPost
					if ws {
						method = http.MethodGet
					}
					outputs := 0
					for index, wire := range wires {
						var caller map[string]any
						if json.Unmarshal(nativePacketJSON(t, packets[index]), &caller) != nil {
							t.Fatal("native metadata capture JSON invalid")
						}
						if wire.method != method || packets[index].method != method {
							t.Fatal("provider metadata probe used an unexpected transport")
						}
						if caller["generate"] != false {
							assertNative1592EmptyAttribution(t, caller, builtin)
							assertNative1592EmptyAttribution(t, wire.value, builtin)
						}
						before := native1592DynamicOutputs(t, caller)
						after := native1592DynamicOutputs(t, wire.value)
						if !reflect.DeepEqual(before, after) {
							t.Fatal("gateway changed an ordinary dynamic tool output")
						}
						outputs += len(before)
						if subscription {
							assertNativeMessageParity(t, packets[index], wire)
						} else if !bytes.Equal(packets[index].payload, wire.encodedBody) {
							t.Fatal("API-key provider metadata request bytes changed")
						}
					}
					if outputs == 0 {
						t.Fatal("native dynamic tool output was never captured")
					}
					t.Logf("provider metadata opt-in: builtin=%t ws=%t subscription=%t business_requests=3 callbacks=1", builtin, ws, subscription)
				})
			}
		}
	}
}

func assertNative1592EmptyAttribution(t *testing.T, value map[string]any, included bool) {
	t.Helper()
	metadata, _ := value["client_metadata"].(map[string]any)
	raw, present := metadata["mcp_attribution"]
	if !included {
		if present {
			t.Fatal("custom loopback provider received internal MCP attribution without opt-in")
		}
		return
	}
	if !present || raw != `{"status":"none"}` {
		t.Fatal("built-in loopback override lost exact native empty MCP attribution")
	}
}

func native1592DynamicOutputs(t *testing.T, value map[string]any) []any {
	t.Helper()
	var outputs []any
	input, _ := value["input"].([]any)
	for _, raw := range input {
		item, _ := raw.(map[string]any)
		if item["type"] != "function_call_output" {
			continue
		}
		encoded, err := json.Marshal(item["output"])
		if err != nil || !bytes.Contains(encoded, []byte("synthetic tool result")) {
			t.Fatal("dynamic tool continuation lost its real callback result")
		}
		outputs = append(outputs, item["output"])
	}
	return outputs
}
