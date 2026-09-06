//go:build nativeparity

package integration

import (
	"bytes"
	"fmt"
	"strings"
	"testing"
)

func TestNativeScenarioCompactionTokenBudgetWindows(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("%s/ws=%t/%s", route, ws, model), func(t *testing.T) {
					capture := newNativeCompactionCapture(t, "v2", false)
					options := nativeOptions{endpoint: capture.server.URL, model: model, ws: ws, base: "synthetic token-budget base", neutralPersonality: true, configOverrides: map[string]string{"features.token_budget": "true"}}
					var gateway nativeGateway
					if route != "direct" {
						gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
						options.endpoint, options.bearer = gateway.server.URL, gateway.secret
					}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					client.turn(thread, "synthetic token-budget seed")
					for _, prompt := range []string{"synthetic token-budget second", "synthetic token-budget third"} {
						accepted, failed := runNativeManualCompaction(t, client, thread)
						if !accepted || failed {
							t.Fatal("token-budget manual reset failed")
						}
						client.turn(thread, prompt)
					}
					all := capture.snapshot()
					business := businessWires(all)
					if len(business) != 3 {
						t.Fatal("token-budget manual reset unexpectedly sampled a model")
					}
					for i, wire := range business {
						metadata := compactionMetadata(t, wire)
						if metadata["request_kind"] != "turn" || metadata["window_number"] != float64(i) {
							t.Fatal("token-budget reset request/window kind changed")
						}
						if i > 0 && wire.value["previous_response_id"] != nil {
							t.Fatal("fresh token-budget window reused an old WS baseline")
						}
					}
					effective := businessWires(capture.effectiveWires(t))
					for i, wire := range effective {
						types, texts := compactionItemCounts(wire)
						if types["compaction"] != 0 || types["compaction_trigger"] != 0 {
							t.Fatal("token-budget reset invented a compaction payload")
						}
						if i > 0 && (texts["synthetic token-budget seed"] != 0 || texts["synthetic precompaction assistant"] != 0) {
							t.Fatal("fresh window retained cleared sampled history")
						}
					}
					var caller []nativeWire
					if route == "direct" {
						caller = all
					} else {
						packets := gateway.tap.packets(t)
						if len(packets) != len(all) {
							t.Fatal("token-budget mediation count changed")
						}
						for i, packet := range packets {
							caller = append(caller, compactionPacketWire(t, packet))
							if route == "api-key" && !bytes.Equal(packet.payload, all[i].encodedBody) {
								t.Fatal("API-key token-budget request changed")
							}
						}
					}
					nativeWindows := assertTokenBudgetWindows(t, caller, false)
					upstreamWindows := assertTokenBudgetWindows(t, all, route == "subscription")
					for number, native := range nativeWindows {
						upstream := upstreamWindows[number]
						if native != upstream {
							t.Fatal("gateway changed caller-owned context-window text")
						}
					}
					if route == "subscription" {
						t.Log("POLICY DIFFERENCE: native context UUID text is preserved while nested metadata UUID is projected; same-window text/metadata equality therefore differs without a proven execution failure")
					}
					t.Logf("token-budget actual native: route=%s ws=%t windows=[0,1,2] sampling_calls=3 compact_sampling_calls=0 first_previous_current_relations=true", route, ws)
				})
			}
		}
	}
}

type tokenBudgetWindowIDs struct{ first, current, previous string }

func assertTokenBudgetWindows(t *testing.T, wires []nativeWire, projected bool) map[int]tokenBudgetWindowIDs {
	t.Helper()
	windows := map[int]tokenBudgetWindowIDs{}
	for _, wire := range wires {
		ids, count := tokenBudgetWindowText(t, wire)
		if count == 0 {
			continue
		}
		if count != 1 {
			t.Fatal("context-window text was not a single separate developer message")
		}
		meta := compactionMetadata(t, wire)
		number, ok := meta["window_number"].(float64)
		if !ok {
			t.Fatal("token-budget window number missing")
		}
		if (meta["context_window_id"] == ids.current) == projected {
			t.Fatal("token-budget text/metadata projection observation changed")
		}
		if saved, seen := windows[int(number)]; seen && saved != ids {
			t.Fatal("context-window text IDs changed inside one window")
		}
		windows[int(number)] = ids
	}
	if len(windows) != 3 {
		t.Fatalf("token-budget context windows captured=%d", len(windows))
	}
	for i := 0; i < 3; i++ {
		ids := windows[i]
		if !isUUIDVersion(ids.first, '7') || !isUUIDVersion(ids.current, '7') || ids.first != windows[0].current {
			t.Fatal("first/current context-window identity lifetime changed")
		}
		if i == 0 && ids.previous != "" {
			t.Fatal("initial context had a previous window")
		}
		if i > 0 && (ids.previous != windows[i-1].current || ids.current == ids.previous) {
			t.Fatal("context-window predecessor was not the prior accepted window")
		}
	}
	return windows
}

func tokenBudgetWindowText(t *testing.T, wire nativeWire) (tokenBudgetWindowIDs, int) {
	t.Helper()
	var ids tokenBudgetWindowIDs
	count := 0
	input, _ := wire.value["input"].([]any)
	for _, raw := range input {
		item, _ := raw.(map[string]any)
		parts, _ := item["content"].([]any)
		for _, raw := range parts {
			part, _ := raw.(map[string]any)
			text, _ := part["text"].(string)
			if !strings.HasPrefix(text, "<context_window>\n") {
				continue
			}
			count++
			if item["role"] != "developer" || len(parts) != 1 || !strings.HasSuffix(text, "</context_window>") {
				t.Fatal("native context-window message boundary changed")
			}
			for _, line := range strings.Split(text, "\n") {
				if value, ok := strings.CutPrefix(line, "First context window id: "); ok {
					ids.first = value
				}
				if value, ok := strings.CutPrefix(line, "Current context window id: "); ok {
					ids.current = value
				}
				if value, ok := strings.CutPrefix(line, "Previous context window id: "); ok {
					ids.previous = value
				}
			}
		}
	}
	return ids, count
}
