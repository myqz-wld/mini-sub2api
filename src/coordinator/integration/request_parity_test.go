package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/httpapi"
	"mini-sub2api/src/coordinator/internal/storage"
)

type parityCapture struct {
	Headers http.Header
	Body    []byte
}

func TestGrokShapedResponsesRequestIsNormalizedForSubscription(t *testing.T) {
	coreBinary := findCoreBinary(t)
	stateDir := t.TempDir()
	coreStateDir := filepath.Join(stateDir, "core-codex")
	accountID := "chatgpt-parity-account"
	accessToken := testJWT(nil, 3600)
	authFile := filepath.Join(stateDir, "codex-auth.json")
	authJSON, err := json.Marshal(map[string]any{
		"auth_mode": "chatgpt",
		"tokens": map[string]string{
			"id_token": testJWT(&accountID, 3600), "access_token": accessToken,
			"refresh_token": "must-not-be-imported", "account_id": accountID,
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(authFile, authJSON, 0o600); err != nil {
		t.Fatal(err)
	}

	captures := make(chan parityCapture, 1)
	upstream := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		body, _ := io.ReadAll(request.Body)
		captures <- parityCapture{Headers: request.Header.Clone(), Body: body}
		writer.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(writer, completedSSE(7))
	}))
	defer upstream.Close()
	assertLoopbackURL(t, upstream.URL)

	metadata := createCoreCredential(t, coreBinary, []string{
		"credential", "import-codex-auth", "--state-dir", coreStateDir,
		"--auth-file", authFile, "--issuer", upstream.URL,
		"--client-id", "client-parity-test", "--upstream-url", upstream.URL + "/responses",
	}, "")
	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	credential := persistCredential(t, store, "Parity subscription", metadata)
	key := createDownstreamKey(t, store, credential.ID, "Parity client")
	supervisor, err := adapter.Start(context.Background(), adapter.Config{
		Binary: coreBinary, StateDir: coreStateDir,
	})
	if err != nil {
		t.Fatal(err)
	}
	defer supervisor.Close()
	public := httptest.NewServer(httpapi.NewHandler(store, supervisor, nil))
	defer public.Close()

	tools := []any{
		map[string]any{
			"type": "function", "name": "lookup", "description": "Lookup",
			"parameters": map[string]any{"type": "object"},
		},
		map[string]any{"type": "web_search_preview"},
	}
	requestJSON, err := json.Marshal(map[string]any{
		"model": "gpt-5.6-sol", "input": []any{
			map[string]any{"type": "message", "role": "system", "content": "Follow system rules"},
			map[string]any{"type": "message", "role": "user", "content": "hello"},
		},
		"tools": tools, "reasoning": map[string]string{"effort": "low"}, "stream": true,
		"max_output_tokens": 32768, "temperature": 0.2, "top_p": 0.9,
	})
	if err != nil {
		t.Fatal(err)
	}
	request, err := http.NewRequest(http.MethodPost, public.URL+"/v1/responses", bytes.NewReader(requestJSON))
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("Authorization", "Bearer "+key.Secret)
	request.Header.Set("Accept", "text/event-stream")
	request.Header.Set("Content-Type", "application/json")
	response, err := public.Client().Do(request)
	if err != nil {
		t.Fatal(err)
	}
	_, _ = io.Copy(io.Discard, response.Body)
	response.Body.Close()
	if response.StatusCode != http.StatusOK {
		t.Fatalf("public status = %d", response.StatusCode)
	}

	var capture parityCapture
	select {
	case capture = <-captures:
	case <-time.After(2 * time.Second):
		t.Fatal("upstream request was not captured")
	}
	if capture.Headers.Get("Authorization") != "Bearer "+accessToken ||
		capture.Headers.Get("ChatGPT-Account-ID") != accountID {
		t.Fatal("subscription authorization was not rebuilt")
	}
	if capture.Headers.Get("Originator") != "mini_sub2api" ||
		capture.Headers.Get("X-OpenAI-Internal-Codex-Responses-Lite") != "true" {
		t.Fatalf("subscription markers = %#v", capture.Headers)
	}
	if capture.Headers.Get("User-Agent") != "codex_cli_rs/0.147.0" {
		t.Fatalf("subscription User-Agent = %q", capture.Headers.Get("User-Agent"))
	}
	for _, name := range []string{
		"Session-Id", "Thread-Id", "X-Client-Request-Id", "X-Codex-Installation-Id",
		"X-Codex-Turn-Metadata", "X-Codex-Window-Id",
	} {
		if capture.Headers.Get(name) == "" {
			t.Fatalf("missing synthesized %s", name)
		}
	}
	var normalized map[string]any
	if err := json.Unmarshal(capture.Body, &normalized); err != nil {
		t.Fatal(err)
	}
	if len(normalized) != 11 {
		t.Fatalf("normalized top-level field count = %d, fields = %#v", len(normalized), normalized)
	}
	if normalized["tools"] != nil || normalized["instructions"] != nil || normalized["store"] != false {
		t.Fatalf("normalization fields = %#v", normalized)
	}
	for _, field := range []string{"max_output_tokens", "temperature", "top_p"} {
		if _, ok := normalized[field]; ok {
			t.Fatalf("unsupported subscription field %s crossed upstream", field)
		}
	}
	if normalized["parallel_tool_calls"] != false || normalized["tool_choice"] != "auto" ||
		normalized["stream"] != true {
		t.Fatalf("Codex request controls = %#v", normalized)
	}
	if !jsonEqual(normalized["include"], []any{"reasoning.encrypted_content"}) {
		t.Fatalf("normalized include = %#v", normalized["include"])
	}
	reasoning := normalized["reasoning"].(map[string]any)
	text := normalized["text"].(map[string]any)
	if reasoning["effort"] != "low" || reasoning["context"] != "all_turns" ||
		text["verbosity"] != "low" {
		t.Fatalf("model defaults reasoning=%#v text=%#v", reasoning, text)
	}
	clientMetadata := normalized["client_metadata"].(map[string]any)
	if clientMetadata["session_id"] != capture.Headers.Get("Session-Id") ||
		clientMetadata["thread_id"] != capture.Headers.Get("Thread-Id") ||
		clientMetadata["turn_id"] != capture.Headers.Get("X-Client-Request-Id") ||
		normalized["prompt_cache_key"] != capture.Headers.Get("Session-Id") {
		t.Fatalf("metadata/header mismatch: metadata=%#v headers=%#v", clientMetadata, capture.Headers)
	}
	input, ok := normalized["input"].([]any)
	if !ok || len(input) != 3 {
		t.Fatalf("normalized input = %#v", normalized["input"])
	}
	additional := input[0].(map[string]any)
	if additional["type"] != "additional_tools" {
		t.Fatalf("first input = %#v", additional)
	}
	if !jsonEqual(additional["tools"], tools) {
		t.Fatalf("normalized tools = %#v, want %#v", additional["tools"], tools)
	}
	developer, ok := input[1].(map[string]any)
	if !ok || developer["role"] != "developer" || developer["content"] != "Follow system rules" {
		t.Fatalf("normalized system message = %#v", input[1])
	}
	user, ok := input[2].(map[string]any)
	if !ok || user["role"] != "user" {
		t.Fatalf("normalized user message = %#v", input[2])
	}
	assertExactStats(t, store, key.ID, 7)
}

func jsonEqual(left, right any) bool {
	leftJSON, _ := json.Marshal(left)
	rightJSON, _ := json.Marshal(right)
	return bytes.Equal(leftJSON, rightJSON)
}
