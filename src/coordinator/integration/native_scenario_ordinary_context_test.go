//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"path/filepath"
	"reflect"
	"testing"
)

// The native client resolves this controlled context once. Ordinary callers then submit the same
// content without native markers or identity metadata; these replays are not native-generated calls.
func TestNativeScenarioOrdinaryResolvedContext(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
		t.Run(model, func(t *testing.T) {
			root, cwd := environmentProject(t)
			direct := newNativeCapture(t)
			options := environmentOptions(model, cwd)
			options.endpoint = direct.server.URL
			client := startNativeClient(t, options)
			thread := client.thread(options)
			skill := environmentWriteSkill(t, root, "ordinary-context-probe", envSkillDescription, envSkillBody)
			client.call("skills/extraRoots/set", map[string]any{"extraRoots": []any{filepath.Dir(filepath.Dir(skill))}})
			skill = environmentResolvedSkill(t, client, cwd, "ordinary-context-probe")
			client.turnWith(thread, map[string]any{
				"input":             []any{map[string]any{"type": "text", "text": "synthetic resolved context"}, map[string]any{"type": "skill", "name": "ordinary-context-probe", "path": skill}},
				"collaborationMode": environmentMode(model, "plan", "ORDINARY_CONTEXT_MODE"),
			})
			seed := ordinaryResolvedContext(t, direct.snapshot()[0])
			for _, subscription := range []bool{false, true} {
				for _, delivery := range []string{"json", "sse", "ws"} {
					t.Run(fmt.Sprintf("subscription=%t/%s", subscription, delivery), func(t *testing.T) {
						upstream := newNativeCapture(t)
						gateway := newNativeGateway(t, upstream.server.URL, subscription)
						ordinary := ordinaryClient(t, gateway, delivery == "ws", delivery != "json", nil)
						first := cloneOrdinaryContext(t, seed)
						response := ordinary.send(first)
						call := response["output"].([]any)[0].(map[string]any)
						second := cloneOrdinaryContext(t, seed)
						second["previous_response_id"] = response["id"]
						second["input"] = []any{map[string]any{"type": "function_call_output", "call_id": call["call_id"], "output": "synthetic ordinary context tool result"}}
						ordinary.send(second)
						wires := businessWires(upstream.snapshot())
						if len(wires) != 2 {
							t.Fatal("ordinary resolved-context inference count changed")
						}
						if !subscription {
							packets := gateway.tap.packets(t)
							if len(packets) != len(wires) {
								t.Fatal("API-key resolved-context request count changed")
							}
							for index, wire := range wires {
								if !bytes.Equal(packets[index].payload, wire.encodedBody) {
									t.Fatal("API-key resolved context payload changed")
								}
							}
							assertProfileStateFileCount(t, gateway.stateDir, 0)
							return
						}
						all := upstream.snapshot()
						for _, wire := range all {
							input, _ := wire.value["input"].([]any)
							if len(input) > 0 && input[0].(map[string]any)["type"] == "additional_tools" {
								assertNativeLiteIDs(t, wire)
							}
						}
						assertOrdinaryResolvedContent(t, seed, ordinaryResolvedFirst(t, all), false)
						if delivery == "ws" {
							if wires[1].value["previous_response_id"] == nil || len(wires[1].value["input"].([]any)) != 1 {
								t.Fatal("ordinary resolved-context WS continuation did not send one appended tool result")
							}
						} else {
							if wires[1].value["previous_response_id"] != nil {
								t.Fatal("ordinary resolved-context HTTP continuation was not reconstructed")
							}
							assertOrdinaryResolvedContent(t, seed, wires[1], true)
						}
					})
				}
			}
		})
	}
}

func cloneOrdinaryContext(t *testing.T, value map[string]any) map[string]any {
	t.Helper()
	var copy map[string]any
	if json.Unmarshal(mustRequestJSON(t, value), &copy) != nil {
		t.Fatal("clone resolved context fixture")
	}
	return copy
}

func ordinaryResolvedContext(t *testing.T, wire nativeWire) map[string]any {
	t.Helper()
	value := cloneOrdinaryContext(t, wire.value)
	input := value["input"].([]any)
	if input[0].(map[string]any)["type"] == "additional_tools" {
		value["tools"] = input[0].(map[string]any)["tools"]
		value["instructions"] = capturedBase(t, wire)
		input = input[2:]
	}
	for _, raw := range input {
		item := raw.(map[string]any)
		delete(item, "id")
		delete(item, "internal_chat_message_metadata_passthrough")
	}
	value["input"] = input
	delete(value, "client_metadata")
	delete(value, "prompt_cache_key")
	for _, marker := range []string{envRoot, envLeaf, envSkillBody} {
		environmentAssertCount(t, value, "user", marker, 1)
	}
	for _, marker := range []string{envDeveloper, envSkillDescription, "<permissions instructions>", "<collaboration_mode>"} {
		environmentAssertCount(t, value, "developer", marker, 1)
	}
	return value
}

func assertOrdinaryResolvedContent(t *testing.T, expected map[string]any, wire nativeWire, continued bool) {
	t.Helper()
	input := wire.value["input"].([]any)
	if wire.value["model"] == "gpt-5.6-sol" {
		if !reflect.DeepEqual(input[0].(map[string]any)["tools"], expected["tools"]) {
			t.Fatal("ordinary resolved-context tools changed during Lite conversion")
		}
		input = input[2:]
	} else if !reflect.DeepEqual(wire.value["tools"], expected["tools"]) {
		t.Fatal("ordinary resolved-context tools changed")
	}
	if capturedBase(t, wire) != expected["instructions"] {
		t.Fatal("ordinary resolved-context base changed")
	}
	original := expected["input"].([]any)
	want := len(original)
	if continued {
		want += 2 // One output function call, once, and its appended result.
	}
	if len(input) != want {
		t.Fatalf("ordinary resolved context length=%d want=%d", len(input), want)
	}
	for i, raw := range original {
		item := input[i].(map[string]any)
		for field, value := range raw.(map[string]any) {
			if !reflect.DeepEqual(value, item[field]) {
				t.Fatalf("ordinary resolved context item %d field %s changed", i, field)
			}
		}
	}
	if continued && (input[want-2].(map[string]any)["type"] != "function_call" || input[want-1].(map[string]any)["type"] != "function_call_output") {
		t.Fatal("ordinary resolved-context continuation order changed")
	}
}

func ordinaryResolvedFirst(t *testing.T, wires []nativeWire) nativeWire {
	t.Helper()
	for index, wire := range wires {
		if wire.value["generate"] == false {
			continue
		}
		if wire.value["previous_response_id"] == nil {
			return wire
		}
		if index != 1 || wires[0].value["generate"] != false || wire.connection != wires[0].connection || wire.value["previous_response_id"] != "resp_native_1" {
			t.Fatal("ordinary first increment lacks a completed setup on the same socket")
		}
		// Prefix IDs were validated on raw transmitted setup bytes above. Reconstruct only
		// the effective ordered context for comparison; this object is never sent or saved.
		value := cloneOrdinaryContext(t, wire.value)
		prefix := append([]any(nil), wires[0].value["input"].([]any)...)
		value["input"] = append(prefix, wire.value["input"].([]any)...)
		wire.value = value
		return wire
	}
	t.Fatal("ordinary first business request missing")
	return nativeWire{}
}
