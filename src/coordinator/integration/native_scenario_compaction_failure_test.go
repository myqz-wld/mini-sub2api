//go:build nativeparity

package integration

import (
	"bytes"
	"fmt"
	"testing"
)

func TestNativeScenarioCompactionRejectedOutput(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("%s/ws=%t/%s", route, ws, model), func(t *testing.T) {
					capture := newNativeCompactionCapture(t, "v2", true)
					options := nativeOptions{endpoint: capture.server.URL, model: model, ws: ws, base: "synthetic compaction base", neutralPersonality: true, configOverrides: map[string]string{"features.remote_compaction_v2": "true", "features.token_budget": "false"}}
					var gateway nativeGateway
					if route != "direct" {
						gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
						options.endpoint, options.bearer = gateway.server.URL, gateway.secret
					}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					client.turn(thread, "synthetic compaction user seed")
					accepted, failed := runNativeManualCompaction(t, client, thread)
					if accepted || !failed {
						t.Fatal("native accepted completed response without one compaction output")
					}
					client.turn(thread, "synthetic postcompaction user")
					beforeRecovery := capture.snapshot()
					business := businessWires(beforeRecovery)
					if len(business) != 3 {
						t.Fatal("failed compaction caused an unexpected replay")
					}
					assertCompactionWindowLifecycle(t, business, false)
					_, texts := compactionItemCounts(businessWires(capture.effectiveWires(t))[2])
					if texts["synthetic compaction user seed"] != 1 || texts["synthetic precompaction assistant"] != 1 || texts["synthetic compact summary"] != 0 {
						t.Fatal("failed compaction mutated durable native history")
					}
					capture.mu.Lock()
					capture.fail = false
					capture.mu.Unlock()
					accepted, failed = runNativeManualCompaction(t, client, thread)
					if !accepted || failed {
						t.Fatal("subsequent valid compact failed")
					}
					client.turn(thread, "synthetic after recovered compact")
					all := capture.snapshot()
					business = businessWires(all)
					if len(business) != 5 {
						t.Fatal("recovery compaction inference count changed")
					}
					if route == "direct" {
						meta := compactionMetadata(t, business[3])
						if meta["window_number"] != float64(0) {
							t.Fatal("native failed compact committed a window")
						}
					} else {
						packets := gateway.tap.packets(t)
						if len(packets) != len(all) {
							t.Fatal("failed compaction mediation count changed")
						}
						var nativeBusiness []nativeWire
						for i, packet := range packets {
							wire := compactionPacketWire(t, packet)
							if wire.value["generate"] != false {
								nativeBusiness = append(nativeBusiness, wire)
							}
							if route == "api-key" && !bytes.Equal(packet.payload, all[i].encodedBody) {
								t.Fatal("API-key changed rejected compaction packet")
							}
						}
						nativeMeta, upstreamMeta := compactionMetadata(t, nativeBusiness[3]), compactionMetadata(t, business[3])
						if nativeMeta["window_number"] != float64(0) {
							t.Fatal("native rejected compaction counter changed")
						}
						wantCounter := float64(0)
						expected := fmt.Sprintf("%s:0", upstreamMeta["thread_id"])
						if route == "subscription" {
							wantCounter = 1
							expected = fmt.Sprintf("%s:1", upstreamMeta["thread_id"])
							t.Log("KNOWN conformance difference: rejected V2 output commits gateway window; next compact emits window_number=1 while native emits 0")
						}
						if upstreamMeta["window_number"] != wantCounter {
							t.Fatalf("rejected compaction observed upstream window_number=%v", upstreamMeta["window_number"])
						}
						if upstreamMeta["window_id"] != expected {
							t.Fatal("rejected compaction baseline observation changed")
						}
					}
					last := compactionMetadata(t, business[4])
					if last["window_number"] != float64(1) {
						t.Fatal("valid recovery did not advance native window once")
					}
					t.Logf("rejected native compact: route=%s ws=%t retained_history=true native_windows=[0,0,0,0,1]", route, ws)
				})
			}
		}
	}
}
