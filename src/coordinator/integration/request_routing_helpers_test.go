package integration

import (
	"bytes"
	"encoding/json"
	"testing"
)

func canonicalExpectedTools(tools []any) []any {
	canonical := make([]any, 0, len(tools))
	for _, tool := range tools {
		object := tool.(map[string]any)
		copy := make(map[string]any, len(object)+1)
		for name, value := range object {
			copy[name] = value
		}
		if copy["type"] == "function" {
			if _, ok := copy["strict"]; !ok {
				copy["strict"] = false
			}
		}
		canonical = append(canonical, copy)
	}
	return canonical
}

func canonicalExpectedLiteTools(tools []any) []any {
	functions := make([]any, 0)
	grouped := make([]any, 0, len(tools))
	functionIndex := -1
	for _, tool := range canonicalExpectedTools(tools) {
		object := tool.(map[string]any)
		if object["type"] == "function" || object["type"] == "custom" {
			if functionIndex == -1 {
				functionIndex = len(grouped)
			}
			functions = append(functions, object)
			continue
		}
		grouped = append(grouped, object)
	}
	if functionIndex >= 0 {
		namespace := map[string]any{
			"type": "namespace", "name": "functions", "description": "", "tools": functions,
		}
		grouped = append(grouped, nil)
		copy(grouped[functionIndex+1:], grouped[functionIndex:])
		grouped[functionIndex] = namespace
	}
	return grouped
}

func mustRequestJSON(t *testing.T, value any) []byte {
	t.Helper()
	encoded, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	return encoded
}

func decodeRequestObject(t *testing.T, body []byte) map[string]any {
	t.Helper()
	var value map[string]any
	if err := json.Unmarshal(body, &value); err != nil {
		t.Fatalf("decode upstream request: %v; body=%s", err, body)
	}
	return value
}

func assertAPIKeyCapture(t *testing.T, capture routingMatrixCapture, wantBody []byte) {
	t.Helper()
	if capture.Headers.Get("Authorization") != "Bearer "+upstreamAPIKey {
		t.Fatalf("API-key authorization = %q", capture.Headers.Get("Authorization"))
	}
	if capture.Headers.Get("ChatGPT-Account-ID") != "" {
		t.Fatalf("API-key route received subscription account header: %#v", capture.Headers)
	}
	if !bytes.Equal(capture.Body, wantBody) {
		t.Fatal("API-key body changed")
	}
}

func assertCodexOpenAIProfileCapture(t *testing.T, capture routingMatrixCapture) {
	t.Helper()
	if capture.Headers.Get("Authorization") != "Bearer "+upstreamAPIKey ||
		capture.Headers.Get("ChatGPT-Account-ID") != "" || capture.Headers.Get("Content-Encoding") != "" ||
		capture.Headers.Get("X-Codex-Routing-Hint") != "" {
		t.Fatal("Codex API-key profile crossed a subscription credential boundary")
	}
}

func assertSubscriptionCapture(
	t *testing.T,
	capture routingMatrixCapture,
	wantAccessToken, wantAccountID string,
) {
	t.Helper()
	if capture.Headers.Get("Authorization") != "Bearer "+wantAccessToken ||
		capture.Headers.Get("ChatGPT-Account-ID") != wantAccountID {
		t.Fatalf("subscription authorization headers = %#v", capture.Headers)
	}
	if capture.Headers.Get("Originator") != "codex_cli_rs" ||
		capture.Headers.Get("Version") != "0.149.0" ||
		capture.Headers.Get("User-Agent") != canonicalSubscriptionUserAgent {
		t.Fatalf("subscription identity headers = %#v", capture.Headers)
	}
	if capture.Headers.Get("OpenAI-Organization") != "" ||
		capture.Headers.Get("OpenAI-Project") != "" ||
		capture.Headers.Get("X-Stainless-Lang") != "" {
		t.Fatalf("API-only headers crossed into subscription route: %#v", capture.Headers)
	}
	if capture.Headers.Get("Content-Encoding") != "zstd" ||
		capture.Headers.Get("Accept") != "text/event-stream" ||
		capture.Headers.Get("Content-Type") != "application/json" {
		t.Fatalf("subscription representation headers = %#v", capture.Headers)
	}
	if capture.Headers.Get("X-Codex-Installation-Id") != "" {
		t.Fatalf("installation id must not be a direct HTTP header: %#v", capture.Headers)
	}
	if capture.Headers.Get("X-Codex-Routing-Hint") == "" {
		t.Fatalf("subscription routing hint is missing: %#v", capture.Headers)
	}
}

func assertLiteSubscriptionBody(t *testing.T, body []byte, messages, tools []any) {
	t.Helper()
	value := decodeRequestObject(t, body)
	input, ok := value["input"].([]any)
	if !ok || len(input) != len(messages)+2 {
		t.Fatalf("lite input = %#v", value["input"])
	}
	additional, ok := input[0].(map[string]any)
	if !ok || additional["type"] != "additional_tools" ||
		!jsonEqual(additional["tools"], canonicalExpectedLiteTools(tools)) {
		t.Fatalf("lite additional tools = %#v", input[0])
	}
	developer, ok := input[1].(map[string]any)
	if !ok || developer["role"] != "developer" {
		t.Fatalf("lite developer message = %#v", input[1])
	}
	assertNormalizedMessages(t, input[2:], messages)
	if additional["id"] != nil || developer["id"] != nil {
		t.Fatalf("synthetic lite items received ids: additional=%#v developer=%#v", additional, developer)
	}
	if value["tools"] != nil || value["instructions"] != nil || value["store"] != false ||
		value["stream"] != false || value["parallel_tool_calls"] != false {
		t.Fatalf("lite controls = %#v", value)
	}
}

func assertNonLiteSubscriptionBody(t *testing.T, body []byte, messages, tools []any) {
	t.Helper()
	value := decodeRequestObject(t, body)
	input, ok := value["input"].([]any)
	if !ok {
		t.Fatalf("non-lite input = %#v", value["input"])
	}
	assertNormalizedMessages(t, input, messages)
	if !jsonEqual(value["tools"], canonicalExpectedTools(tools)) {
		t.Fatalf("non-lite messages/tools = %#v", value)
	}
	if value["instructions"] != "Look up the requested order." || value["store"] != false ||
		value["stream"] != false || value["parallel_tool_calls"] != true ||
		value["tool_choice"] != "auto" {
		t.Fatalf("non-lite controls = %#v", value)
	}
	if !jsonEqual(value["include"], []any{"reasoning.encrypted_content"}) {
		t.Fatalf("non-lite include = %#v", value["include"])
	}
}

func assertNativeSubscriptionBody(t *testing.T, body []byte, messages []any) {
	t.Helper()
	value := decodeRequestObject(t, body)
	input, ok := value["input"].([]any)
	if !ok {
		t.Fatalf("native subscription input = %#v", value["input"])
	}
	assertMessageSemantics(t, input, messages)
	if value["model"] != "gpt-5.4" || value["instructions"] != "Continue the existing turn." ||
		value["store"] != false || value["stream"] != false || value["tool_choice"] != "auto" ||
		value["parallel_tool_calls"] != true || !isUUIDv8(value["prompt_cache_key"]) {
		t.Fatalf("native subscription controls = %#v", value)
	}
	metadata, ok := value["client_metadata"].(map[string]any)
	if !ok || !isUUIDv8(metadata["session_id"]) || !isUUIDv8(metadata["thread_id"]) ||
		!isUUIDv8(metadata["turn_id"]) || !isUUIDv8(metadata["x-codex-installation-id"]) ||
		metadata["x-codex-turn-metadata"] == "" {
		t.Fatalf("native subscription metadata = %#v", value["client_metadata"])
	}
}

func isUUIDv8(value any) bool {
	text, ok := value.(string)
	if !ok || len(text) != 36 || text[8] != '-' || text[13] != '-' || text[14] != '8' ||
		text[18] != '-' || text[23] != '-' {
		return false
	}
	for index, character := range text {
		if index == 8 || index == 13 || index == 18 || index == 23 {
			continue
		}
		if !(character >= '0' && character <= '9') && !(character >= 'a' && character <= 'f') {
			return false
		}
	}
	return text[19] == '8' || text[19] == '9' || text[19] == 'a' || text[19] == 'b'
}

func assertNormalizedMessages(t *testing.T, got, want []any) {
	t.Helper()
	assertMessageSemantics(t, got, want)
	for index := range want {
		gotMessage := got[index].(map[string]any)
		id, _ := gotMessage["id"].(string)
		metadata, _ := gotMessage["internal_chat_message_metadata_passthrough"].(map[string]any)
		role, _ := gotMessage["role"].(string)
		wantCreateTime := role == "user" || role == "system" || role == "developer"
		hasCreateTime := metadata["create_time"] != nil
		if id == "" || metadata["turn_id"] == "" || hasCreateTime != wantCreateTime {
			t.Fatalf("normalized message %d identity = %#v", index, gotMessage)
		}
	}
}

func assertMessageSemantics(t *testing.T, got, want []any) {
	t.Helper()
	if len(got) != len(want) {
		t.Fatalf("normalized message count = %d, want %d", len(got), len(want))
	}
	for index := range want {
		gotMessage, gotOK := got[index].(map[string]any)
		wantMessage, wantOK := want[index].(map[string]any)
		if !gotOK || !wantOK || gotMessage["type"] != wantMessage["type"] ||
			gotMessage["role"] != wantMessage["role"] || !jsonEqual(gotMessage["content"], wantMessage["content"]) {
			t.Fatalf("normalized message %d semantic mismatch", index)
		}
	}
}
