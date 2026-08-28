package integration

import (
	"bytes"
	"context"
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
	var runtimeCodexUserAgent string
	assertSharedRuntimeUserAgent := func(t *testing.T, capture routingMatrixCapture) {
		t.Helper()
		value := capture.Headers.Get("User-Agent")
		assertRuntimeCodexUserAgent(t, value)
		if runtimeCodexUserAgent == "" {
			runtimeCodexUserAgent = value
			return
		}
		if value != runtimeCodexUserAgent {
			t.Fatalf("runtime Codex User-Agent changed: got %q want %q", value, runtimeCodexUserAgent)
		}
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
			name:      "api_key_emulates_codex_api_multi_message_mixed_tools",
			secret:    apiKey.Secret,
			body:      codexAPIBody,
			headers:   codexScenarioHeaders("api-key", "codex_cli_rs/9.9.9 routing-matrix"),
			wantRoute: "Codex OpenAI 0.149",
			assert: func(t *testing.T, capture routingMatrixCapture) {
				assertCodexOpenAIProfileCapture(t, capture)
				assertSharedRuntimeUserAgent(t, capture)
				value := decodeRequestObject(t, capture.Body)
				assertCodexBaseInstructions(t, value["instructions"], "gpt-5.4")
				input := value["input"].([]any)
				assertDeveloperMessageText(t, input[0], "Use the available tools only when needed.")
				assertNormalizedMessages(t, input[1:], messages)
				if capture.Headers.Get("Session-Id") != "api-key-session" ||
					capture.Headers.Get("Version") != "0.149.0" ||
					capture.Headers.Get("X-Openai-Subagent") != "review" {
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
				assertSharedRuntimeUserAgent(t, capture)
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
				assertSharedRuntimeUserAgent(t, capture)
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
				assertSharedRuntimeUserAgent(t, capture)
				assertNativeSubscriptionBody(t, capture.Body, messages)
				if capture.Headers.Get("Originator") != "codex-tui" ||
					!isUUIDv8(capture.Headers.Get("Session-Id")) ||
					capture.Headers.Get("X-Openai-Subagent") != "review" {
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
