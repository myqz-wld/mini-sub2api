//go:build nativeparity && liveparity && scaffoldparity

package integration

import (
	"bytes"
	"encoding/json"
	"path/filepath"
	"strings"
	"testing"
)

func TestLiveSubscriptionOpenCode(t *testing.T) {
	gateway, relay := newLiveCallerWireGateway(t, 8)
	for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
		t.Run(model, func(t *testing.T) {
			client := startOpenCode(t, gateway.server.URL, gateway.secret, model)
			created := client.call(t, "/session", map[string]any{"title": "Synthetic live session"})
			session, ok := created["id"].(string)
			if !ok || session == "" {
				t.Fatal("live OpenCode session absent")
			}
			for _, word := range []string{"PASS", "NEXT"} {
				result := client.turn(t, session, "Reply with exactly the single word "+word+". Do not use tools.")
				info, _ := result["info"].(map[string]any)
				if info["error"] != nil {
					t.Fatal("live OpenCode reported an inference error")
				}
				parts, _ := result["parts"].([]any)
				var answer strings.Builder
				for _, raw := range parts {
					part, _ := raw.(map[string]any)
					if part["type"] == "text" {
						if text, ok := part["text"].(string); ok {
							answer.WriteString(text)
						}
					}
				}
				if strings.TrimSpace(answer.String()) != word {
					t.Fatal("live OpenCode did not consume the requested response")
				}
			}
		})
	}
	assertLiveCallerWire(t, gateway, relay)
}

func TestLiveSubscriptionOpenCodeRead(t *testing.T) {
	gateway, relay := newLiveCallerWireGateway(t, 8)
	for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
		t.Run(model, func(t *testing.T) {
			client := startOpenCodeWithRead(t, gateway.server.URL, gateway.secret, model, true)
			created := client.call(t, "/session", map[string]any{"title": "Synthetic live read"})
			session, ok := created["id"].(string)
			if !ok || session == "" {
				t.Fatal("live OpenCode read session absent")
			}
			before := len(gateway.tap.packets(t))
			result := client.turn(t, session, "Use the read tool exactly once to read "+filepath.Join(client.project, "fixture.txt")+". Reply exactly with that file's content and no formatting. Do not call any other tool.")
			info, _ := result["info"].(map[string]any)
			if info["error"] != nil {
				t.Fatal("live OpenCode read inference failed")
			}
			parts, _ := result["parts"].([]any)
			var answer strings.Builder
			// The public prompt endpoint returns the last assistant message only; the tool
			// step belongs to an earlier message. Validate that step on the captured continuation.
			for _, raw := range parts {
				part, _ := raw.(map[string]any)
				if part["type"] == "text" {
					if text, ok := part["text"].(string); ok {
						answer.WriteString(text)
					}
				}
			}
			if strings.TrimSpace(answer.String()) != "SYNTHETIC_FILE_VALUE" {
				t.Fatal("live OpenCode did not consume the real fixture result")
			}
			packets := gateway.tap.packets(t)[before:]
			if len(packets) != 2 {
				t.Fatal("live OpenCode read inference count differs")
			}
			var request map[string]any
			if json.Unmarshal(packets[1].payload, &request) != nil {
				t.Fatal("live OpenCode continuation JSON invalid")
			}
			calls, outputs := 0, 0
			callID := ""
			input, _ := request["input"].([]any)
			for _, raw := range input {
				item, _ := raw.(map[string]any)
				if item["type"] == "function_call" {
					calls++
					callID, _ = item["call_id"].(string)
					if item["name"] != "read" || callID == "" {
						t.Fatal("live OpenCode emitted an unexpected tool call")
					}
				}
				if item["type"] == "function_call_output" {
					value, _ := json.Marshal(item["output"])
					if callID == "" || item["call_id"] != callID || !bytes.Contains(value, []byte("SYNTHETIC_FILE_VALUE")) {
						t.Fatal("live OpenCode read result correlation or content changed")
					}
					outputs++
				}
			}
			if calls != 1 || outputs != 1 {
				t.Fatal("live OpenCode continuation lacks executed read result")
			}
		})
	}
	assertLiveCallerWire(t, gateway, relay)
}
