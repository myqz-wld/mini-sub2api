package integration

import (
	"bytes"
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/httpapi"
	"mini-sub2api/src/coordinator/internal/storage"
)

type routingMatrixCapture struct {
	Headers http.Header
	Body    []byte
}

func TestRequestRoutingMatrixWithMultipleMessagesAndToolSets(t *testing.T) {
	t.Setenv("NO_PROXY", "127.0.0.1,::1")
	t.Setenv("no_proxy", "127.0.0.1,::1")

	coreBinary := findCoreBinary(t)
	captures := make(chan routingMatrixCapture, 8)
	upstream := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		body, err := readCapturedUpstreamBody(request)
		if err != nil {
			http.Error(writer, "capture body", http.StatusInternalServerError)
			return
		}
		captures <- routingMatrixCapture{Headers: request.Header.Clone(), Body: body}
		writer.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(writer, `{"id":"resp_matrix","usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}`)
	}))
	defer upstream.Close()
	assertLoopbackURL(t, upstream.URL)

	stateDir := t.TempDir()
	coreStateDir := filepath.Join(stateDir, "core-codex")
	apiMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "add-api-key", "--state-dir", coreStateDir,
		"--upstream-url", upstream.URL + "/responses", "--secret-stdin",
	}, upstreamAPIKey+"\n")

	accountID := "chatgpt-routing-matrix"
	oauthAccessToken := testJWT(nil, 3600)
	authFile := filepath.Join(stateDir, "codex-auth.json")
	authJSON := mustRequestJSON(t, map[string]any{
		"auth_mode": "chatgpt",
		"tokens": map[string]string{
			"id_token":      testJWT(&accountID, 3600),
			"access_token":  oauthAccessToken,
			"refresh_token": "not-imported-routing-matrix",
			"account_id":    accountID,
		},
	})
	if err := os.WriteFile(authFile, authJSON, 0o600); err != nil {
		t.Fatal(err)
	}
	oauthMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "import-codex-auth", "--state-dir", coreStateDir,
		"--auth-file", authFile, "--issuer", upstream.URL,
		"--client-id", "client-routing-matrix", "--upstream-url", upstream.URL + "/responses",
	}, "")

	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	apiCredential := persistCredential(t, store, "Routing API key", apiMetadata)
	oauthCredential := persistCredential(t, store, "Routing OAuth subscription", oauthMetadata)
	apiKey := createDownstreamKey(t, store, apiCredential.ID, "Routing API client")
	oauthKey := createDownstreamKey(t, store, oauthCredential.ID, "Routing OAuth client")

	supervisor, err := adapter.Start(context.Background(), adapter.Config{
		Binary: coreBinary, StateDir: coreStateDir,
	})
	if err != nil {
		t.Fatal(err)
	}
	defer supervisor.Close()
	public := httptest.NewServer(httpapi.NewHandler(store, supervisor, nil))
	defer public.Close()

	messages := []any{
		map[string]any{
			"type": "message", "role": "user",
			"content": []any{map[string]any{"type": "input_text", "text": "First user question"}},
		},
		map[string]any{
			"type": "message", "role": "assistant",
			"content": []any{map[string]any{"type": "output_text", "text": "Intermediate answer"}},
		},
		map[string]any{
			"type": "message", "role": "user",
			"content": []any{map[string]any{"type": "input_text", "text": "Second user question"}},
		},
	}
	functionTools := []any{
		map[string]any{
			"type": "function", "name": "lookup_order", "description": "Look up an order",
			"parameters": map[string]any{
				"type": "object",
				"properties": map[string]any{
					"order_id": map[string]any{"type": "string"},
				},
				"required": []any{"order_id"},
			},
		},
	}
	mixedTools := []any{
		functionTools[0],
		map[string]any{"type": "web_search_preview", "search_context_size": "low"},
	}

	pureAPIBody := mustRequestJSON(t, map[string]any{
		"model": "gpt-5.4", "input": messages, "tools": []any{}, "stream": false,
	})
	codexAPIBody := mustRequestJSON(t, map[string]any{
		"model":               "gpt-5.4",
		"instructions":        "Use the available tools only when needed.",
		"input":               messages,
		"tools":               mixedTools,
		"tool_choice":         "auto",
		"parallel_tool_calls": true,
		"reasoning":           map[string]any{"effort": "medium", "summary": "auto"},
		"text":                map[string]any{"verbosity": "low"},
		"store":               false,
		"stream":              false,
		"include":             []any{"reasoning.encrypted_content"},
		"prompt_cache_key":    "api-key-codex-session",
		"client_metadata": map[string]any{
			"session_id": "api-key-codex-session", "thread_id": "api-key-codex-thread",
			"turn_id": "api-key-codex-turn",
		},
	})
	litePlainBody := mustRequestJSON(t, map[string]any{
		"model": "gpt-5.6-sol", "instructions": "Answer both user messages.",
		"input": messages, "tools": mixedTools, "stream": false,
	})
	nonLitePlainBody := mustRequestJSON(t, map[string]any{
		"model": "gpt-5.4-mini", "instructions": "Look up the requested order.",
		"input": messages, "tools": functionTools, "stream": false,
	})
	codexToSubscriptionBody := mustRequestJSON(t, map[string]any{
		"model":               "gpt-5.4",
		"instructions":        "Continue the existing turn.",
		"input":               messages,
		"tools":               []any{},
		"tool_choice":         "auto",
		"parallel_tool_calls": true,
		"reasoning":           map[string]any{"effort": "medium"},
		"text":                map[string]any{"verbosity": "low"},
		"store":               false,
		"stream":              false,
		"include":             []any{"reasoning.encrypted_content"},
		"prompt_cache_key":    "oauth-codex-session",
		"client_metadata": map[string]any{
			"session_id": "oauth-codex-session", "thread_id": "oauth-codex-thread",
			"turn_id": "oauth-codex-turn",
		},
	})

	tests := []struct {
		name      string
		secret    string
		body      []byte
		headers   http.Header
		wantRoute string
		assert    func(*testing.T, routingMatrixCapture)
	}{
		{
			name:   "api_key_keeps_pure_api_multi_message_empty_tools",
			secret: apiKey.Secret,
			body:   pureAPIBody,
			headers: http.Header{
				"User-Agent":          []string{"OpenAI/Go 3.52.0"},
				"OpenAI-Organization": []string{"org-routing"},
				"OpenAI-Project":      []string{"proj-routing"},
				"X-Stainless-Lang":    []string{"go"},
			},
			wantRoute: "pure API",
			assert: func(t *testing.T, capture routingMatrixCapture) {
				assertAPIKeyCapture(t, capture, pureAPIBody)
				if capture.Headers.Get("OpenAI-Organization") != "org-routing" ||
					capture.Headers.Get("OpenAI-Project") != "proj-routing" ||
					capture.Headers.Get("X-Stainless-Lang") != "go" {
					t.Fatalf("pure API headers = %#v", capture.Headers)
				}
			},
		},
		{
			name:      "api_key_keeps_codex_api_multi_message_mixed_tools",
			secret:    apiKey.Secret,
			body:      codexAPIBody,
			headers:   codexScenarioHeaders("api-key", "codex_cli_rs/9.9.9 routing-matrix"),
			wantRoute: "Codex API",
			assert: func(t *testing.T, capture routingMatrixCapture) {
				assertAPIKeyCapture(t, capture, codexAPIBody)
				if capture.Headers.Get("Originator") != "codex_exec" ||
					capture.Headers.Get("Session-Id") != "api-key-session" ||
					capture.Headers.Get("Version") != "9.9.9" ||
					capture.Headers.Get("X-Openai-Subagent") != "review" ||
					capture.Headers.Get("User-Agent") != "codex_cli_rs/9.9.9 routing-matrix" {
					t.Fatalf("Codex API headers = %#v", capture.Headers)
				}
			},
		},
		{
			name:   "oauth_converts_plain_lite_multi_message_mixed_tools",
			secret: oauthKey.Secret,
			body:   litePlainBody,
			headers: http.Header{
				"User-Agent":          []string{"scenario-client/1.0"},
				"OpenAI-Organization": []string{"must-not-cross"},
				"X-Stainless-Lang":    []string{"must-not-cross"},
			},
			wantRoute: "Codex Sub lite",
			assert: func(t *testing.T, capture routingMatrixCapture) {
				assertSubscriptionCapture(t, capture, oauthAccessToken, accountID)
				assertLiteSubscriptionBody(t, capture.Body, messages, mixedTools)
				if capture.Headers.Get("X-OpenAI-Internal-Codex-Responses-Lite") != "true" {
					t.Fatalf("missing lite header: %#v", capture.Headers)
				}
			},
		},
		{
			name:      "oauth_converts_plain_non_lite_multi_message_function_tool",
			secret:    oauthKey.Secret,
			body:      nonLitePlainBody,
			headers:   http.Header{"User-Agent": []string{"scenario-client/1.0"}},
			wantRoute: "Codex Sub non-lite",
			assert: func(t *testing.T, capture routingMatrixCapture) {
				assertSubscriptionCapture(t, capture, oauthAccessToken, accountID)
				assertNonLiteSubscriptionBody(t, capture.Body, messages, functionTools)
				if capture.Headers.Get("X-OpenAI-Internal-Codex-Responses-Lite") != "" {
					t.Fatalf("unexpected lite header: %#v", capture.Headers)
				}
			},
		},
		{
			name:   "oauth_turns_codex_api_into_exact_subscription_shape",
			secret: oauthKey.Secret,
			body:   codexToSubscriptionBody,
			headers: func() http.Header {
				headers := codexScenarioHeaders("oauth", "codex_cli_rs/9.9.9 routing-matrix")
				headers.Set("OpenAI-Project", "must-not-cross")
				return headers
			}(),
			wantRoute: "Codex Sub native",
			assert: func(t *testing.T, capture routingMatrixCapture) {
				assertSubscriptionCapture(t, capture, oauthAccessToken, accountID)
				assertNativeSubscriptionBody(t, capture.Body, messages)
				if capture.Headers.Get("Originator") != "codex_exec" ||
					capture.Headers.Get("Session-Id") != "oauth-session" ||
					capture.Headers.Get("X-Openai-Subagent") != "review" ||
					capture.Headers.Get("User-Agent") != "codex_cli_rs/0.149.0 routing-matrix" {
					t.Fatalf("native subscription headers = %#v", capture.Headers)
				}
			},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			status, responseBody, _ := publicRequestWithHeaders(
				t, public, test.secret, string(test.body), test.headers,
			)
			if status != http.StatusOK {
				t.Fatalf("public response = %d %s", status, responseBody)
			}
			capture := waitForRoutingCapture(t, captures)
			if bytes.Contains(capture.Body, []byte(test.secret)) ||
				capture.Headers.Get("Authorization") == "Bearer "+test.secret {
				t.Fatal("downstream key crossed the upstream boundary")
			}
			test.assert(t, capture)
			t.Logf("verified %s with %d input messages", test.wantRoute, len(messages))
		})
	}
}

func codexScenarioHeaders(prefix, userAgent string) http.Header {
	return http.Header{
		"Originator":               []string{"codex_exec"},
		"Session-Id":               []string{prefix + "-session"},
		"Thread-Id":                []string{prefix + "-thread"},
		"X-Client-Request-Id":      []string{prefix + "-turn"},
		"X-Codex-Installation-Id":  []string{prefix + "-installation"},
		"X-Codex-Turn-Metadata":    []string{`{"turn_id":"` + prefix + `-turn"}`},
		"X-Codex-Window-Id":        []string{prefix + "-window"},
		"X-Codex-Beta-Features":    []string{"routing-matrix"},
		"X-Codex-Parent-Thread-Id": []string{prefix + "-parent"},
		"X-Codex-Turn-State":       []string{"active"},
		"User-Agent":               []string{userAgent},
		"Version":                  []string{"9.9.9"},
		"X-Openai-Subagent":        []string{"review"},
	}
}

func waitForRoutingCapture(t *testing.T, captures <-chan routingMatrixCapture) routingMatrixCapture {
	t.Helper()
	select {
	case capture := <-captures:
		return capture
	case <-time.After(2 * time.Second):
		t.Fatal("upstream request was not captured")
		return routingMatrixCapture{}
	}
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
		t.Fatalf("API-key body changed:\n got: %s\nwant: %s", capture.Body, wantBody)
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
	userAgent := capture.Headers.Get("User-Agent")
	if capture.Headers.Get("Originator") == "" || capture.Headers.Get("Version") != "0.149.0" ||
		userAgent != "codex_cli_rs/0.149.0 routing-matrix" &&
			!strings.HasPrefix(userAgent, "codex_cli_rs/0.149.0 (") {
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
	assertNormalizedMessages(t, input, messages)
	if !jsonEqual(value["tools"], canonicalExpectedTools(tools)) {
		t.Fatalf("non-lite messages/tools = %#v", value)
	}
	if value["instructions"] != "Look up the requested order." || value["store"] != false ||
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
	assertMessageSemantics(t, input, messages)
	if value["model"] != "gpt-5.4" || value["instructions"] != "Continue the existing turn." ||
		value["store"] != false || value["stream"] != true || value["tool_choice"] != "auto" ||
		value["parallel_tool_calls"] != true || value["prompt_cache_key"] != "oauth-codex-session" {
		t.Fatalf("native subscription controls = %#v", value)
	}
	metadata, ok := value["client_metadata"].(map[string]any)
	if !ok || metadata["session_id"] != "oauth-codex-session" ||
		metadata["thread_id"] != "oauth-codex-thread" || metadata["turn_id"] != "oauth-codex-turn" ||
		metadata["x-codex-installation-id"] == "" || metadata["x-codex-turn-metadata"] == "" {
		t.Fatalf("native subscription metadata = %#v", value["client_metadata"])
	}
}

func assertNormalizedMessages(t *testing.T, got, want []any) {
	t.Helper()
	assertMessageSemantics(t, got, want)
	for index := range want {
		gotMessage := got[index].(map[string]any)
		id, _ := gotMessage["id"].(string)
		metadata, _ := gotMessage["internal_chat_message_metadata_passthrough"].(map[string]any)
		role, _ := gotMessage["role"].(string)
		wantCreateTime := role == "user" || role == "developer"
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
			t.Fatalf("normalized message %d = %#v, want semantic %#v", index, got[index], want[index])
		}
	}
}
