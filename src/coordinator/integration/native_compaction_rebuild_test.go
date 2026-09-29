//go:build nativeparity

package integration

import (
	"fmt"
	"reflect"
	"strings"
	"testing"
)

// Exercise the real pinned client: compact.rs changes a retained message's
// content without allocating a new ID. Captures stay in memory on loopback.
func TestNativeCompactionRebuiltMessageIDs(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
			for _, media := range []bool{false, true} {
				t.Run(fmt.Sprintf("ws=%t/%s/media=%t", ws, model, media), func(t *testing.T) {
					capture := newNativeCompactionCapture(t, "local", false)
					gateway := newNativeGateway(t, capture.server.URL, true)
					options := nativeOptions{endpoint: gateway.server.URL, bearer: gateway.secret,
						model: model, ws: ws, base: "synthetic compaction rebuild base",
						providerName: "Native Local Capture", neutralPersonality: true,
						configOverrides: map[string]string{"features.token_budget": "false",
							"compact_prompt": `"synthetic compact instruction"`}}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					text := strings.Repeat("word ", 24_000)
					var input []any
					if media {
						text = "keep thirteen"
						input = append(input, map[string]any{"type": "image", "url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="})
					}
					input = append(input, map[string]any{"type": "text", "text": text})
					client.turnWith(thread, map[string]any{"input": input})
					accepted, failed := runNativeManualCompaction(t, client, thread)
					if !accepted || failed {
						t.Fatal("native compaction did not complete")
					}
					client.turn(thread, "first postcompaction user")
					client.turn(thread, "second postcompaction user")

					packets := gateway.tap.packets(t)
					var caller []nativeWire
					for _, packet := range packets {
						caller = append(caller, compactionPacketWire(t, packet))
					}
					caller = businessWires(caller)
					upstream := businessWires(capture.effectiveWires(t))
					if len(caller) != 4 || len(upstream) != 4 {
						t.Fatal("compaction rebuild added or lost a business request")
					}
					var rebuiltContent any
					for _, wires := range [][]nativeWire{caller, upstream} {
						before := compactionUserWithText(t, wires[0], text)
						after := compactionMessageWithID(t, wires[2], before["id"])
						if wires[2].value["previous_response_id"] != nil {
							t.Fatal("rebuilt history reused the old response baseline")
						}
						if reflect.DeepEqual(before["content"], after["content"]) {
							t.Fatal("native fixture did not rebuild the retained message")
						}
						parts, _ := after["content"].([]any)
						if len(parts) != 1 {
							t.Fatal("rebuilt message did not contain exactly one text block")
						}
						part := parts[0].(map[string]any)
						retained, _ := part["text"].(string)
						if part["type"] != "input_text" || (media && retained != text) ||
							(!media && (!strings.Contains(retained, "tokens truncated") || len(retained) >= len(text))) {
							t.Fatal("native media removal or text truncation changed")
						}
						if rebuiltContent != nil && !reflect.DeepEqual(rebuiltContent, after["content"]) {
							t.Fatal("gateway altered the rebuilt caller content")
						}
						rebuiltContent = after["content"]
					}
					before := compactionUserWithText(t, upstream[0], text)
					later := compactionMessageWithID(t, upstream[3], before["id"])
					if !reflect.DeepEqual(later["content"], rebuiltContent) {
						t.Fatal("later continuation revived discarded message content")
					}
				})
			}
		}
	}
}

func compactionUserWithText(t *testing.T, wire nativeWire, text string) map[string]any {
	t.Helper()
	items, _ := wire.value["input"].([]any)
	for _, raw := range items {
		item, _ := raw.(map[string]any)
		if item["role"] != "user" {
			continue
		}
		parts, _ := item["content"].([]any)
		for _, raw := range parts {
			part, _ := raw.(map[string]any)
			if part["type"] == "input_text" && part["text"] == text {
				return item
			}
		}
	}
	t.Fatal("source user message was not captured")
	return nil
}

func compactionMessageWithID(t *testing.T, wire nativeWire, id any) map[string]any {
	t.Helper()
	if value, ok := id.(string); !ok || value == "" {
		t.Fatal("source user message lacks an ID")
	}
	items, _ := wire.value["input"].([]any)
	var found map[string]any
	for _, raw := range items {
		item, _ := raw.(map[string]any)
		if item["id"] == id {
			if found != nil {
				t.Fatal("retained message was duplicated")
			}
			found = item
		}
	}
	if found == nil {
		t.Fatal("retained message ID changed or disappeared")
	}
	return found
}
