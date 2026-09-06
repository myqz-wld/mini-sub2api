//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"testing"
)

// These keys resemble other carriers but are not reserved by pinned native
// responses_metadata.rs. Only their location gives them meaning.
func fingerprintBoundaryExtras() map[string]string {
	extra := map[string]string{}
	for _, field := range []string{
		"response_id", "conversation_id", "item_id", "inference_call_id",
		"traceparent", "tracestate", "ws_request_header_traceparent",
		"ws_request_header_tracestate", "x-codex-turn-state", "x-client-request-id",
		"Session_id", "TURN_ID", "tool_namespaces_info.case", "arbitrary 任意",
	} {
		extra[field] = "synthetic opaque é🚀 " + strings.Repeat("x", 140)
	}
	return extra
}

func TestNativeExtraMetadataNamespace(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, ws := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/ws=%t", route, ws), func(t *testing.T) {
				capture := newNativeCapture(t)
				options := nativeOptions{endpoint: capture.server.URL, model: "gpt-5.4", ws: ws}
				var gateway nativeGateway
				if route != "direct" {
					gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
					options.endpoint, options.bearer = gateway.server.URL, gateway.secret
				}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				extra := fingerprintBoundaryExtras()
				// Reserved keys are negative controls: native filters these before
				// metadata construction while the case/prefix lookalikes survive.
				input := fingerprintBoundaryExtras()
				for _, name := range []string{"session_id", "turn_id", "tool_namespaces_info", "code_mode_tool_names", "x-codex-installation-id"} {
					input[name] = "reserved-probe-sentinel"
				}
				nativeScenarioTraceTurn(client, thread, input, nil)
				nativeScenarioTraceTurn(client, thread, nil, nil)
				packets := capture.tap.packets(t)
				if route != "direct" {
					packets = gateway.tap.packets(t)
				}
				wires := capture.snapshot()
				if len(packets) != len(wires) || len(businessWires(wires)) != 3 {
					t.Fatal("namespace probe request sequence changed")
				}
				business := 0
				for index, wire := range wires {
					if route == "api-key" && !bytes.Equal(packets[index].payload, wire.encodedBody) {
						t.Fatal("API-key namespace probe bytes changed")
					}
					before := nativeScenarioPacketValue(t, packets[index])
					prewarm := before["generate"] == false
					present := !prewarm && business < 2
					for _, value := range []map[string]any{before, wire.value} {
						flat, nested := nativeScenarioTraceMetadata(t, value)
						for field, expected := range extra {
							if present && nested[field] != expected {
								t.Fatalf("opaque nested field changed: %s", field)
							}
							if !present && nested[field] != nil {
								t.Fatalf("opaque nested field survived reset: %s", field)
							}
							if flat[field] != nil {
								// Native client.rs:1788 independently emits the real
								// response routing token in this flat carrier. Require
								// that exact mock token, never the similarly named extra.
								if field != "x-codex-turn-state" || flat[field] != "native-routing-token" {
									t.Fatalf("nested custom field promoted to flat carrier: %s", field)
								}
							}
						}
						for _, field := range []string{"session_id", "turn_id", "tool_namespaces_info", "code_mode_tool_names", "x-codex-installation-id"} {
							if nested[field] == "reserved-probe-sentinel" {
								t.Fatalf("reserved key override survived: %s", field)
							}
						}
					}
					for _, headers := range []http.Header{packets[index].headers, wire.headers} {
						var nested map[string]any
						raw := headers.Get("X-Codex-Turn-Metadata")
						if json.Unmarshal([]byte(raw), &nested) != nil {
							t.Fatal("namespace probe header metadata missing")
						}
						for _, character := range raw {
							if character > 127 {
								t.Fatal("header metadata is not ASCII JSON")
							}
						}
						for field, expected := range extra {
							if present && !ws && nested[field] != expected {
								t.Fatalf("opaque header field changed: %s", field)
							}
							if (ws || !present) && nested[field] != nil {
								t.Fatalf("custom field crossed header lifetime: %s", field)
							}
						}
						if headers.Get("Traceparent") != "" || headers.Get("Tracestate") != "" || headers.Get("X-Codex-Inference-Call-Id") != "" {
							t.Fatal("opaque custom metadata generated an active trace header")
						}
					}
					if !prewarm {
						business++
					}
				}
			})
		}
	}
}

func TestNativeHeaderOnlyTrace(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		t.Run(fmt.Sprintf("subscription=%t", subscription), func(t *testing.T) {
			capture := newNativeCapture(t)
			gateway := newNativeGateway(t, capture.server.URL, subscription)
			for _, present := range []bool{true, false} {
				headers := make(http.Header)
				if present {
					for name, value := range nativeScenarioTraceParent(0) {
						headers.Set(name, value)
					}
				}
				client := ordinaryClient(t, gateway, false, true, headers)
				client.send(map[string]any{"model": "gpt-5.4", "instructions": "synthetic", "input": "synthetic header-only trace"})
			}
			wires := capture.snapshot()
			if len(wires) != 2 {
				t.Fatal("header-only trace sequence changed")
			}
			for field, expected := range nativeScenarioTraceParent(0) {
				if wires[0].headers.Get(field) != expected || wires[1].headers.Get(field) != "" {
					t.Fatalf("header-only trace changed or inherited: %s", field)
				}
			}
		})
	}
}
