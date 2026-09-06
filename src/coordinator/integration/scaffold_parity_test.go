//go:build nativeparity && scaffoldparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"reflect"
	"slices"
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
					assertScaffoldMessageParity(t, packets[i], wire)
					if wire.value["previous_response_id"] != nil {
						t.Fatal("OpenCode Subscription HTTP sent an incremental reference")
					}
					if model == "gpt-6-astra" {
						assertNativeLiteIDs(t, wire)
					}
				}
			})
		}
	}
}

// A third-party Responses producer need not send native item wrappers or a native base prefix.
// Compare its business fields using the declared ordinary conversion rules, without erasing text.
func assertScaffoldMessageParity(t *testing.T, packet nativePacket, wire nativeWire) {
	t.Helper()
	if wire.headers.Get("Originator") != "codex-tui" || wire.headers.Get("Version") != "0.153.4" {
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
	after, _ := wire.value["input"].([]any)
	tools, _ := wire.value["tools"].([]any)
	if len(after) > 0 && after[0].(map[string]any)["type"] == "additional_tools" {
		tools, _ = after[0].(map[string]any)["tools"].([]any)
		if len(after) < 2 || after[1].(map[string]any)["role"] != "developer" {
			t.Fatal("scaffold Lite base carrier missing")
		}
		after = after[2:]
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
	if base, ok := caller["instructions"].(string); ok && base != "" && capturedBase(t, wire) != base {
		t.Fatal("scaffold caller base changed")
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
