//go:build nativeparity && scaffoldparity

package integration

import (
	"bytes"
	"fmt"
	"testing"
)

// This exercises the real built-in OpenAI header hook, separately from custom-provider history.
// Authentication remains a synthetic API key and every endpoint is an isolated loopback mock.
func TestOpenCodeBuiltInSessionHeader(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
		for _, subscription := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/subscription=%t", model, subscription), func(t *testing.T) {
				capture := newScaffoldCapture(t)
				gateway := newNativeGateway(t, capture.server.URL, subscription)
				client := startOpenCodeProvider(t, gateway.server.URL, gateway.secret, model, "openai", false)
				created := client.call(t, "/session", map[string]any{"title": "Synthetic built-in provider session"})
				session, ok := created["id"].(string)
				if !ok || session == "" {
					t.Fatal("OpenCode built-in provider session missing")
				}
				for _, prompt := range []string{"Reply briefly to a synthetic first turn.", "Reply briefly to a synthetic next turn."} {
					result := client.turn(t, session, prompt)
					info, _ := result["info"].(map[string]any)
					if info["error"] != nil {
						t.Fatal("OpenCode built-in provider inference failed")
					}
				}
				packets, wires := gateway.tap.packets(t), capture.snapshot()
				if len(packets) != 2 || len(wires) != 2 {
					t.Fatal("OpenCode built-in provider request count differs")
				}
				for i, packet := range packets {
					if packet.headers.Get("Session-Id") != session || packet.headers.Get("Originator") != "opencode" {
						t.Fatal("built-in OpenAI plugin did not supply Codex session and originator headers")
					}
					if subscription {
						assertScaffoldMessageParity(t, packet, wires[i])
						assertCallerWireContract(t, packet, wires[i], capture.tap.packets(t)[i])
					} else if !bytes.Equal(packet.payload, wires[i].encodedBody) {
						t.Fatal("API-key built-in provider payload changed")
					}
				}
				if subscription {
					assertScaffoldContinuation(t, wires, false)
				}
			})
		}
	}
}
