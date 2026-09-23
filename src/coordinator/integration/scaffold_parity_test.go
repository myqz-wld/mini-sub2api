//go:build nativeparity && scaffoldparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"reflect"
	"slices"
	"strings"
	"testing"
)

func TestOpenCodeResponsesCapture(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
		for _, route := range []string{"direct", "api-key", "subscription"} {
			t.Run(fmt.Sprintf("%s/%s", model, route), func(t *testing.T) {
				capture := newScaffoldCapture(t)
				endpoint, secret := capture.server.URL, "synthetic-opencode-key"
				var gateway nativeGateway
				if route != "direct" {
					gateway = newNativeGateway(t, endpoint, route == "subscription")
					endpoint, secret = gateway.server.URL, gateway.secret
				}
				client := startOpenCode(t, endpoint, secret, model)
				created := client.call(t, "/session", map[string]any{"title": "Synthetic capture session"})
				session, ok := created["id"].(string)
				if !ok || session == "" {
					t.Fatal("OpenCode session creation failed")
				}
				for _, prompt := range []string{"Reply briefly to this synthetic first turn.", "Reply briefly to this synthetic follow-up."} {
					result := client.turn(t, session, prompt)
					info, _ := result["info"].(map[string]any)
					if info["error"] != nil {
						t.Fatal("OpenCode reported an inference error")
					}
					parts, _ := result["parts"].([]any)
					found := false
					for _, raw := range parts {
						part, _ := raw.(map[string]any)
						if part["type"] == "text" && part["text"] == "synthetic answer" {
							found = true
						}
					}
					if !found {
						t.Fatal("OpenCode did not consume complete Responses text")
					}
				}
				wires := capture.snapshot()
				if len(wires) < 2 {
					t.Fatal("OpenCode turn requests missing")
				}
				for _, wire := range wires {
					if wire.method != "POST" || wire.value["model"] != model {
						t.Fatal("OpenCode selected unexpected model or transport")
					}
				}
				if route == "direct" {
					saveFinalCapture(t, capture.tap.packets(t), wires)
					return
				}
				packets := gateway.tap.packets(t)
				if len(packets) != len(wires) {
					t.Fatal("gateway added or lost an OpenCode request")
				}
				for i, wire := range wires {
					if route == "api-key" {
						if !bytes.Equal(packets[i].payload, wire.encodedBody) {
							t.Fatal("OpenCode API-key payload changed")
						}
						continue
					}
					assertScaffoldCustomSession(t, packets[i], session)
					assertScaffoldMessageParity(t, packets[i], wire)
					if wire.value["previous_response_id"] != nil {
						t.Fatal("OpenCode Subscription HTTP sent an incremental reference")
					}
				}
				if route == "subscription" {
					assertScaffoldContinuation(t, wires, false)
				}
				saveFinalCapture(t, packets, wires)
			})
		}
	}
}

func assertScaffoldCustomSession(t *testing.T, packet nativePacket, session string) {
	t.Helper()
	if packet.headers.Get("Session-Id") != "" || packet.headers.Get("Originator") != "" ||
		packet.headers.Get("X-Session-Id") != session || packet.headers.Get("X-Session-Affinity") != session {
		t.Fatal("custom-provider fixture no longer exercises anonymous history association")
	}
	caller := decodeNativePacket(t, packet)
	metadata, _ := caller["client_metadata"].(map[string]any)
	if caller["previous_response_id"] != nil || metadata["session_id"] != nil {
		t.Fatal("custom-provider fixture supplied an explicit gateway context locator")
	}
}

// A successful full-context request alone does not prove that the gateway associated its history.
func assertScaffoldContinuation(t *testing.T, wires []nativeWire, toolFollowup bool) {
	t.Helper()
	if len(wires) != 2 {
		t.Fatal("scaffold continuation requires exactly two captured requests")
	}
	first, _ := wires[0].value["client_metadata"].(map[string]any)
	second, _ := wires[1].value["client_metadata"].(map[string]any)
	for _, field := range []string{"session_id", "thread_id"} {
		value, ok := first[field].(string)
		if !ok || value == "" || value != second[field] {
			t.Fatalf("scaffold continuation changed %s", field)
		}
	}
	firstTurn, _ := first["turn_id"].(string)
	secondTurn, _ := second["turn_id"].(string)
	if firstTurn == "" || secondTurn == "" || (firstTurn == secondTurn) != toolFollowup {
		t.Fatal("scaffold continuation did not preserve tool turns or advance user turns")
	}
}

// A third-party Responses producer need not send native item wrappers or a native base prefix.
// Compare its business fields using the declared ordinary conversion rules, without erasing text.
func assertScaffoldMessageParity(t *testing.T, packet nativePacket, wire nativeWire) {
	t.Helper()
	if wire.headers.Get("Originator") != "codex-tui" || wire.headers.Get("Version") != "0.156.0" {
		t.Fatal("scaffold Subscription upstream client identity changed")
	}
	metadata, _ := wire.value["client_metadata"].(map[string]any)
	var turn map[string]any
	if raw, ok := metadata["x-codex-turn-metadata"].(string); !ok || json.Unmarshal([]byte(raw), &turn) != nil {
		t.Fatal("scaffold turn metadata missing")
	}
	if turn["installation_id"] != metadata["x-codex-installation-id"] || turn["session_id"] != metadata["session_id"] {
		t.Fatal("scaffold identity carriers disagree")
	}
	if turn["node_repl_auto_review_required"] != (wire.value["model"] == "gpt-6-astra") {
		t.Fatal("scaffold model policy default changed")
	}
	var caller map[string]any
	if json.Unmarshal(packet.payload, &caller) != nil {
		t.Fatal("scaffold caller JSON invalid")
	}
	before, ok := caller["input"].([]any)
	if !ok {
		t.Fatal("scaffold input array absent")
	}
	base, _ := caller["instructions"].(string)
	if strings.TrimSpace(base) == "" {
		base = ""
	}
	after, _ := wire.value["input"].([]any)
	tools, _ := wire.value["tools"].([]any)
	lite := len(after) > 0 && after[0].(map[string]any)["type"] == "additional_tools"
	if lite != (caller["model"] == "gpt-6-astra") {
		t.Fatal("scaffold selected an unexpected upstream format")
	}
	if lite {
		tools, _ = after[0].(map[string]any)["tools"].([]any)
		prefixLength := 1
		if base != "" {
			prefixLength++
		}
		if len(after) < prefixLength {
			t.Fatal("scaffold Lite setup missing")
		}
		assertScenarioBaseValue(t, wire, true, base)
		after = after[prefixLength:]
	} else {
		assertScenarioBaseValue(t, wire, false, base)
	}
	if len(before) != len(after) {
		t.Fatal("scaffold business message count changed")
	}
	for i, raw := range before {
		var expected map[string]any
		if json.Unmarshal(mustRequestJSON(t, raw), &expected) != nil {
			t.Fatal("scaffold input item invalid")
		}
		actual, _ := after[i].(map[string]any)
		if expected["role"] == "system" {
			expected["role"] = "developer"
		}
		if expected["type"] == nil && expected["role"] != nil {
			expected["type"] = "message"
		}
		if content, ok := expected["content"].(string); ok {
			kind := "input_text"
			if expected["role"] == "assistant" {
				kind = "output_text"
			}
			expected["content"] = []any{map[string]any{"type": kind, "text": content}}
		}
		for field, value := range expected {
			if slices.Contains([]string{"id", "call_id", "previous_item_id"}, field) {
				if id, ok := actual[field].(string); !ok || id == "" {
					t.Fatal("scaffold typed reference disappeared")
				}
				continue
			}
			if !reflect.DeepEqual(value, actual[field]) {
				t.Fatalf("scaffold ordered item %d field %s changed", i, field)
			}
		}
	}
	var flattened []any
	for _, raw := range tools {
		tool, _ := raw.(map[string]any)
		if tool["type"] == "namespace" && tool["name"] == "functions" {
			children, _ := tool["tools"].([]any)
			flattened = append(flattened, children...)
		} else {
			flattened = append(flattened, raw)
		}
	}
	inputTools, _ := caller["tools"].([]any)
	if len(inputTools) != len(flattened) {
		t.Fatal("scaffold tool inventory changed")
	}
	for i, raw := range inputTools {
		original, _ := raw.(map[string]any)
		actual, _ := flattened[i].(map[string]any)
		for field, value := range original {
			if !reflect.DeepEqual(value, actual[field]) {
				t.Fatalf("scaffold tool %d field %s changed", i, field)
			}
		}
	}
	for _, field := range []string{"model", "tool_choice", "text", "reasoning"} {
		if original, ok := caller[field].(map[string]any); ok {
			actual, _ := wire.value[field].(map[string]any)
			for key, value := range original {
				if !reflect.DeepEqual(value, actual[key]) {
					t.Fatalf("scaffold request %s.%s changed", field, key)
				}
			}
		} else if value, exists := caller[field]; exists && !reflect.DeepEqual(value, wire.value[field]) {
			t.Fatalf("scaffold request field %s changed", field)
		}
	}
	for _, field := range []string{"max_output_tokens", "temperature", "top_p"} {
		if _, exists := wire.value[field]; exists {
			t.Fatal("scaffold server-unsupported control was forwarded")
		}
	}
}
