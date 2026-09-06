//go:build nativeparity

package integration

import (
	"bufio"
	"bytes"
	"fmt"
	"io"
	"net/http"
	"testing"
)

func TestNativeScenarioCompactionLegacyEndpoint(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("%s/ws=%t/%s", route, ws, model), func(t *testing.T) {
					capture := newNativeCompactionCapture(t, "v1", false)
					options := nativeOptions{endpoint: capture.server.URL, model: model, ws: ws, base: "synthetic compaction base", neutralPersonality: true, configOverrides: map[string]string{"features.remote_compaction_v2": "false", "features.token_budget": "false"}}
					var gateway nativeGateway
					if route != "direct" {
						gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
						options.endpoint, options.bearer = gateway.server.URL, gateway.secret
					}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					client.turn(thread, "synthetic compaction user seed")
					accepted, failed := runNativeManualCompaction(t, client, thread)
					if accepted != (route == "direct") || failed != (route != "direct") {
						t.Fatal("legacy Compact endpoint acceptance changed")
					}
					client.turn(thread, "synthetic postcompaction user")
					tap := capture.tap
					if route != "direct" {
						tap = gateway.tap
					}
					compactPackets := legacyCompactionPackets(t, tap)
					if len(compactPackets) != 1 {
						t.Fatalf("native legacy Compact HTTP attempts=%d", len(compactPackets))
					}
					compact := compactionPacketWire(t, compactPackets[0])
					assertLegacyCompactionShape(t, compact, model)
					business := businessWires(capture.snapshot())
					if route == "direct" {
						if len(business) != 3 {
							t.Fatal("direct legacy compact sample count changed")
						}
						assertCompactionWindowLifecycle(t, business, true)
						types, texts := compactionItemCounts(business[2])
						if types["compaction"] != 1 || types["function_call"] != 0 || texts["synthetic stale server developer"] != 0 || texts["synthetic server retained user"] != 1 || texts["synthetic compaction user seed"] != 0 {
							t.Fatal("legacy replacement filtering differs from native source")
						}
						if business[2].value["previous_response_id"] != nil {
							t.Fatal("legacy replacement reused old sampling baseline")
						}
					} else {
						if len(business) != 2 {
							t.Fatal("unsupported public Compact endpoint reached upstream")
						}
						for _, wire := range business {
							if compactionMetadata(t, wire)["window_number"] != float64(0) {
								t.Fatal("rejected endpoint committed a window")
							}
						}
						_, texts := compactionItemCounts(businessWires(capture.effectiveWires(t))[1])
						if texts["synthetic compaction user seed"] != 1 || texts["synthetic precompaction assistant"] != 1 {
							t.Fatal("unsupported Compact endpoint changed native history")
						}
						t.Log("SCOPE: actual native /responses/compact attempt is unsupported by the Responses-only public gateway for both credential types")
					}
					t.Logf("legacy native compact: route=%s sampling_ws=%t compact_http=true accepted=%t", route, ws, accepted)
				})
			}
		}
	}
}

func assertLegacyCompactionShape(t *testing.T, wire nativeWire, model string) {
	t.Helper()
	if wire.method != http.MethodPost {
		t.Fatal("native legacy compact is not HTTP POST")
	}
	for _, field := range []string{"client_metadata", "include", "stream", "store", "tool_choice", "previous_response_id"} {
		if _, present := wire.value[field]; present {
			t.Errorf("native legacy compact unexpectedly includes %s", field)
		}
	}
	if wire.headers.Get("X-Codex-Installation-Id") == "" || compactionMetadata(t, wire)["request_kind"] != "compaction" {
		t.Fatal("legacy compact identity headers missing")
	}
	if model == "gpt-5.4" {
		if wire.value["instructions"] != "synthetic compaction base" || wire.value["tools"] == nil {
			t.Fatal("ordinary legacy compact base/tools location changed")
		}
	} else {
		if wire.value["instructions"] != nil || wire.value["tools"] != nil {
			t.Fatal("Lite legacy compact top-level carrier was emitted")
		}
		input, _ := wire.value["input"].([]any)
		if len(input) < 2 || input[0].(map[string]any)["type"] != "additional_tools" {
			t.Fatal("Lite legacy compact tools prefix missing")
		}
	}
}

func legacyCompactionPackets(t *testing.T, tap *nativeTap) []nativePacket {
	t.Helper()
	_ = tap.packets(t) // Includes the shared bound/framing checks; that helper filters legacy paths.
	tap.mu.Lock()
	connections := append([]*nativeTapConn(nil), tap.connections...)
	tap.mu.Unlock()
	var packets []nativePacket
	for _, connection := range connections {
		connection.mu.Lock()
		raw := append([]byte(nil), connection.received...)
		connection.mu.Unlock()
		reader := bufio.NewReader(bytes.NewReader(raw))
		for {
			request, err := http.ReadRequest(reader)
			if err == io.EOF {
				break
			}
			if err != nil {
				t.Fatal("legacy capture HTTP framing")
			}
			if request.Header.Get("Upgrade") != "" {
				break
			}
			body, err := io.ReadAll(io.LimitReader(request.Body, nativeCaptureLimit+1))
			_ = request.Body.Close()
			if err != nil || len(body) > nativeCaptureLimit {
				t.Fatal("legacy body capture bound")
			}
			if request.URL.Path == "/v1/responses/compact" {
				packets = append(packets, nativePacket{method: request.Method, headers: request.Header.Clone(), payload: body})
			}
		}
	}
	return packets
}
