//go:build nativeparity && scaffoldparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestOpenCodeReadToolCapture(t *testing.T) {
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
				client := startOpenCodeWithRead(t, endpoint, secret, model, true)
				created := client.call(t, "/session", map[string]any{"title": "Synthetic read-tool session"})
				session, ok := created["id"].(string)
				if !ok {
					t.Fatal("OpenCode read session absent")
				}
				result := client.turn(t, session, "Read fixture: "+filepath.Join(client.project, "fixture.txt")+"\nUse read once and then respond briefly.")
				info, _ := result["info"].(map[string]any)
				if info["error"] != nil {
					t.Fatal("OpenCode read loop failed")
				}
				wires := capture.snapshot()
				if len(wires) != 2 {
					t.Fatal("OpenCode read loop inference count differs")
				}
				found := false
				input, _ := wires[1].value["input"].([]any)
				for _, raw := range input {
					item, _ := raw.(map[string]any)
					if item["type"] == "function_call_output" {
						output, _ := json.Marshal(item["output"])
						if item["call_id"] != "call_scaffold_read" || !bytes.Contains(output, []byte("SYNTHETIC_FILE_VALUE")) {
							text := strings.ToLower(string(output))
							t.Logf("SCAFFOLD_TOOL_RESULT matched_call=%t contains_fixture=%t denied=%t invalid=%t unknown=%t", item["call_id"] == "call_scaffold_read", bytes.Contains(output, []byte("SYNTHETIC_FILE_VALUE")), strings.Contains(text, "denied") || strings.Contains(text, "permission"), strings.Contains(text, "invalid"), strings.Contains(text, "unknown") || strings.Contains(text, "not found"))
							t.Fatal("OpenCode read result or call correlation changed")
						}
						found = true
					}
				}
				if !found {
					t.Fatal("OpenCode did not execute the supplied read call")
				}
				if route == "direct" {
					saveFinalCapture(t, capture.tap.packets(t), wires)
					return
				}
				packets := gateway.tap.packets(t)
				if len(packets) != len(wires) {
					t.Fatal("gateway changed OpenCode read request count")
				}
				for i, wire := range wires {
					if route == "api-key" {
						if !bytes.Equal(packets[i].payload, wire.encodedBody) {
							t.Fatal("OpenCode API-key read request changed")
						}
					} else {
						assertScaffoldCustomSession(t, packets[i], session)
						assertScaffoldMessageParity(t, packets[i], wire)
					}
				}
				if route == "subscription" {
					assertScaffoldContinuation(t, wires, true)
				}
				saveFinalCapture(t, packets, wires)
			})
		}
	}
}

func TestOpenCodeReadPermissionBoundary(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
		t.Run(model, func(t *testing.T) {
			capture := newScaffoldCapture(t)
			client := startOpenCodeWithRead(t, capture.server.URL, "synthetic-opencode-key", model, true)
			forbidden := filepath.Join(client.project, "private-test.txt")
			if os.WriteFile(forbidden, []byte("SYNTHETIC_FORBIDDEN_VALUE"), 0600) != nil {
				t.Fatal("create denied read control")
			}
			created := client.call(t, "/session", map[string]any{"title": "Synthetic denied read"})
			session, ok := created["id"].(string)
			if !ok {
				t.Fatal("OpenCode denied-read session absent")
			}
			client.turn(t, session, "Read fixture: "+forbidden+"\nExercise the permission boundary.")
			wires := capture.snapshot()
			if len(wires) != 2 {
				t.Fatal("OpenCode denied-read inference count differs")
			}
			input, _ := wires[1].value["input"].([]any)
			for _, raw := range input {
				item, _ := raw.(map[string]any)
				if item["type"] == "function_call_output" {
					output, _ := json.Marshal(item["output"])
					text := strings.ToLower(string(output))
					if bytes.Contains(output, []byte("SYNTHETIC_FORBIDDEN_VALUE")) || (!strings.Contains(text, "denied") && !strings.Contains(text, "permission")) {
						t.Fatal("OpenCode read permission escaped its synthetic file")
					}
					return
				}
			}
			t.Fatal("OpenCode denied read did not return a bounded tool error")
		})
	}
}
