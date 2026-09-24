package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync/atomic"
	"testing"
)

var loopbackResponseSequence atomic.Uint64

func writeLoopbackResponsesResult(
	writer http.ResponseWriter,
	body []byte,
	responseID string,
) (string, string) {
	var request map[string]any
	if err := json.Unmarshal(body, &request); err != nil {
		http.Error(writer, "invalid captured request", http.StatusInternalServerError)
		return "", ""
	}
	responseID = fmt.Sprintf("%s_%d", responseID, loopbackResponseSequence.Add(1))
	providerRequestID := "provider-" + responseID
	writer.Header().Set("X-Request-Id", providerRequestID)
	writer.Header().Set("Openai-Request-Id", "secondary-"+providerRequestID)
	writer.Header().Set("Openai-Model", "gpt-loopback")
	writer.Header().Set("Server-Timing", "provider;dur=2")
	writer.Header().Set("X-Codex-Installation-Id", "must-not-cross")
	writer.Header().Set("X-Unrecognized-Provider-Extension", "must-not-cross")
	if request["model"] == "profile-non2xx" {
		writer.Header().Set("Content-Type", "text/plain")
		writer.WriteHeader(http.StatusTooManyRequests)
		_, _ = fmt.Fprintf(
			writer,
			"raw provider response_id=%s conversation=conv_private request=request_private",
			responseID,
		)
		return responseID, providerRequestID
	}
	terminalType := "response.completed"
	if request["model"] == "response.failed" || request["model"] == "response.incomplete" {
		terminalType = request["model"].(string)
	}
	errorObject := ""
	if terminalType != "response.completed" {
		errorObject = `,"error":{"message":"opaque resp_raw conversation_raw","id":"opaque_nested_id"}`
	}
	response := fmt.Sprintf(
		`{"id":%q,"object":"response","usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}%s}`,
		responseID, errorObject,
	)
	if request["stream"] == true {
		writer.Header().Set("Content-Type", "text/event-stream")
		_, _ = fmt.Fprintf(writer, "event: %s\ndata: {\"type\":%q,\"response\":%s}\n\n", terminalType, terminalType, response)
		return responseID, providerRequestID
	}
	writer.Header().Set("Content-Type", "application/json")
	_, _ = io.WriteString(writer, response)
	return responseID, providerRequestID
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
			if schema, ok := copy["parameters"].(map[string]any); ok && schema["type"] == "object" {
				cloned := make(map[string]any, len(schema)+1)
				for key, value := range schema {
					cloned[key] = value
				}
				if _, ok := cloned["properties"]; !ok {
					cloned["properties"] = map[string]any{}
				}
				copy["parameters"] = cloned
			}

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
		capture.Headers.Get("Version") != "0.156.0" {
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
	if !strings.HasPrefix(value, "codex-tui/0.156.0 (") ||
		!strings.Contains(value, "; ") || !strings.Contains(value, ") ") ||
		!strings.HasSuffix(value, " (codex-tui; 0.156.0)") {
		t.Fatalf("runtime Codex User-Agent = %q", value)
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

func assertLiteSubscriptionBody(t *testing.T, body []byte, messages, tools []any) {
	t.Helper()
	value := decodeRequestObject(t, body)
	input, ok := value["input"].([]any)
	if !ok || len(input) != len(messages)+2 {
		t.Fatalf("lite input count = %d, want %d", len(input), len(messages)+2)
	}
	additional, ok := input[0].(map[string]any)
	if !ok || additional["type"] != "additional_tools" ||
		!jsonEqual(additional["tools"], canonicalExpectedLiteTools(tools)) {
		t.Fatalf("lite additional tools = %#v", input[0])
	}
	assertDeveloperMessageText(t, input[1], "Answer both user messages.")
	assertNormalizedMessages(t, input[2:], messages)
	base := input[1].(map[string]any)
	if !strings.HasPrefix(stringValue(additional["id"]), "at_") || !strings.HasPrefix(stringValue(base["id"]), "msg_") {
		t.Fatal("synthetic Lite tools/base items omitted stable IDs")
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
	if len(input) != len(messages) {
		t.Fatalf("non-lite input count = %d, want %d", len(input), len(messages))
	}
	assertNormalizedMessages(t, input, messages)
	if !jsonEqual(value["tools"], canonicalExpectedTools(tools)) {
		t.Fatalf("non-lite messages/tools = %#v", value)
	}
	if value["instructions"] != "Look up the requested order." {
		t.Fatal("non-lite caller base instructions changed")
	}
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
	if len(input) != len(messages) {
		t.Fatalf("native subscription input count = %d, want %d", len(input), len(messages))
	}
	assertMessageSemantics(t, input, messages)
	if value["instructions"] != "Continue the existing turn." {
		t.Fatal("native subscription caller base instructions changed")
	}
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
		_, hasID := gotMessage["id"]
		metadata, _ := gotMessage["internal_chat_message_metadata_passthrough"].(map[string]any)
		role, _ := gotMessage["role"].(string)
		wantCreateTime := role == "user" || role == "system" || role == "developer"
		hasCreateTime := metadata["create_time"] != nil
		_, wantID := want[index].(map[string]any)["id"]
		if hasID != wantID || metadata["turn_id"] == "" || hasCreateTime != wantCreateTime {
			t.Fatalf("normalized message %d identity shape changed", index)
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
