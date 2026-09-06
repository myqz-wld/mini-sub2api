//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"testing"
)

func TestNativeScenarioIdentityChildNextTurn(t *testing.T) {
	for _, route := range []string{"direct", "api_key", "subscription"} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("%s/ws=%t/%s", route, ws, model), func(t *testing.T) {
					capture := newNativeIdentityCapture(t, "none")
					capture.followup = true
					options := nativeOptions{endpoint: capture.server.URL, model: model, ws: ws, configOverrides: map[string]string{"features.multi_agent_v2": "true"}}
					var gateway nativeGateway
					if route != "direct" {
						gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
						options.endpoint, options.bearer = gateway.server.URL, gateway.secret
					}
					client := startNativeClient(t, options)
					root := client.thread(options)
					status, _ := identityNativeTurn(client, root)
					if status != "completed" {
						t.Fatal("native parent seed failed")
					}
					native := capture.snapshot()
					if route != "direct" {
						native = identityPackets(t, gateway.tap.packets(t))
					}
					child := identityNativeGraph(t, native)
					read := client.call("thread/read", map[string]any{"threadId": child, "includeTurns": false})
					view, _ := read["thread"].(map[string]any)
					if view["ephemeral"] != true {
						t.Fatal("child was not ephemeral before rejoin")
					}
					before := len(businessWires(capture.snapshot()))
					// The parent-owned V2 child receives a real native followup_task. Direct
					// app-server resume requires stored child graph metadata even when live.
					status, _ = identityNativeTurn(client, root)
					after := len(businessWires(capture.snapshot()))

					if status != "completed" || after != before+4 {
						t.Fatal("native child follow-up did not reach both branches")
					}
					native = capture.snapshot()
					if route != "direct" {
						native = identityPackets(t, gateway.tap.packets(t))
					}
					groups := identityWireGroups(t, native)
					children := businessWires(groups["child"])
					roots := businessWires(groups["root"])
					if len(children) != 2 || len(roots) != 6 {
						t.Fatal("native followup_task failed to sample each intended branch")
					}
					first, _ := identityMetadata(t, children[0])
					second, nested := identityMetadata(t, children[1])
					parent, _ := identityMetadata(t, roots[3])
					if first["thread_id"] != second["thread_id"] || first["turn_id"] == second["turn_id"] || second["parent_turn_id"] != parent["turn_id"] || nested["root_turn_id"] != parent["turn_id"] {
						t.Fatal("native child follow-up causal graph differs")
					}
					oldItems := 0
					input, _ := children[1].value["input"].([]any)
					for _, raw := range input {
						item, _ := raw.(map[string]any)
						metadata, _ := item["internal_chat_message_metadata_passthrough"].(map[string]any)
						if metadata["turn_id"] == first["turn_id"] {
							oldItems++
						}
					}
					if ws && children[1].value["previous_response_id"] == nil || !ws && (children[1].value["previous_response_id"] != nil || oldItems == 0) {
						t.Fatal("native child full versus incremental history contract differs")
					}
					emittedChildren := len(businessWires(identityWireGroups(t, capture.snapshot())["child"]))
					if emittedChildren != 2 {
						t.Fatal("gateway dropped native child follow-up inference")
					}
					if route != "direct" {
						identityCompareGateway(t, native, capture.snapshot(), route == "subscription")
					}
					t.Logf("actual_native_child_next_turn parent_status=%s upstream_inference_delta=%d old_child_turn_items=%d", status, after-before, oldItems)
				})
			}
		}
	}
}

func TestNativeScenarioIdentityEphemeralForkUnavailable(t *testing.T) {
	for _, fork := range []string{"all", "1"} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("fork=%s/ws=%t/%s", fork, ws, model), func(t *testing.T) {
					capture := newNativeIdentityCapture(t, fork)
					options := nativeOptions{endpoint: capture.server.URL, model: model, ws: ws, configOverrides: map[string]string{"features.multi_agent_v2": "true"}}
					client := startNativeClient(t, options)
					root := client.thread(options)
					status, _ := identityNativeTurn(client, root)
					groups := identityWireGroups(t, capture.snapshot())
					if status != "completed" || len(businessWires(groups["root"])) != 2 || len(groups["child"]) != 0 {
						t.Fatal("ephemeral fork limitation changed; reassess actual history capture availability")
					}
					missingHistory := false
					for _, wire := range groups["root"] {
						input, _ := wire.value["input"].([]any)
						for _, raw := range input {
							item, _ := raw.(map[string]any)
							if item["type"] != "function_call_output" {
								continue
							}
							encoded, _ := json.Marshal(item)
							missingHistory = missingHistory || bytes.Contains(encoded, []byte("collab spawn failed: no thread with id: "+root))
						}
					}
					if !missingHistory {
						t.Fatal("native fork did not report the source-backed missing-history reason")
					}
					t.Log("actual native spawn fork attempt: parent completed after tool error; child request unavailable without persisted source history")
				})
			}
		}
	}
}

func TestNativeScenarioIdentityCopiedHistory(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				for _, history := range []string{"none", "parent", "prior_child"} {
					t.Run(fmt.Sprintf("subscription=%t/ws=%t/%s/%s", subscription, ws, model, history), func(t *testing.T) {
						capture := newNativeIdentityCapture(t, "disabled")
						gateway := newNativeGateway(t, capture.server.URL, subscription)
						status, _ := identitySend(t, gateway, ws, nil, identityRequest(model, identityRoot, identityRoot, identityRootTurn, "", "", ""))
						if status != 200 {
							t.Fatal("historical identity root seed failed")
						}
						historical := ""
						if history == "parent" {
							historical = identityRootTurn
						}
						if history == "prior_child" {
							status, _ = identitySend(t, gateway, ws, nil, identityRequest(model, identityRoot, identityChild, identityChildTurn, identityRoot, "", ""))
							if status != 200 {
								t.Fatal("historical identity child seed failed")
							}
							historical = identityChildTurn
						}
						before := len(businessWires(capture.snapshot()))
						status, terminal := identitySend(t, gateway, ws, nil, identityRequest(model, identityRoot, identityChild, identityForkTurn, identityRoot, "", historical))
						reached := len(businessWires(capture.snapshot())) > before
						if status != 200 || !reached {
							t.Fatal("gateway rejected eligible historical turn attribution")
						}
						t.Logf("source-backed copied history: status=%d terminal=%s inference_reached=%t", status, terminal, reached)
					})
				}
			}
		}
	}
}
