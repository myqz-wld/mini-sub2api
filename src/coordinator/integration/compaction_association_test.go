//go:build nativeparity

package integration

import (
	"bytes"
	"fmt"
	"net/http"
	"testing"
)

func checkpointReplacement(first map[string]any, checkpoint any, format string) map[string]any {
	base := "current checkpoint base"
	items := []any{
		map[string]any{"role": "developer", "content": "fresh client environment"},
		checkpoint,
		compactionUser("post-checkpoint user"),
	}
	request := map[string]any{"model": first["model"], "instructions": base, "tools": []any{}, "input": items}
	if format == "native-lite" {
		request["input"] = append([]any{
			map[string]any{"type": "additional_tools", "role": "developer", "tools": []any{}},
			map[string]any{"role": "developer", "content": base},
		}, items...)
		delete(request, "instructions")
		delete(request, "tools")
	}
	return request
}

func TestCompactionAnonymousAssociation(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, delivery := range []string{"http-json", "http-sse", "ws-reconnect"} {
			for _, format := range []string{"ordinary", "converted-lite", "native-lite"} {
				for _, marked := range []bool{false, true} {
					t.Run(fmt.Sprintf("subscription=%t/%s/%s/marked=%t", subscription, delivery, format, marked), func(t *testing.T) {
						capture := newNativeCompactionCapture(t, "v2", false)
						gateway := newNativeGateway(t, capture.server.URL, subscription)
						headers := http.Header{}
						if marked {
							headers.Set("Originator", "codex_cli_rs")
						}
						ws := delivery == "ws-reconnect"
						client := ordinaryClient(t, gateway, ws, delivery != "http-json", headers.Clone())
						first := ordinaryRequest(format, "anonymous")
						seed := client.send(first)
						compact := cloneOrdinaryContext(t, first)
						items := append(compact["input"].([]any), seed["output"].([]any)...)
						compact["input"] = append(items, map[string]any{"type": "compaction_trigger"})
						compact["client_metadata"] = map[string]any{"x-codex-turn-metadata": string(mustRequestJSON(t,
							map[string]any{"request_kind": "compaction", "compaction": map[string]any{"implementation": "responses_compaction_v2"}}))}
						checkpoint := client.send(compact)
						if client.ws != nil {
							_ = client.ws.CloseNow()
							client = ordinaryClient(t, gateway, true, true, headers.Clone())
						}
						current := checkpointReplacement(first, checkpoint["output"].([]any)[0], format)
						response := client.send(current)
						client.send(compactionDelta(current, response["id"], "later checkpoint user"))
						wires := capture.snapshot()
						business := businessWires(wires)
						if len(business) != 4 {
							t.Fatal("anonymous compaction inference count changed")
						}
						wantFrames := 4
						if subscription && ws && !marked {
							wantFrames += 2
						}
						if len(wires) != wantFrames {
							t.Fatal("anonymous compaction added an unexpected prewarm")
						}
						packets := gateway.tap.packets(t)
						for _, packet := range packets {
							value := decodeNativePacket(t, packet)
							flat, _ := value["client_metadata"].(map[string]any)
							if packet.headers.Get("Session-Id") != "" || flat["session_id"] != nil {
								t.Fatal("anonymous fixture supplied an explicit session")
							}
						}
						assertCheckpointRawCapture(t, capture, wires)
						if !subscription {
							assertCompactionPassthroughPackets(t, packets, business)
							return
						}
						assertCheckpointIdentity(t, business, wires, ws)
						for _, wire := range wires {
							items, _ := wire.value["input"].([]any)
							if len(items) > 0 && items[0].(map[string]any)["type"] == "additional_tools" {
								if format == "native-lite" {
									assertBareFormedLitePrefix(t, wire)
								} else {
									assertNativeLiteIDs(t, wire)
								}
							}
						}
						effective := businessWires(capture.effectiveWires(t))
						for index, wire := range effective[2:] {
							types, texts := compactionItemCounts(wire)
							if types["compaction"] != 1 || types["compaction_trigger"] != 0 ||
								texts["first ordinary user"] != 0 || texts["system fixture"] != 0 || texts["duplicate developer"] != 0 ||
								texts["fresh client environment"] != 1 || texts["post-checkpoint user"] != 1 || texts["later checkpoint user"] != index {
								t.Fatal("checkpoint association changed the caller replacement context")
							}
							assertCheckpointCurrentInput(t, wire, format)
						}
					})
				}
			}
		}
	}
}

func assertCheckpointIdentity(t *testing.T, business, all []nativeWire, ws bool) {
	t.Helper()
	before := compactionMetadata(t, business[0])
	for index, wire := range business {
		flat, meta := identityMetadata(t, wire)
		for _, field := range []string{"session_id", "thread_id"} {
			if meta[field] != before[field] || flat[field] != meta[field] {
				t.Fatal("checkpoint association lost session or thread identity")
			}
		}
		window := 0
		if index >= 2 {
			window = 1
		}
		if meta["window_id"] != fmt.Sprintf("%s:%d", meta["thread_id"], window) || flat["x-codex-window-id"] != meta["window_id"] {
			t.Fatal("checkpoint association lost composite window position")
		}
		// Bare callers do not supply these optional native fields. Core preserves their absence;
		// the actual-native lifecycle controls separately verify supplied counters/window UUIDs.
		if meta["window_number"] != nil || meta["context_window_id"] != nil {
			t.Fatal("checkpoint association invented optional native window metadata")
		}
		if wire.headers.Get("Session-Id") != meta["session_id"] {
			t.Fatal("checkpoint body session differs from HTTP or WS handshake header")
		}
		if wire.method == "POST" && wire.headers.Get("X-Codex-Window-Id") != meta["window_id"] {
			t.Fatal("checkpoint identity differs between body and HTTP headers")
		}
	}
	compact, after := compactionMetadata(t, business[1]), compactionMetadata(t, business[2])
	if compact["turn_id"] == after["turn_id"] || !isUUIDVersion(after["turn_id"].(string), '7') {
		t.Fatal("checkpoint association inherited the old turn")
	}
	if previous := business[2].value["previous_response_id"]; previous != nil {
		// Ordinary callers may establish a fresh, full hidden prewarm on the new socket. Only
		// that actual upstream baseline can justify a delta; checkpoint identity alone cannot.
		warmed := false
		for i, wire := range all {
			if bytes.Equal(wire.encodedBody, business[2].encodedBody) {
				break
			}
			if wire.value["generate"] == false && wire.connection == business[2].connection &&
				previous == fmt.Sprintf("resp_compaction_%d", i+1) {
				warmed = true
			}
		}
		if !warmed {
			t.Fatal("checkpoint identity reused a response without a fresh socket baseline")
		}
	}
	if ws && business[0].connection == business[2].connection {
		t.Fatal("checkpoint reconnect fixture reused its first socket")
	}
}

func assertCheckpointCurrentInput(t *testing.T, wire nativeWire, format string) {
	t.Helper()
	items := wire.value["input"].([]any)
	offset := 0
	if format != "ordinary" {
		offset = 2
		if wire.value["instructions"] != nil || wire.value["tools"] != nil {
			t.Fatal("checkpoint Lite context retained top-level setup")
		}
		prefix := items[0].(map[string]any)
		base := items[1].(map[string]any)
		content, _ := base["content"].([]any)
		if prefix["type"] != "additional_tools" || len(prefix["tools"].([]any)) != 0 ||
			base["role"] != "developer" || len(content) != 1 || content[0].(map[string]any)["text"] != "current checkpoint base" {
			t.Fatal("checkpoint Lite setup content or order changed")
		}
	} else if wire.value["instructions"] != "current checkpoint base" || len(wire.value["tools"].([]any)) != 0 {
		t.Fatal("checkpoint ordinary current setup changed")
	}
	checkpoint := items[offset+1].(map[string]any)
	if items[offset].(map[string]any)["role"] != "developer" || items[offset+2].(map[string]any)["role"] != "user" ||
		checkpoint["type"] != "compaction" || checkpoint["id"] != "cmp_compaction_fixture" || checkpoint["encrypted_content"] != "synthetic_compacted_state" {
		t.Fatal("checkpoint item content, identity or placement changed")
	}
}

func assertCheckpointRawCapture(t *testing.T, capture *nativeCompactionCapture, wires []nativeWire) {
	t.Helper()
	packets := capture.tap.packets(t)
	if len(packets) != len(wires) {
		t.Fatal("checkpoint raw capture count differs")
	}
	// This fixture completes all requests on a socket before opening the next socket.
	for i, wire := range wires {
		if !bytes.Equal(packets[i].payload, wire.encodedBody) {
			t.Fatal("checkpoint raw capture differs from application bytes")
		}
	}
}
