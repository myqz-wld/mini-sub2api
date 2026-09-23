//go:build nativeparity

package integration

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

func capturedBase(t *testing.T, wire nativeWire) string {
	t.Helper()
	input, _ := wire.value["input"].([]any)
	if len(input) > 1 {
		first, _ := input[0].(map[string]any)
		if first["type"] == "additional_tools" {
			base, _ := input[1].(map[string]any)
			content, _ := base["content"].([]any)
			if len(content) == 1 {
				part, _ := content[0].(map[string]any)
				if text, ok := part["text"].(string); ok {
					return text
				}
			}
		}
	}
	if base, ok := wire.value["instructions"].(string); ok && base != "" {
		return base
	}
	t.Fatal("full captured base instructions missing")
	return ""
}

func TestNativeAllCatalogModelDefaults(t *testing.T) {
	data, err := os.ReadFile(filepath.Join(nativeSource(t), "codex-rs", "models-manager", "models.json"))
	if err != nil {
		t.Fatal("read pinned catalog")
	}
	var catalog struct {
		Models []struct {
			Slug string `json:"slug"`
		} `json:"models"`
	}
	if json.Unmarshal(data, &catalog) != nil || len(catalog.Models) != 9 {
		t.Fatal("pinned catalog shape")
	}
	for _, model := range catalog.Models {
		t.Run(model.Slug, func(t *testing.T) {
			direct := newNativeCapture(t)
			options := nativeOptions{endpoint: direct.server.URL, model: model.Slug, neutralPersonality: true}
			native := startNativeClient(t, options)
			thread := native.thread(options)
			native.turn(thread, "synthetic default probe")
			actual := direct.snapshot()
			if len(actual) < 1 {
				t.Fatal("native default request missing")
			}
			expected := capturedBase(t, actual[0])
			upstream := newNativeCapture(t)
			gateway := newNativeGateway(t, upstream.server.URL, true)
			client := ordinaryClient(t, gateway, false, false, nil)
			// Native defaults remain caller-owned: preserve them only when explicitly supplied.
			client.send(map[string]any{"model": model.Slug, "input": "synthetic default probe", "instructions": expected})
			emitted := upstream.snapshot()
			if len(emitted) != 1 {
				t.Fatal("ordinary default count")
			}
			if capturedBase(t, emitted[0]) != expected {
				t.Fatal("explicit native catalog base changed")
			}
			for _, wire := range []nativeWire{actual[0], emitted[0]} {
				input, _ := wire.value["input"].([]any)
				first, _ := input[0].(map[string]any)
				if first["type"] == "additional_tools" {
					assertNativeLiteIDs(t, wire)
				}
			}
			// Settings may include extra native runtime defaults; compare the model-owned fields.
			for _, field := range []string{"reasoning", "text"} {
				a, _ := json.Marshal(actual[0].value[field])
				b, _ := json.Marshal(emitted[0].value[field])
				if string(a) != string(b) {
					t.Errorf("model default field differs: %s", field)
				}
			}
			client.send(map[string]any{"model": model.Slug, "input": "synthetic omitted base probe"})
			emitted = upstream.snapshot()
			if len(emitted) != 2 {
				t.Fatal("omitted catalog base request count changed")
			}
			missing := emitted[1]
			if _, present := missing.value["instructions"]; present {
				t.Fatal("catalog base was inserted for an ordinary caller")
			}
			items, _ := missing.value["input"].([]any)
			if len(items) > 0 && items[0].(map[string]any)["type"] == "additional_tools" {
				assertScenarioBaseValue(t, missing, true, "")
				items = items[1:]
			}
			if len(items) != 1 || items[0].(map[string]any)["role"] != "user" {
				t.Fatal("omitted catalog base acquired an instruction message")
			}
		})
	}
}

func TestNativeLitePrefixContentAndThreadDimensions(t *testing.T) {
	for _, mediated := range []bool{false, true} {
		t.Run(fmt.Sprintf("gateway=%t", mediated), func(t *testing.T) {
			capture := newNativeCapture(t)
			endpoint := capture.server.URL
			bearer := ""
			if mediated {
				gateway := newNativeGateway(t, endpoint, true)
				endpoint = gateway.server.URL
				bearer = gateway.secret
			}
			options := nativeOptions{endpoint: endpoint, bearer: bearer, model: "gpt-5.6-sol", base: "  基础 {{literal}}\n"}
			client := startNativeClient(t, options)
			thread := client.thread(options)
			client.turn(thread, "first prefix probe")
			client.turn(thread, "repeat prefix probe")
			initial := capture.snapshot()
			if len(initial) != 3 {
				t.Fatal("native repeated prefix capture count")
			}
			for _, wire := range initial {
				assertNativeLiteIDs(t, wire)
			}
			prefixID := func(w nativeWire, index int) any { return w.value["input"].([]any)[index].(map[string]any)["id"] }
			for _, wire := range initial[1:] {
				for _, i := range []int{0, 1} {
					if prefixID(wire, i) != prefixID(initial[0], i) {
						t.Fatal("same thread/payload changed deterministic ID")
					}
				}
			}
			// A distinct native thread must produce distinct IDs even for identical tools/base.
			other := client.thread(options)
			client.turn(other, "separate native thread")
			wires := capture.snapshot()
			last := wires[len(wires)-1]
			assertNativeLiteIDs(t, last)
			for _, i := range []int{0, 1} {
				if prefixID(last, i) == prefixID(initial[0], i) {
					t.Fatal("native thread did not namespace prefix ID")
				}
			}
			// v0.156.0 requires a persisted source rollout for fork/resume. Assert this
			// boundary explicitly; native ephemeral tests never create transcript fixtures.
			client.callExpect("thread/fork", map[string]any{"threadId": thread, "ephemeral": true}, -32600)
			options.base = "基础 {{literal}}\n"
			options.tools = []any{
				map[string]any{"type": "function", "name": "native_probe", "description": "Synthetic changed tool", "inputSchema": map[string]any{"type": "object", "properties": map[string]any{}}},
				map[string]any{"type": "function", "name": "second_probe", "description": "Second synthetic tool", "inputSchema": map[string]any{"type": "object", "properties": map[string]any{}}},
			}
			changed := client.thread(options)
			client.turn(changed, "changed prefix probe")
			wires = capture.snapshot()
			last = wires[len(wires)-1]
			assertNativeLiteIDs(t, last)
			if capturedBase(t, last) != options.base {
				t.Fatal("native changed base lost")
			}
			options.tools[0], options.tools[1] = options.tools[1], options.tools[0]
			reordered := client.thread(options)
			client.turn(reordered, "reordered tool prefix probe")
			wires = capture.snapshot()
			assertNativeLiteIDs(t, wires[len(wires)-1])
		})
	}
}

func TestNativeOrdinaryOmittedSetupDoesNotRestoreBase(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, format := range []string{"ordinary", "converted-lite"} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, format), func(t *testing.T) {
				capture := newNativeCapture(t)
				gateway := newNativeGateway(t, capture.server.URL, true)
				client := ordinaryClient(t, gateway, ws, !ws, nil)
				first := ordinaryRequest(format, "session")
				result := client.send(first)
				output := result["output"].([]any)[0].(map[string]any)
				client.send(map[string]any{"model": first["model"], "previous_response_id": result["id"], "input": []any{map[string]any{"type": "function_call_output", "call_id": output["call_id"], "output": "result"}}})
				wires := businessWires(capture.snapshot())
				if len(wires) != 2 {
					t.Fatal("omitted setup count")
				}
				if format == "converted-lite" && wires[1].value["previous_response_id"] != nil {
					t.Fatal("changed Lite setup reused stale prefix")
				}
				if _, present := wires[1].value["instructions"]; present {
					t.Fatal("ordinary omitted base incorrectly restored instructions")
				}
				if format == "converted-lite" {
					assertScenarioBaseValue(t, wires[1], true, "")
				}
				developers := []string{}
				for _, item := range wires[1].value["input"].([]any) {
					if text, ok := developerMessageText(item); ok {
						developers = append(developers, text)
					}
				}
				want := []string{}
				if wires[1].value["previous_response_id"] == nil {
					want = []string{"system fixture", "duplicate developer", "duplicate developer"}
				}
				if !reflect.DeepEqual(developers, want) {
					t.Fatal("ordinary reconstruction invented or removed developer instructions")
				}
			})
		}
	}
}
