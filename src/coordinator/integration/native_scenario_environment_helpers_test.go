//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"reflect"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"
)

func environmentCallerValues(t *testing.T, gateway nativeGateway) []map[string]any {
	t.Helper()
	var out []map[string]any
	for _, packet := range gateway.tap.packets(t) {
		body := packet.payload
		if packet.headers.Get("Content-Encoding") == "zstd" {
			decoder, err := zstd.NewReader(nil, zstd.WithDecoderMaxMemory(nativeCaptureLimit), zstd.WithDecoderConcurrency(1))
			if err != nil {
				t.Fatal("environment capture decoder")
			}
			body, err = decoder.DecodeAll(body, nil)
			decoder.Close()
			if err != nil {
				t.Fatal("environment capture decompression")
			}
		}
		var value map[string]any
		if json.Unmarshal(body, &value) != nil {
			t.Fatal("environment capture JSON")
		}
		out = append(out, value)
	}
	return out
}

func environmentFirstContext(t *testing.T, gateway nativeGateway) map[string]any {
	t.Helper()
	for _, value := range environmentCallerValues(t, gateway) {
		if environmentCount(value, "developer", envDeveloper) > 0 {
			return value
		}
	}
	t.Fatal("native initial context not captured")
	return nil
}

func environmentMessages(value map[string]any, role string) []map[string]any {
	var out []map[string]any
	input, _ := value["input"].([]any)
	for _, raw := range input {
		item, _ := raw.(map[string]any)
		if item["type"] == "message" && item["role"] == role {
			out = append(out, item)
		}
	}
	return out
}

func environmentTexts(item map[string]any) []string {
	var texts []string
	content, _ := item["content"].([]any)
	for _, raw := range content {
		block, _ := raw.(map[string]any)
		if text, ok := block["text"].(string); ok {
			texts = append(texts, text)
		}
	}
	return texts
}

func environmentCount(value map[string]any, role, marker string) int {
	count := 0
	for _, item := range environmentMessages(value, role) {
		for _, text := range environmentTexts(item) {
			count += strings.Count(text, marker)
		}
	}
	return count
}

func environmentAssertCount(t *testing.T, value map[string]any, role, marker string, want int) {
	t.Helper()
	if got := environmentCount(value, role, marker); got != want {
		// Markers can include ephemeral paths; failures report counts and role, never payload text.
		t.Fatalf("native environment marker count differs: role=%s got=%d want=%d", role, got, want)
	}
}

func environmentAssertGroupedKinds(t *testing.T, value map[string]any, role string, expected ...string) {
	t.Helper()
	for _, item := range environmentMessages(value, role) {
		metadata, _ := item["internal_chat_message_metadata_passthrough"].(map[string]any)
		kinds, _ := metadata["content_item_kinds"].([]any)
		content, _ := item["content"].([]any)
		if len(kinds) != len(content) {
			continue
		}
		found := 0
		for _, kind := range kinds {
			if found < len(expected) && kind == expected[found] {
				found++
			}
		}
		if found == len(expected) {
			return
		}
	}
	t.Fatalf("native %s fragments are not grouped in expected content-element order", role)
}

func environmentAssertParity(t *testing.T, gateway nativeGateway, upstream *nativeCapture, subscription bool) {
	t.Helper()
	packets := gateway.tap.packets(t)
	caller := environmentCallerValues(t, gateway)
	wires := upstream.snapshot()
	if len(caller) != len(wires) {
		t.Fatal("environment gateway request count changed")
	}
	for index, before := range caller {
		wire := wires[index]
		if !subscription {
			if !bytes.Equal(packets[index].payload, wire.encodedBody) {
				t.Fatal("environment API-key payload changed")
			}
			continue
		}
		assertNativeMessageParity(t, packets[index], wire)
		input, _ := before["input"].([]any)
		emitted, _ := wire.value["input"].([]any)
		if len(input) != len(emitted) {
			t.Fatal("environment item count changed")
		}
		for position, raw := range input {
			original, _ := raw.(map[string]any)
			projected, _ := emitted[position].(map[string]any)
			metadata, _ := original["internal_chat_message_metadata_passthrough"].(map[string]any)
			actual, _ := projected["internal_chat_message_metadata_passthrough"].(map[string]any)
			for key, expected := range metadata {
				if key == "turn_id" {
					continue
				} // Scoped lifecycle identity is covered by the identity scenarios.
				if !reflect.DeepEqual(expected, actual[key]) {
					t.Fatalf("environment item metadata changed: %s", key)
				}
			}
		}
	}
}

func environmentResolvedSkill(t *testing.T, client *nativeClient, cwd, name string) string {
	t.Helper()
	result := client.call("skills/list", map[string]any{"cwds": []any{cwd}, "forceReload": true})
	entries, _ := result["data"].([]any)
	for _, raw := range entries {
		entry, _ := raw.(map[string]any)
		skills, _ := entry["skills"].([]any)
		for _, raw := range skills {
			skill, _ := raw.(map[string]any)
			if skill["name"] == name {
				path, _ := skill["path"].(string)
				if path == "" {
					t.Fatal("native listed skill lacks selectable path")
				}
				return path
			}
		}
	}
	t.Fatal("native did not list synthetic skill")
	return ""
}
