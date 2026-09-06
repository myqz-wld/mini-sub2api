//go:build nativeparity

package integration

import (
	"encoding/json"
	"fmt"
	"testing"
)

// Return only status and typed thread metadata. Control bodies never enter logs.
func identityNativeTurn(client *nativeClient, thread string) (string, map[string]bool) {
	client.t.Helper()
	client.call("turn/start", map[string]any{"threadId": thread, "input": []any{map[string]any{"type": "text", "text": "synthetic identity parent task"}}})
	ephemeral := map[string]bool{}
	for {
		var event map[string]any
		if len(client.pending) > 0 {
			event, client.pending = client.pending[0], client.pending[1:]
		} else {
			event = client.read()
		}
		params, _ := event["params"].(map[string]any)
		if event["method"] == "thread/started" {
			if data, ok := params["thread"].(map[string]any); ok {
				id, _ := data["id"].(string)
				ephemeral[id] = data["ephemeral"] == true
			}
		}
		if event["method"] == "turn/completed" {
			if params["threadId"] != thread {
				continue
			}
			turn, _ := params["turn"].(map[string]any)
			status, _ := turn["status"].(string)
			return status, ephemeral
		}
		if event["method"] == "item/tool/call" {
			client.t.Fatal("identity scenario unexpectedly delegated a dynamic tool")
		}
	}
}

func identityMetadata(t *testing.T, wire nativeWire) (map[string]any, map[string]any) {
	t.Helper()
	flat, ok := wire.value["client_metadata"].(map[string]any)
	if !ok {
		t.Fatal("identity metadata absent")
	}
	raw, ok := flat["x-codex-turn-metadata"].(string)
	var nested map[string]any
	if !ok || json.Unmarshal([]byte(raw), &nested) != nil {
		t.Fatal("identity nested metadata invalid")
	}
	return flat, nested
}

func TestNativeScenarioIdentityFreshChild(t *testing.T) {
	for _, route := range []string{"direct", "api_key", "subscription"} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("%s/ws=%t/%s", route, ws, model), func(t *testing.T) {
					capture := newNativeIdentityCapture(t, "none")
					options := nativeOptions{endpoint: capture.server.URL, model: model, ws: ws, configOverrides: map[string]string{"features.multi_agent_v2": "true"}}
					var gateway nativeGateway
					if route != "direct" {
						gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
						options.endpoint, options.bearer = gateway.server.URL, gateway.secret
					}
					client := startNativeClient(t, options)
					root := client.thread(options)
					status, ephemeral := identityNativeTurn(client, root)
					wires := capture.snapshot()
					native := wires
					if route != "direct" {
						native = identityPackets(t, gateway.tap.packets(t))
					}
					child := identityNativeGraph(t, native)
					metadata := client.call("thread/read", map[string]any{"threadId": child, "includeTurns": false})
					childView, _ := metadata["thread"].(map[string]any)
					if childView["ephemeral"] != true {
						t.Fatal("native child metadata confirms non-ephemeral mode")
					}
					if route != "direct" {
						identityCompareGateway(t, native, wires, route == "subscription")
					}
					for _, value := range ephemeral {
						if !value {
							t.Fatal("native spawned thread was not ephemeral")
						}
					}
					t.Logf("native_parent_status=%s business_requests=%d child_ephemeral_verified=true", status, len(businessWires(wires)))
					if status != "completed" {
						t.Fatal("native parent lifecycle failed")
					}
				})
			}
		}
	}
}
