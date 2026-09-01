package integration

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"
)

func writeLoopbackResponsesResult(writer http.ResponseWriter, body []byte, responseID string) {
	var request map[string]any
	if err := json.Unmarshal(body, &request); err != nil {
		http.Error(writer, "invalid captured request", http.StatusInternalServerError)
		return
	}
	response := fmt.Sprintf(
		`{"id":%q,"object":"response","usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}`,
		responseID,
	)
	if request["stream"] == true {
		writer.Header().Set("Content-Type", "text/event-stream")
		_, _ = fmt.Fprintf(writer, "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":%s}\n\n", response)
		return
	}
	writer.Header().Set("Content-Type", "application/json")
	_, _ = io.WriteString(writer, response)
}

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
	if capture.Headers.Get("Originator") != "codex-tui" ||
		capture.Headers.Get("Version") != "0.149.0" ||
		capture.Headers.Get("Accept") != "text/event-stream" {
		t.Fatalf("Codex API-key identity headers = %#v", capture.Headers)
	}
	assertRuntimeCodexUserAgent(t, capture.Headers.Get("User-Agent"))
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
	if capture.Headers.Get("Originator") != "codex-tui" ||
		capture.Headers.Get("Version") != "0.149.0" {
		t.Fatalf("subscription identity headers = %#v", capture.Headers)
	}
	assertRuntimeCodexUserAgent(t, capture.Headers.Get("User-Agent"))
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

func assertRuntimeCodexUserAgent(t *testing.T, value string) {
	t.Helper()
	if !strings.HasPrefix(value, "codex-tui/0.149.0 (") ||
		!strings.Contains(value, "; ") || !strings.Contains(value, ") ") ||
		!strings.HasSuffix(value, " (codex-tui; 0.149.0)") {
		t.Fatalf("runtime Codex User-Agent = %q", value)
	}
}

func assertCodexBaseInstructions(t *testing.T, value any, model string) {
	t.Helper()
	instructions, ok := value.(string)
	if !ok {
		t.Fatalf("Codex base instructions are not text for model %q", model)
	}
	got := fmt.Sprintf("%x", sha256.Sum256([]byte(instructions)))
	if want := expectedCodexInstructionsHash(model); got != want {
		t.Fatalf("Codex base instructions hash for %q = %s, want %s", model, got, want)
	}
}

func assertCodexBaseDeveloperMessage(t *testing.T, input any, model string) {
	t.Helper()
	items, ok := input.([]any)
	if !ok {
		t.Fatalf("Codex input is not an array for model %q", model)
	}
	matches := 0
	for _, item := range items {
		if text, ok := developerMessageText(item); ok {
			hash := fmt.Sprintf("%x", sha256.Sum256([]byte(text)))
			if hash == expectedCodexInstructionsHash(model) {
				matches++
			}
		}
	}
	if matches != 1 {
		t.Fatalf("Codex base developer message count for %q = %d, want 1", model, matches)
	}
}

func assertDeveloperMessageText(t *testing.T, item any, want string) {
	t.Helper()
	got, ok := developerMessageText(item)
	if !ok || got != want {
		t.Fatalf("developer message text = %q, want %q", got, want)
	}
}

func developerMessageText(item any) (string, bool) {
	message, ok := item.(map[string]any)
	if !ok || message["type"] != "message" || message["role"] != "developer" {
		return "", false
	}
	content, ok := message["content"].([]any)
	if !ok || len(content) != 1 {
		return "", false
	}
	part, ok := content[0].(map[string]any)
	if !ok || part["type"] != "input_text" {
		return "", false
	}
	text, ok := part["text"].(string)
	return text, ok
}

func expectedCodexInstructionsHash(model string) string {
	if slash := strings.IndexByte(model, '/'); slash >= 0 {
		namespace, suffix := model[:slash], model[slash+1:]
		validNamespace := namespace != "" && !strings.Contains(suffix, "/")
		for _, character := range namespace {
			validNamespace = validNamespace && (character >= 'a' && character <= 'z' ||
				character >= 'A' && character <= 'Z' || character >= '0' && character <= '9' ||
				character == '_' || character == '-')
		}
		if validNamespace {
			model = suffix
		}
	}
	switch {
	case strings.HasPrefix(model, "gpt-5.6-sol"),
		strings.HasPrefix(model, "gpt-5.6-terra"),
		strings.HasPrefix(model, "gpt-5.6-luna"):
		return "cbefa6b0bede0e332d957fca70ccacf9f12f4c0ecdf81b819e5cbe1a3b16e265"
	case strings.HasPrefix(model, "gpt-5.5"):
		return "e58c21f9377e946e2e10f886fcbf6f030e1c6fd9067241c637a56e9e998d3c31"
	case strings.HasPrefix(model, "gpt-5.4-mini"):
		return "9109777dc7f3bc9ee9a0d187982b13538c53e0572de2959300f7226e9c59855e"
	case strings.HasPrefix(model, "gpt-5.4"), strings.HasPrefix(model, "codex-auto-review"):
		return "9721f7a86edc261996e628fe14fade8d66ec60e6cc727274a8da6a03e15464de"
	case strings.HasPrefix(model, "gpt-5.2"):
		return "c9b2fa097ac69cae82c3d2ae12271083890a96521c55ad8dc14cae5168ad3f39"
	case model == "exp-codex-personality":
		return "4cf5dd6317a9920b3f0398f6fa7ca49310b57961f6dd076eb2141acd4f963843"
	default:
		return "ac8ae107a0d72fe3476b430afb161ea4e67da2e446d778aefc44828160559807"
	}
}

func assertLiteSubscriptionBody(t *testing.T, body []byte, messages, tools []any) {
	t.Helper()
	value := decodeRequestObject(t, body)
	input, ok := value["input"].([]any)
	if !ok || len(input) != len(messages)+3 {
		t.Fatalf("lite input count = %d, want %d", len(input), len(messages)+3)
	}
	additional, ok := input[0].(map[string]any)
	if !ok || additional["type"] != "additional_tools" ||
		!jsonEqual(additional["tools"], canonicalExpectedLiteTools(tools)) {
		t.Fatalf("lite additional tools = %#v", input[0])
	}
	assertCodexBaseDeveloperMessage(t, input, "gpt-5.6-sol")
	baseText, baseOK := developerMessageText(input[1])
	if !baseOK {
		t.Fatal("Lite canonical base is not the first developer message after tools")
	}
	assertCodexBaseInstructions(t, baseText, "gpt-5.6-sol")
	assertDeveloperMessageText(t, input[2], "Answer both user messages.")
	assertNormalizedMessages(t, input[3:], messages)
	base := input[1].(map[string]any)
	custom := input[2].(map[string]any)
	if additional["id"] != nil || base["id"] != nil || custom["id"] != nil {
		t.Fatal("synthetic Lite tools/base/custom items received ids")
	}
	if value["tools"] != nil || value["instructions"] != nil || value["store"] != false ||
		value["stream"] != true || value["parallel_tool_calls"] != false {
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
	if len(input) != len(messages)+1 {
		t.Fatalf("non-lite input count = %d, want %d", len(input), len(messages)+1)
	}
	assertDeveloperMessageText(t, input[0], "Look up the requested order.")
	assertNormalizedMessages(t, input[1:], messages)
	if !jsonEqual(value["tools"], canonicalExpectedTools(tools)) {
		t.Fatalf("non-lite messages/tools = %#v", value)
	}
	assertCodexBaseInstructions(t, value["instructions"], "gpt-5.4-mini")
	if value["store"] != false ||
		value["stream"] != true || value["parallel_tool_calls"] != true ||
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
	if len(input) != len(messages)+1 {
		t.Fatalf("native subscription input count = %d, want %d", len(input), len(messages)+1)
	}
	assertDeveloperMessageText(t, input[0], "Continue the existing turn.")
	assertMessageSemantics(t, input[1:], messages)
	assertCodexBaseInstructions(t, value["instructions"], "gpt-5.4")
	if value["model"] != "gpt-5.4" ||
		value["store"] != false || value["stream"] != true || value["tool_choice"] != "auto" ||
		value["parallel_tool_calls"] != true || !isUUIDVersion(value["prompt_cache_key"], '7') {
		t.Fatalf("native subscription controls = %#v", value)
	}
	metadata, ok := value["client_metadata"].(map[string]any)
	if !ok || !isUUIDVersion(metadata["session_id"], '7') || !isUUIDVersion(metadata["thread_id"], '7') ||
		!isUUIDVersion(metadata["turn_id"], '7') || !isUUIDVersion(metadata["x-codex-installation-id"], '4') ||
		metadata["x-codex-turn-metadata"] == "" {
		t.Fatalf("native subscription metadata = %#v", value["client_metadata"])
	}
}

func isUUIDVersion(value any, version byte) bool {
	text, ok := value.(string)
	if !ok || len(text) != 36 || text[8] != '-' || text[13] != '-' || text[14] != version ||
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
