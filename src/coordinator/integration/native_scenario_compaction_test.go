//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"reflect"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"
)

func TestNativeScenarioCompactionLifecycle(t *testing.T) {
	for _, mode := range []string{"v2", "local"} {
		for _, route := range []string{"direct", "api-key", "subscription"} {
			for _, ws := range []bool{false, true} {
				for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
					t.Run(fmt.Sprintf("%s/%s/ws=%t/%s", mode, route, ws, model), func(t *testing.T) {
						capture := newNativeCompactionCapture(t, mode, false)
						options := nativeOptions{endpoint: capture.server.URL, model: model, ws: ws, base: "synthetic compaction base", neutralPersonality: true, configOverrides: map[string]string{"features.remote_compaction_v2": "true", "features.token_budget": "false", "compact_prompt": `"synthetic compact instruction"`}}
						if mode == "local" {
							options.providerName = "Native Local Capture"
						}
						var gateway nativeGateway
						if route != "direct" {
							gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
							options.endpoint, options.bearer = gateway.server.URL, gateway.secret
						}
						client := startNativeClient(t, options)
						thread := client.thread(options)
						client.turn(thread, "synthetic compaction user seed")
						accepted, failed := runNativeManualCompaction(t, client, thread)
						if !accepted || failed {
							t.Fatal("native successful compaction was not accepted")
						}
						client.turn(thread, "synthetic postcompaction user")
						wires := capture.snapshot()
						for _, wire := range wires {
							assertCompactionLitePrefix(t, wire)
						}
						business := businessWires(wires)
						if len(business) != 3 {
							t.Fatalf("compaction business request count=%d", len(business))
						}
						assertCompactionWindowLifecycle(t, business, true)
						assertCompactionHistory(t, businessWires(capture.effectiveWires(t)), mode)
						if (business[1].value["previous_response_id"] != nil) != (ws && mode == "v2") {
							t.Fatal("manual compaction baseline eligibility changed")
						}
						if route != "direct" {
							packets := gateway.tap.packets(t)
							if len(packets) != len(wires) {
								t.Fatal("compaction mediation request count changed")
							}
							for i, wire := range wires {
								if route == "api-key" {
									if !bytes.Equal(packets[i].payload, wire.encodedBody) {
										t.Fatal("API-key compaction packet changed")
									}
								} else {
									before := compactionPacketWire(t, packets[i])
									assertCompactionLitePrefix(t, before)
									assertCompactionProjection(t, before, wire)
								}
							}
						}
						t.Logf("compaction actual native lifecycle: mode=%s route=%s ws=%t accepted=true windows=[0,0,1] requests=%d", mode, route, ws, len(wires))
					})
				}
			}
		}
	}
}

func assertCompactionLitePrefix(t *testing.T, wire nativeWire) {
	t.Helper()
	input, _ := wire.value["input"].([]any)
	if len(input) > 0 && input[0].(map[string]any)["type"] == "additional_tools" {
		assertNativeLiteIDs(t, wire)
	}
}

func assertCompactionWindowLifecycle(t *testing.T, wires []nativeWire, accepted bool) {
	t.Helper()
	before, compact, after := compactionMetadata(t, wires[0]), compactionMetadata(t, wires[1]), compactionMetadata(t, wires[2])
	wantAfter := float64(0)
	if accepted {
		wantAfter = 1
	}
	if before["window_number"] != float64(0) || compact["window_number"] != float64(0) || after["window_number"] != wantAfter {
		t.Fatal("compaction committed at incorrect window boundary")
	}
	if before["context_window_id"] != compact["context_window_id"] || (after["context_window_id"] != compact["context_window_id"]) != accepted {
		t.Fatal("context window UUID lifetime changed")
	}
	for i, metadata := range []map[string]any{before, compact, after} {
		id, _ := metadata["context_window_id"].(string)
		if !isUUIDVersion(id, '7') {
			t.Fatal("context window UUID version mismatch")
		}
		if metadata["window_id"] != fmt.Sprintf("%s:%.0f", metadata["thread_id"], metadata["window_number"]) {
			t.Fatal("composite window metadata is inconsistent")
		}
		if wires[i].method == "POST" && wires[i].headers.Get("X-Codex-Window-Id") != metadata["window_id"] {
			t.Fatal("HTTP window header differs from nested window")
		}
		if metadata["thread_id"] != before["thread_id"] || metadata["session_id"] != before["session_id"] {
			t.Fatal("compaction changed thread/session")
		}
	}
	if before["turn_id"] == compact["turn_id"] || compact["turn_id"] == after["turn_id"] {
		t.Fatal("manual compact did not use a separate turn")
	}
}

func compactionItemCounts(wire nativeWire) (map[string]int, map[string]int) {
	types, text := map[string]int{}, map[string]int{}
	input, _ := wire.value["input"].([]any)
	for _, raw := range input {
		item, _ := raw.(map[string]any)
		kind, _ := item["type"].(string)
		types[kind]++
		parts, _ := item["content"].([]any)
		for _, raw := range parts {
			part, _ := raw.(map[string]any)
			value, _ := part["text"].(string)
			text[value]++
		}
	}
	return types, text
}

func assertCompactionHistory(t *testing.T, wires []nativeWire, mode string) {
	t.Helper()
	compactTypes, compactTexts := compactionItemCounts(wires[1])
	afterTypes, afterTexts := compactionItemCounts(wires[2])
	if compactTexts["synthetic compaction user seed"] != 1 || compactTexts["synthetic precompaction assistant"] != 1 {
		t.Fatal("compaction input lacks complete sampled history")
	}
	if afterTexts["synthetic compaction user seed"] != 1 || afterTexts["synthetic postcompaction user"] != 1 || afterTexts["synthetic precompaction assistant"] != 0 {
		t.Fatal("native compacted history did not replace old assistant and retain users")
	}
	if afterTypes["compaction_trigger"] != 0 {
		t.Fatal("transient compaction trigger entered durable history")
	}
	if mode == "v2" {
		if compactTypes["compaction_trigger"] != 1 || afterTypes["compaction"] != 1 {
			t.Fatal("V2 trigger/result contract changed")
		}
		input := wires[1].value["input"].([]any)
		trigger := input[len(input)-1].(map[string]any)
		if trigger["type"] != "compaction_trigger" || trigger["id"] != nil {
			t.Fatal("V2 trigger must be last and have no durable ID")
		}
	} else {
		if compactTypes["compaction_trigger"] != 0 || afterTypes["compaction"] != 0 || compactTexts["synthetic compact instruction"] != 1 {
			t.Fatal("local compact request used remote V2 form")
		}
		var summary bool
		for text, count := range afterTexts {
			if strings.HasSuffix(text, "\nsynthetic compact summary") && count == 1 {
				summary = true
			}
		}
		if !summary {
			t.Fatal("local summary was not installed")
		}
		tools, _ := wires[1].value["tools"].([]any)
		if prefix := compactTypes["additional_tools"]; prefix != 0 {
			input := wires[1].value["input"].([]any)
			tools, _ = input[0].(map[string]any)["tools"].([]any)
		}
		if tools == nil || len(tools) != 0 {
			t.Fatal("local compact must emit an explicit empty tool inventory")
		}
	}
	if wires[2].value["previous_response_id"] != nil {
		t.Fatal("postcompaction replacement history reused the old WS baseline")
	}
}

func compactionPacketWire(t *testing.T, packet nativePacket) nativeWire {
	t.Helper()
	body := packet.payload
	if packet.headers.Get("Content-Encoding") == "zstd" {
		decoder, err := zstd.NewReader(nil, zstd.WithDecoderMaxMemory(nativeCaptureLimit), zstd.WithDecoderConcurrency(1))
		if err != nil {
			t.Fatal("compaction packet decoder")
		}
		defer decoder.Close()
		body, err = decoder.DecodeAll(body, nil)
		if err != nil {
			t.Fatal("compaction packet zstd")
		}
	}
	var value map[string]any
	if json.Unmarshal(body, &value) != nil {
		t.Fatal("compaction native JSON")
	}
	return nativeWire{value: value, body: body, encodedBody: packet.payload, headers: packet.headers, method: packet.method, connection: packet.connection}
}

func assertCompactionProjection(t *testing.T, before, after nativeWire) {
	t.Helper()
	native, emitted := compactionMetadata(t, before), compactionMetadata(t, after)
	for _, field := range []string{"window_number", "request_kind", "compaction"} {
		if !reflect.DeepEqual(native[field], emitted[field]) {
			t.Errorf("compaction semantic metadata differs: %s", field)
		}
	}
	beforeTypes, beforeText := compactionItemCounts(before)
	afterTypes, afterText := compactionItemCounts(after)
	if !reflect.DeepEqual(beforeTypes, afterTypes) || !reflect.DeepEqual(beforeText, afterText) {
		t.Fatal("compaction projection changed ordered context content/types")
	}
	left, _ := before.value["input"].([]any)
	right, _ := after.value["input"].([]any)
	for i, raw := range left {
		original, _ := raw.(map[string]any)
		projected, _ := right[i].(map[string]any)
		for _, field := range []string{"type", "role", "content", "encrypted_content", "tools"} {
			if !reflect.DeepEqual(original[field], projected[field]) {
				t.Errorf("compaction item[%d] changed semantic field %s", i, field)
			}
		}
	}
}
