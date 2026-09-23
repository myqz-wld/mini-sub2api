//go:build nativeparity

package integration

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"regexp"
	"slices"
	"strings"
	"sync"
	"testing"
)

var callerFieldOrders = struct {
	sync.Mutex
	values map[string][]string
}{values: map[string][]string{}}

// Read ordering from the independently pinned upstream definitions, not gateway code.
// Actual CLI captures calibrate these definitions in caller_wire_capture_test.go.
func callerNativeFieldOrder(t *testing.T, file, marker string) []string {
	t.Helper()
	callerFieldOrders.Lock()
	defer callerFieldOrders.Unlock()
	key := file + "\n" + marker
	if order, ok := callerFieldOrders.values[key]; ok {
		return order
	}
	root := nativeSource(t)
	if exec.Command("git", "-C", root, "diff", "--quiet", "HEAD", "--", filepath.Join("codex-rs", file)).Run() != nil {
		t.Fatal("pinned wire definition has local changes")
	}
	source, err := os.ReadFile(filepath.Join(root, "codex-rs", file))
	if err != nil {
		t.Fatal("read pinned wire definition")
	}
	text := string(source)
	start := strings.Index(text, marker)
	if start < 0 {
		t.Fatal("pinned wire definition missing")
	}
	text = text[start:]
	start = strings.IndexByte(text, '{')
	if start < 0 {
		t.Fatal("pinned wire definition has no body")
	}
	text = text[start+1:]
	end := strings.Index(text, "\n}")
	if strings.HasPrefix(marker, "    ") {
		end = strings.Index(text, "\n    },")
	}
	if end < 0 {
		t.Fatal("pinned wire definition has no terminator")
	}
	field := regexp.MustCompile(`(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:r#)?([a-z][a-z0-9_]*)\s*:`)
	var order []string
	for _, match := range field.FindAllStringSubmatch(text[:end], -1) {
		order = append(order, match[1])
	}
	if len(order) == 0 {
		t.Fatal("empty pinned wire definition")
	}
	callerFieldOrders.values[key] = order
	return order
}

func callerObjectDifferences(node *nativeJSONShape, order, required []string, path string) []string {
	if node == nil || (node.kind != "object" && node.kind != "serialized_object") {
		return []string{path + ":object"}
	}
	var differences, actual, expected []string
	for _, key := range required {
		if node.fields[key] == nil {
			differences = append(differences, path+":missing:"+key)
		}
	}
	for _, key := range node.keys {
		if slices.Contains(order, key) {
			actual = append(actual, key)
		}
	}
	for _, key := range order {
		if node.fields[key] != nil {
			expected = append(expected, key)
		}
	}
	if !slices.Equal(actual, expected) {
		differences = append(differences, path+":order")
	}
	return differences
}

func assertCallerObject(t *testing.T, node *nativeJSONShape, order, required []string, path string) {
	t.Helper()
	for _, difference := range callerObjectDifferences(node, order, required, path) {
		t.Errorf("caller wire differs: %s", difference)
	}
}

func assertCallerWireContract(t *testing.T, incoming nativePacket, wire nativeWire, sent nativePacket) {
	t.Helper()
	caller := readNativeJSONShape(t, nativePacketJSON(t, incoming))
	output := readNativeJSONShape(t, wire.body)
	ws := sent.method == "GET"
	marker := "pub struct ResponsesApiRequest"
	if ws {
		marker = "pub struct ResponseCreateWsRequest"
	}
	order := callerNativeFieldOrder(t, "codex-api/src/common.rs", marker)
	if ws {
		order = append([]string{"type"}, order...)
	}
	required := []string{"model", "input", "tool_choice", "parallel_tool_calls", "reasoning", "store", "stream", "include", "prompt_cache_key", "client_metadata"}
	assertCallerObject(t, output, order, required, "$")
	// Nullable caller controls are an intentional API extension. Test presence/type without
	// coercing null, empty objects and empty arrays to the same representation.
	for _, key := range []string{"reasoning", "text", "tool_choice", "parallel_tool_calls", "service_tier", "access_programs"} {
		before := caller.fields[key]
		if before != nil && before.kind == "null" {
			after := output.fields[key]
			if after == nil || after.kind != "null" {
				t.Errorf("caller null control changed: %s", key)
			}
		}
	}
	for _, key := range []string{"max_output_tokens", "temperature", "top_p", "wire_unknown_probe", "metadata", "prompt_cache_retention", "safety_identifier", "user", "truncation"} {
		if output.fields[key] != nil {
			t.Errorf("unsupported caller field crossed: %s", key)
		}
	}
	if incoming.method == "POST" && output.fields["previous_response_id"] != nil {
		t.Error("HTTP reconstruction retained a response reference")
	}
	if !ws && output.fields["type"] != nil {
		t.Error("HTTP acquired a WS envelope")
	}
	if ws && (output.fields["type"] == nil || wire.value["type"] != "response.create") {
		t.Error("WS envelope missing")
	}
	model, _ := wire.value["model"].(string)
	baseline := callerWireBaselineFor(t, model, ws)
	assertCallerHeaderContract(t, baseline, sent)
	assertCallerRootPresence(t, caller, output, baseline.shape, wire)
	assertCallerMetadataWire(t, output, baseline.shape)
	for _, name := range []string{"reasoning", "text", "stream_options"} {
		node := output.fields[name]
		if node == nil || node.kind != "object" {
			continue
		}
		definition := map[string]string{"reasoning": "pub struct Reasoning", "text": "pub struct TextControls", "stream_options": "pub struct StreamOptions"}[name]
		assertCallerObject(t, node, callerNativeFieldOrder(t, "codex-api/src/common.rs", definition), nil, "$."+name)
		if name == "text" && node.fields["format"] != nil && node.fields["format"].kind == "object" {
			assertCallerObject(t, node.fields["format"], callerNativeFieldOrder(t, "codex-api/src/common.rs", "pub struct TextFormat {"), []string{"type"}, "$.text.format")
		}
	}
	input, _ := wire.value["input"].([]any)
	for index, item := range output.fields["input"].items {
		object, _ := input[index].(map[string]any)
		assertCallerItemWire(t, item, object, fmt.Sprintf("$.input[%d]", index))
	}
	if tools := output.fields["tools"]; tools != nil && tools.kind == "array" {
		values, _ := wire.value["tools"].([]any)
		for i, tool := range tools.items {
			assertCallerToolWire(t, tool, values[i], fmt.Sprintf("$.tools[%d]", i))
		}
	}
}

func assertCallerMetadataWire(t *testing.T, output, baseline *nativeJSONShape) {
	t.Helper()
	metadata, native := output.fields["client_metadata"], baseline.fields["client_metadata"]
	if metadata == nil || native == nil {
		t.Fatal("caller/native client metadata missing")
	}
	// Native uses a randomized HashMap here. Presence and value shape are checked without
	// pretending a single process's random key permutation is a deterministic wire contract.
	for _, name := range []string{"session_id", "thread_id", "turn_id", "root_turn_id", "x-codex-installation-id", "x-codex-window-id"} {
		field := metadata.fields[name]
		if field == nil || field.kind != "string" {
			t.Errorf("caller metadata identity missing or empty: %s", name)
		}
	}
	turn, nativeTurn := metadata.fields["x-codex-turn-metadata"], native.fields["x-codex-turn-metadata"]
	if nativeTurn == nil {
		t.Fatal("native turn metadata reference missing")
	}
	assertCallerObject(t, turn, nativeTurn.keys, []string{"installation_id", "session_id", "thread_id", "turn_id", "root_turn_id", "sandbox", "sandbox_mode", "request_kind", "analytics_enabled"}, "$.client_metadata.x-codex-turn-metadata")
	if turn == nil {
		return
	}
	for _, name := range []string{"analytics_enabled", "auto_review_enabled", "node_repl_auto_review_required", "node_repl_disabled"} {
		field := turn.fields[name]
		if field == nil || (field.kind != "true" && field.kind != "false") {
			t.Errorf("caller turn boolean absent or mistyped: %s", name)
		}
	}
}

func assertCallerRootPresence(t *testing.T, caller, output, baseline *nativeJSONShape, wire nativeWire) {
	t.Helper()
	expected := append([]string(nil), baseline.keys...)
	// WS carries the model format in its create/setup frames, not the HTTP-only Lite header.
	lite := !slices.Contains(baseline.keys, "tools")
	for _, name := range []string{"instructions", "tools", "text", "stream_options", "service_tier", "access_programs", "previous_response_id", "generate"} {
		present := slices.Contains(expected, name)
		switch name {
		case "instructions":
			present = false
			if node := caller.fields[name]; node != nil {
				value, _ := node.scalar.(string)
				present = !lite && strings.TrimSpace(value) != ""
			}
		case "tools":
			present = !lite || (caller.fields[name] != nil && caller.fields[name].kind == "null")
		case "previous_response_id":
			// Full and incremental are separate legal native schemas. Below, removing a
			// reference must be accompanied by complete caller input, never a bare suffix.
			present = wire.method == "GET" && output.fields[name] != nil
		case "generate":
			present = caller.fields[name] != nil
		case "stream_options":
			options := caller.fields[name]
			present = options != nil && options.fields["reasoning_summary_delivery"] != nil && options.fields["reasoning_summary_delivery"].scalar == "sequential_cutoff"
		default:
			present = present || caller.fields[name] != nil
		}
		expected = slices.DeleteFunc(expected, func(key string) bool { return key == name })
		if present {
			expected = append(expected, name)
		}
	}
	// Supported public extensions are caller-owned; they are never silently synthesized.
	for _, name := range []string{"background", "conversation", "context_management", "prompt", "prompt_cache_options", "max_tool_calls", "moderation"} {
		if caller.fields[name] != nil {
			expected = append(expected, name)
		}
	}
	slices.Sort(expected)
	actual := append([]string(nil), output.keys...)
	slices.Sort(actual)
	if !reflect.DeepEqual(expected, actual) {
		t.Errorf("caller top-level field presence differs: expected=%v actual=%v", expected, actual)
	}
	for _, difference := range callerContinuationDifferences(caller, output, lite) {
		t.Errorf("caller continuation differs: %s", difference)
	}
}

func callerContinuationDifferences(caller, output *nativeJSONShape, lite bool) []string {
	input := output.fields["input"]
	if input == nil || input.kind != "array" {
		return []string{"input-array"}
	}
	if previous := output.fields["previous_response_id"]; previous != nil {
		if previous.kind != "string" {
			return []string{"nonempty-reference"}
		}
		return nil
	}
	before := caller.fields["input"]
	if before == nil {
		return []string{"caller-input"}
	}
	want := len(before.items)
	if before.kind == "string" || before.kind == "empty_string" {
		want = 1
	}
	formed := len(before.items) > 0 && before.items[0].fields["type"] != nil && before.items[0].fields["type"].scalar == "additional_tools"
	if lite && !formed {
		want++
		if base := caller.fields["instructions"]; base != nil {
			text, _ := base.scalar.(string)
			if strings.TrimSpace(text) != "" {
				want++
			}
		}
	}
	if caller.fields["previous_response_id"] != nil {
		// A referenced HTTP/reconnected WS request expands saved history. Existing identity
		// and five-turn functional tests prove that history; it must at least retain input.
		if len(input.items) < want {
			return []string{"reconstruction-lost-input"}
		}
	} else if len(input.items) != want {
		return []string{"full-input-length"}
	}
	return nil
}

func assertCallerItemWire(t *testing.T, node *nativeJSONShape, value map[string]any, path string) {
	t.Helper()
	kind, _ := value["type"].(string)
	variants := map[string]string{"message": "Message", "additional_tools": "AdditionalTools", "function_call": "FunctionCall", "function_call_output": "FunctionCallOutput", "reasoning": "Reasoning", "configuration_update": "ConfigurationUpdate", "custom_tool_call": "CustomToolCall", "custom_tool_call_output": "CustomToolCallOutput"}
	variant, ok := variants[kind]
	if !ok {
		t.Fatalf("caller wire item kind requires an explicit oracle: %s", safeNativeShapeKey(kind))
	}
	order := append([]string{"type"}, callerNativeFieldOrder(t, "protocol/src/models.rs", "    "+variant+" {")...)
	required := map[string][]string{"message": {"type", "role", "content"}, "additional_tools": {"type", "role", "tools"}, "function_call": {"type", "name", "arguments", "call_id"}, "function_call_output": {"type", "output"}, "reasoning": {"type", "summary", "encrypted_content"}}[kind]
	assertCallerObject(t, node, order, required, path)
	if metadata := node.fields["internal_chat_message_metadata_passthrough"]; metadata != nil && metadata.kind == "object" {
		assertCallerObject(t, metadata, []string{"turn_id", "create_time", "executed_tool_calls"}, nil, path+".internal_chat_message_metadata_passthrough")
	}
	for _, name := range []string{"content", "summary"} {
		if parts := node.fields[name]; parts != nil && parts.kind == "array" {
			values, _ := value[name].([]any)
			for i, part := range parts.items {
				object, _ := values[i].(map[string]any)
				partKind, _ := object["type"].(string)
				orders := map[string][]string{"input_text": {"type", "text"}, "output_text": {"type", "text", "annotations", "logprobs"}, "summary_text": {"type", "text"}, "reasoning_text": {"type", "text"}}
				if order, ok := orders[partKind]; ok {
					assertCallerObject(t, part, order, []string{"type", "text"}, fmt.Sprintf("%s.%s[%d]", path, name, i))
				}
			}
		}
	}
	if tools := node.fields["tools"]; tools != nil && tools.kind == "array" {
		values, _ := value["tools"].([]any)
		for i, tool := range tools.items {
			assertCallerToolWire(t, tool, values[i], fmt.Sprintf("%s.tools[%d]", path, i))
		}
	}
}

func assertCallerToolWire(t *testing.T, node *nativeJSONShape, value any, path string) {
	t.Helper()
	object, _ := value.(map[string]any)
	kind, _ := object["type"].(string)
	orders := map[string][]string{"function": {"type", "name", "description", "strict", "allowed_callers", "defer_loading", "parameters", "output_schema"}, "namespace": {"type", "name", "description", "tools"}, "custom": {"type", "name", "description", "allowed_callers", "defer_loading", "format"}}
	if order, ok := orders[kind]; ok {
		assertCallerObject(t, node, order, []string{"type", "name", "description"}, path)
	}
	if kind == "function" {
		for _, name := range []string{"strict", "parameters"} {
			if node.fields[name] == nil {
				t.Errorf("tool field missing: %s.%s", path, name)
			}
		}
		assertCallerSchemaWire(t, node.fields["parameters"], path+".parameters")
	}
	if kind == "namespace" {
		children, _ := object["tools"].([]any)
		if node.fields["tools"] == nil {
			t.Fatal("namespace tools missing")
		}
		for i, child := range node.fields["tools"].items {
			assertCallerToolWire(t, child, children[i], fmt.Sprintf("%s.tools[%d]", path, i))
		}
	}
}

func assertCallerSchemaWire(t *testing.T, node *nativeJSONShape, path string) {
	t.Helper()
	if node == nil || node.kind != "object" {
		return
	}
	assertCallerObject(t, node, []string{"$ref", "type", "description", "encrypted", "enum", "items", "properties", "required", "additionalProperties", "anyOf", "oneOf", "allOf", "$defs", "definitions"}, nil, path)
	for _, name := range []string{"properties", "$defs", "definitions"} {
		if properties := node.fields[name]; properties != nil && properties.kind == "object" {
			if !slices.IsSorted(properties.keys) {
				t.Errorf("schema property order differs: %s.%s", path, name)
			}
			for _, key := range properties.keys {
				assertCallerSchemaWire(t, properties.fields[key], path+"."+name+"."+safeNativeShapeKey(key))
			}
		}
	}
	for _, name := range []string{"items", "additionalProperties"} {
		assertCallerSchemaWire(t, node.fields[name], path+"."+name)
	}
	for _, name := range []string{"anyOf", "oneOf", "allOf"} {
		if array := node.fields[name]; array != nil {
			for i, item := range array.items {
				assertCallerSchemaWire(t, item, fmt.Sprintf("%s.%s[%d]", path, name, i))
			}
		}
	}
}

func TestNativeCallerWireOracleRejectsOrderAndMissingFields(t *testing.T) {
	order := []string{"type", "role", "content"}
	for _, raw := range []string{`{"role":"user","type":"message","content":[]}`, `{"type":"message","role":"user"}`} {
		if len(callerObjectDifferences(readNativeJSONShape(t, []byte(raw)), order, order, "$")) == 0 {
			t.Fatal("wire oracle accepted a mutation")
		}
	}
	caller := readNativeJSONShape(t, []byte(`{"input":[{}, {}, {}]}`))
	missing := readNativeJSONShape(t, []byte(`{"input":[{}]}`))
	if len(callerContinuationDifferences(caller, missing, false)) == 0 {
		t.Fatal("wire oracle accepted a suffix without its response reference")
	}
	for _, raw := range []string{`{"input":[{}],"previous_response_id":null}`, `{"input":[{}],"previous_response_id":""}`} {
		if len(callerContinuationDifferences(caller, readNativeJSONShape(t, []byte(raw)), false)) == 0 {
			t.Fatal("wire oracle accepted an empty/null response reference")
		}
	}
}
