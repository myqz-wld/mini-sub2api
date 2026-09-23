//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"reflect"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"
)

func TestNativeScenarioTransportLifecycle(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("%s/ws=%t/%s", route, ws, model), func(t *testing.T) {
					capture := newNativeTransportCapture(t, false, false)
					options := nativeOptions{endpoint: capture.server.URL, model: model, ws: ws, base: "  transport {{literal}} base  ", neutralPersonality: true}
					var gateway nativeGateway
					if route != "direct" {
						gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
						options.endpoint, options.bearer = gateway.server.URL, gateway.secret
					}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					client.turn(thread, "first synthetic transport turn")
					client.turn(thread, "next synthetic transport turn")
					wires := capture.snapshot()
					assertTransportLifecycle(t, wires, ws)
					if route != "direct" {
						packets := gateway.tap.packets(t)
						if len(packets) != len(wires) {
							t.Fatal("transport mediation request count changed")
						}
						projection := &transportProjection{identities: map[string]map[string]string{}}
						for i, wire := range wires {
							if route == "api-key" {
								if !bytes.Equal(packets[i].payload, wire.encodedBody) {
									t.Fatal("API-key lifecycle request payload changed")
								}
							} else {
								assertNativeJSONShape(t, nativePacketJSON(t, packets[i]), wire.body)
								projection.compare(t, transportPacketValue(t, packets[i]), wire.value, fmt.Sprintf("request[%d]", i))
							}
						}
						if route == "api-key" {
							assertProfileStateFileCount(t, gateway.stateDir, 0)
						} else {
							t.Logf("projected scalar differences: %v", projection.known)
						}
					}
					// The raw parser independently validates masks, deflate context and framing.
					raw := capture.tap.packets(t)
					if len(raw) != len(wires) {
						t.Fatal("transport raw tap request count changed")
					}
					for i, wire := range wires {
						if !bytes.Equal(raw[i].payload, wire.encodedBody) {
							t.Fatal("transport raw frames differ from server-decoded messages")
						}
						if ws && raw[i].headers.Get("X-Codex-Turn-State") != "" {
							t.Error("native WS handshake unexpectedly carries a turn-local token")
						}
					}
					t.Logf("transport lifecycle: route=%s ws=%t business=4 first-token-retained=true next-turn-reset=true", route, ws)
				})
			}
		}
	}
}

func assertTransportLifecycle(t *testing.T, all []nativeWire, ws bool, tokens ...string) {
	t.Helper()
	if len(tokens) == 0 {
		tokens = []string{"", "transport-token-1", "transport-token-1", ""}
	}
	if len(tokens) != 4 {
		t.Fatal("transport token oracle shape")
	}
	wires := businessWires(all)
	if len(wires) != 4 {
		t.Fatalf("transport business request count=%d, expected two tool continuations and next turn", len(wires))
	}
	first := transportMetadata(t, wires[0].value)
	firstTurn, _ := first["turn_id"].(string)
	if !isUUIDVersion(firstTurn, '7') {
		t.Fatal("transport initial turn is not UUIDv7")
	}
	for i, wire := range wires {
		metadata := transportMetadata(t, wire.value)
		var nested map[string]any
		text, _ := metadata["x-codex-turn-metadata"].(string)
		if json.Unmarshal([]byte(text), &nested) != nil {
			t.Fatal("transport nested turn metadata missing")
		}
		for _, field := range []string{"session_id", "thread_id", "turn_id", "root_turn_id"} {
			if metadata[field] == nil || metadata[field] != nested[field] {
				t.Errorf("transport flat/nested identity relationship differs: %s", field)
			}
		}
		if metadata["session_id"] != first["session_id"] || metadata["thread_id"] != first["thread_id"] || metadata["session_id"] != metadata["thread_id"] {
			t.Fatal("transport root session/thread identity changed")
		}
		if metadata["root_turn_id"] != metadata["turn_id"] || metadata["parent_turn_id"] != nil || nested["parent_turn_id"] != nil {
			t.Fatal("independent user turn received causal parent or incorrect root turn")
		}
		if (i < 3) != (metadata["turn_id"] == firstTurn) {
			t.Error("tool continuation/new user turn boundary changed")
		}
		if nested["request_kind"] != "turn" || nested["window_number"] != float64(0) {
			t.Fatal("normal sampling invented compaction/window advancement")
		}
		if wire.value["include"] == nil || !reflect.DeepEqual(wire.value["include"], []any{"reasoning.encrypted_content"}) {
			t.Fatal("encrypted reasoning include missing or changed")
		}
		expectedToken := tokens[i]
		var actualToken string
		if ws {
			actualToken, _ = metadata["x-codex-turn-state"].(string)
			if wire.method != http.MethodGet || wire.value["type"] != "response.create" || wire.connection != wires[0].connection {
				t.Fatal("WS lifecycle changed transport, message type or connection")
			}
			if i > 0 && wire.value["previous_response_id"] != fmt.Sprintf("resp_transport_%d", len(all)-len(wires)+i) {
				t.Error("healthy WS tool/next-turn input did not reference its exact completed-response baseline")
			}
			if i > 0 {
				assertTransportItems(t, wire.value, firstTurn, i, true)
			}
		} else {
			actualToken = wire.headers.Get("X-Codex-Turn-State")
			if wire.method != http.MethodPost || wire.value["previous_response_id"] != nil {
				t.Fatal("native HTTP failed to send full context")
			}
			assertTransportItems(t, wire.value, firstTurn, i, false)
		}
		if actualToken != expectedToken {
			t.Errorf("routing token lifecycle differs at business request %d", i)
		}
	}
}

func assertTransportItems(t *testing.T, value map[string]any, firstTurn string, step int, incremental bool) {
	t.Helper()
	input, _ := value["input"].([]any)
	counts := map[string]int{}
	calls, results := map[string]bool{}, map[string]bool{}
	for _, raw := range input {
		item, _ := raw.(map[string]any)
		kind, _ := item["type"].(string)
		counts[kind]++
		if kind == "function_call" || kind == "function_call_output" {
			call, _ := item["call_id"].(string)
			if call == "" {
				t.Fatal("tool dependency reference missing")
			}
			if kind == "function_call" {
				calls[call] = true
			} else {
				results[call] = true
			}
		}
		if kind == "reasoning" {
			id, _ := item["id"].(string)
			n := strings.TrimPrefix(id, "rs_transport_")
			if n != "1" && n != "2" {
				t.Fatal("reasoning item lost provider identity")
			}
			var summary, content any = []any{}, nil
			if n == "1" {
				summary = []any{map[string]any{"type": "summary_text", "text": "synthetic reasoning summary"}}
				content = []any{map[string]any{"type": "reasoning_text", "text": "synthetic reasoning content"}}
			}
			_, contentPresent := item["content"]
			if !contentPresent || item["encrypted_content"] != "transport-opaque-"+n || !reflect.DeepEqual(item["summary"], summary) || !reflect.DeepEqual(item["content"], content) {
				t.Fatal("reasoning content, summary or opaque encrypted state changed")
			}
		}
		if kind == "function_call" || kind == "function_call_output" || kind == "reasoning" || item["role"] == "assistant" {
			metadata, ok := item["internal_chat_message_metadata_passthrough"].(map[string]any)
			if !ok || metadata["turn_id"] != firstTurn {
				t.Error("historical output lost its originating turn or was relabeled")
			}
		}
	}
	want := min(step, 2)
	if incremental {
		want = 0
		if step < 3 && (counts["function_call_output"] != 1 || len(results) != 1) {
			t.Error("WS tool suffix is not exactly one result")
		}
		if step == 3 && counts["function_call_output"] != 0 {
			t.Error("WS next turn replayed consumed tools")
		}
	} else if counts["function_call_output"] != want || len(results) != want || !reflect.DeepEqual(calls, results) {
		t.Error("HTTP full history lost, duplicated or mismatched tool dependencies")
	}
	if counts["reasoning"] != want || counts["function_call"] != want {
		t.Error("history reasoning/call count differs from completed-response baseline")
	}
}

func transportMetadata(t *testing.T, value map[string]any) map[string]any {
	t.Helper()
	metadata, ok := value["client_metadata"].(map[string]any)
	if !ok {
		t.Fatal("transport client metadata missing")
	}
	return metadata
}

func transportPacketValue(t *testing.T, packet nativePacket) map[string]any {
	t.Helper()
	body := packet.payload
	if packet.headers.Get("Content-Encoding") == "zstd" {
		decoder, err := zstd.NewReader(nil, zstd.WithDecoderMaxMemory(nativeCaptureLimit), zstd.WithDecoderConcurrency(1))
		if err != nil {
			t.Fatal("transport packet decoder")
		}
		defer decoder.Close()
		body, err = decoder.DecodeAll(body, nil)
		if err != nil {
			t.Fatal("transport packet decompression")
		}
	}
	var value map[string]any
	if json.Unmarshal(body, &value) != nil {
		t.Fatal("transport packet JSON")
	}
	return value
}
