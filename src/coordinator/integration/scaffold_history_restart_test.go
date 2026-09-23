//go:build nativeparity && scaffoldparity

package integration

import "testing"

func TestOpenCodeAnonymousFullHistoryAfterCoreRestart(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
		t.Run(model, func(t *testing.T) {
			capture := newScaffoldCaptureWithTurnMetadata(t, true)
			gateway := newNativeGateway(t, capture.server.URL, true)
			client := startOpenCode(t, gateway.server.URL, gateway.secret, model)
			created := client.call(t, "/session", map[string]any{"title": "Synthetic restart history"})
			session, ok := created["id"].(string)
			if !ok || session == "" {
				t.Fatal("OpenCode session missing")
			}
			for step, prompt := range []string{"Reply briefly to this synthetic first turn.", "Reply briefly to this synthetic continuation."} {
				if step == 1 {
					restartHistoryFixtureCore(t, gateway.supervisor)
				}
				result := client.turn(t, session, prompt)
				info, _ := result["info"].(map[string]any)
				if info["error"] != nil {
					t.Fatal("OpenCode continuation failed")
				}
			}
			packets, wires := gateway.tap.packets(t), capture.snapshot()
			if len(packets) != 2 || len(wires) != 2 {
				t.Fatal("OpenCode restart changed inference count")
			}
			for i := range wires {
				assertScaffoldCustomSession(t, packets[i], session)
				assertScaffoldMessageParity(t, packets[i], wires[i])
				assertCallerWireContract(t, packets[i], wires[i], capture.tap.packets(t)[i])
			}
			first, _ := wires[0].value["client_metadata"].(map[string]any)
			second, _ := wires[1].value["client_metadata"].(map[string]any)
			if first["thread_id"] == second["thread_id"] {
				t.Fatal("expired anonymous history did not establish an independent thread")
			}
			caller := decodeNativePacket(t, packets[1])
			input, _ := caller["input"].([]any)
			assistant, preservedTurns := 0, 0
			for _, raw := range input {
				item, _ := raw.(map[string]any)
				if item["role"] == "assistant" {
					assistant++
				}
				metadata, _ := item["internal_chat_message_metadata_passthrough"].(map[string]any)
				if metadata["turn_id"] != nil {
					preservedTurns++
				}
			}
			if assistant == 0 {
				t.Fatal("OpenCode omitted complete assistant history")
			}
			t.Logf("opencode_restart_complete=true historical_assistants=%d retained_item_turn_fields=%d", assistant, preservedTurns)
		})
	}
}
