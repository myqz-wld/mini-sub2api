//go:build nativeparity

package integration

import (
	"bytes"
	"fmt"
	"net/http"
	"reflect"
	"testing"
	"time"
)

func TestNativeScenarioTransportSocketBoundary(t *testing.T) {
	for _, boundary := range []string{"handshake-token", "prewarm-token", "completed-tool-disconnect", "retryable-tool-disconnect"} {
		for _, route := range []string{"direct", "api-key", "subscription"} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("%s/%s/%s", boundary, route, model), func(t *testing.T) {
					capture := newNativeTransportCapture(t, boundary == "handshake-token", boundary == "completed-tool-disconnect" || boundary == "retryable-tool-disconnect")
					capture.prewarmToken = boundary == "prewarm-token"
					options := nativeOptions{endpoint: capture.server.URL, model: model, ws: true, base: "synthetic socket base", neutralPersonality: true}
					if boundary == "retryable-tool-disconnect" {
						options.configOverrides = map[string]string{"model_providers.native_capture.stream_max_retries": "1"}
					}
					var gateway nativeGateway
					if route != "direct" {
						gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
						options.endpoint, options.bearer = gateway.server.URL, gateway.secret
					}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					if boundary == "handshake-token" {
						client.turn(thread, "synthetic handshake routing state probe")
						client.turn(thread, "synthetic next turn after handshake")
						assertTransportLifecycle(t, capture.snapshot(), true)
						assertTransportMediation(t, gateway, capture, route)
						t.Log("handshake response token ignored; response.metadata first token retained; next turn cleared")
						return
					}
					if boundary == "prewarm-token" {
						client.turn(thread, "synthetic prewarm token adoption")
						client.turn(thread, "synthetic new turn after prewarm adoption")
						assertTransportPrewarmToken(t, capture.snapshot(), route)
						tokens := []string{"transport-prewarm-token", "transport-prewarm-token", "transport-prewarm-token", ""}
						assertTransportLifecycle(t, capture.snapshot(), true, tokens...)
						if route != "direct" {
							var nativeWires []nativeWire
							for _, packet := range gateway.tap.packets(t) {
								nativeWires = append(nativeWires, nativeWire{value: transportPacketValue(t, packet)})
							}
							assertTransportPrewarmToken(t, nativeWires, "direct")
						}
						assertTransportMediation(t, gateway, capture, route)
						return
					}
					status := transportTurnWithDisconnect(t, client, thread, capture.closed)
					wires := businessWires(capture.snapshot())
					if status != "completed" {
						t.Fatalf("completed-tool disconnect outcome: route=%s turn_status=%s observed_business=%d", route, status, len(wires))
					}
					if len(wires) != 3 || wires[1].connection == wires[0].connection || wires[1].value["previous_response_id"] != nil {
						t.Fatal("same-turn reconnect did not establish a new full baseline")
					}
					metadata := transportMetadata(t, wires[0].value)
					turn, _ := metadata["turn_id"].(string)
					for i, wire := range wires[1:] {
						current := transportMetadata(t, wire.value)
						token := current["x-codex-turn-state"]
						if boundary == "completed-tool-disconnect" {
							if wire.method != http.MethodPost || wire.value["previous_response_id"] != nil {
								t.Fatal("zero-retry native disconnect did not select full HTTP fallback")
							}
							token = wire.headers.Get("X-Codex-Turn-State")
						} else if wire.method != http.MethodGet {
							t.Fatal("retry-enabled native disconnect failed to remain WS")
						}
						if current["turn_id"] != turn || token != "transport-token-1" {
							t.Error("same-turn reconnect lost the turn identity or first routing token")
						}
						assertTransportItems(t, wire.value, turn, i+1, boundary == "retryable-tool-disconnect" && i == 1)
					}
					if boundary == "retryable-tool-disconnect" && (wires[2].connection != wires[1].connection || wires[2].value["previous_response_id"] == nil) {
						t.Error("reconnected socket failed to reuse its own new baseline")
					}
					assertTransportMediation(t, gateway, capture, route)
					t.Logf("same-turn completed-response disconnect: route=%s boundary=%s business=3 retained-token=true", route, boundary)
				})
			}
		}
	}
}

func assertTransportMediation(t *testing.T, gateway nativeGateway, capture *nativeTransportCapture, route string) {
	t.Helper()
	upstream := capture.snapshot()
	for _, wire := range upstream {
		if wire.method == http.MethodGet && wire.headers.Get("X-Codex-Turn-State") != "" {
			t.Error("native reconnect handshake unexpectedly carries a turn-local token")
		}
	}
	if route == "direct" {
		return
	}
	packets := gateway.tap.packets(t)
	// The native writer can race the close notification. A single tool suffix can
	// reach the old downstream socket after the scripted upstream is already closed;
	// it receives failure and native retries on its selected transport. Count the
	// attempt explicitly instead of treating it as an executed upstream inference.
	if capture.closeAfterTool && len(packets) == len(upstream)+1 {
		if len(packets) < 3 || packets[2].method != http.MethodGet || packets[2].connection != packets[0].connection {
			t.Fatal("unexpected extra request outside the closed WS connection")
		}
		failed := transportPacketValue(t, packets[2])
		first := transportPacketValue(t, packets[1])
		turn, _ := transportMetadata(t, first)["turn_id"].(string)
		if failed["previous_response_id"] == nil || transportMetadata(t, failed)["turn_id"] != turn {
			t.Fatal("failed WS attempt does not continue the completed first tool call")
		}
		assertTransportItems(t, failed, turn, 1, true)
		packets = append(append([]nativePacket(nil), packets[:2]...), packets[3:]...)
		t.Log("closed-socket ingress attempt observed: failed_tool_suffix=1 extra_upstream_inferences=0")
	}
	if len(packets) != len(upstream) {
		t.Fatal("gateway replayed or dropped a socket-boundary request")
	}
	for i, packet := range packets {
		if packet.method != upstream[i].method {
			t.Fatal("gateway converted the transport instead of forwarding native's selected transport")
		}
		if route == "api-key" && !bytes.Equal(packet.payload, upstream[i].encodedBody) {
			t.Error("API-key socket-boundary request payload changed")
		}
	}
}

func assertTransportPrewarmToken(t *testing.T, all []nativeWire, route string) {
	t.Helper()
	wires := businessWires(all)
	if len(wires) != 4 || len(all) != 5 || all[0].value["generate"] != false {
		t.Fatal("prewarm token fixture request sequence differs")
	}
	expected := []any{"transport-prewarm-token", "transport-prewarm-token", "transport-prewarm-token", nil}
	var observed []any
	for _, wire := range wires {
		observed = append(observed, transportMetadata(t, wire.value)["x-codex-turn-state"])
	}
	if !reflect.DeepEqual(observed, expected) {
		t.Errorf("prewarm adoption token sequence differs from native lifetime: %s", route)
	}
}

func transportTurnWithDisconnect(t *testing.T, client *nativeClient, thread string, disconnected <-chan struct{}) string {
	t.Helper()
	client.call("turn/start", map[string]any{"threadId": thread, "input": []any{map[string]any{"type": "text", "text": "synthetic same-turn socket disconnect"}}})
	firstTool := true
	for {
		if len(client.pending) > 0 {
			event := client.pending[0]
			client.pending = client.pending[1:]
			params, _ := event["params"].(map[string]any)
			turn, _ := params["turn"].(map[string]any)
			status, _ := turn["status"].(string)
			return status
		}
		event := client.read()
		if firstTool && event["method"] == "item/tool/call" {
			firstTool = false
			select {
			case <-disconnected:
			case <-time.After(5 * time.Second):
				t.Fatal("scripted completed-response disconnect did not occur")
			}
		}
		client.handle(event)
	}
}
