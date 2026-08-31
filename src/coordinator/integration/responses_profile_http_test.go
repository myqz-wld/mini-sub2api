package integration

import (
	"context"
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

type responsesProfileHTTPFixture struct {
	apiKey          string
	subscriptionKey string
	public          *httptest.Server
	captures        <-chan routingMatrixCapture
}

func TestResponsesProfileHTTPMatrixTwoTurns(t *testing.T) {
	fixture := newResponsesProfileHTTPFixture(t)
	firstNormal, secondNormal := responsesProfileHTTPBodies(t, "gpt-5.4")
	firstLite, secondLite := responsesProfileHTTPBodies(t, "gpt-5.6-sol")
	tests := []struct {
		name         string
		secret       string
		headers      http.Header
		first        []byte
		second       []byte
		lite         bool
		emulates     bool
		subscription bool
	}{
		{
			name: "bare_api_key_normal", secret: fixture.apiKey,
			first: firstNormal, second: secondNormal,
		},
		{
			name: "codex_api_key_normal", secret: fixture.apiKey,
			headers: codexScenarioHeaders("profile-api", "profile-client/0.149.0"),
			first:   firstNormal, second: secondNormal, emulates: true,
		},
		{
			name: "bare_subscription_lite", secret: fixture.subscriptionKey,
			first: firstLite, second: secondLite, lite: true, emulates: true, subscription: true,
		},
		{
			name: "codex_subscription_lite", secret: fixture.subscriptionKey,
			headers: codexScenarioHeaders("profile-subscription", "profile-client/0.149.0"),
			first:   firstLite, second: secondLite, lite: true, emulates: true, subscription: true,
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			for turn, body := range [][]byte{test.first, test.second} {
				status, _, _ := publicRequestWithHeaders(t, fixture.public, test.secret, string(body), test.headers)
				if status != http.StatusOK {
					t.Fatalf("turn %d public response = %d", turn+1, status)
				}
				capture := waitForRoutingCapture(t, fixture.captures)
				assertHTTPProfileCredentialBoundary(t, capture, test.subscription)
				if !test.emulates {
					assertAPIKeyCapture(t, capture, body)
					continue
				}
				assertResponsesProfileHTTPBody(t, capture.Body, test.lite, turn == 1, test.subscription)
			}
		})
	}
}

func newResponsesProfileHTTPFixture(t *testing.T) responsesProfileHTTPFixture {
	t.Helper()
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
		writeLoopbackResponsesResult(writer, body, "resp_profile")
	}))
	t.Cleanup(upstream.Close)
	assertLoopbackURL(t, upstream.URL)

	stateDir := t.TempDir()
	coreStateDir := filepath.Join(stateDir, "core-codex")
	apiMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "add-api-key", "--state-dir", coreStateDir,
		"--upstream-url", upstream.URL + "/responses", "--secret-stdin",
	}, upstreamAPIKey+"\n")
	accountID := "profile-loopback-account"
	authFile := filepath.Join(stateDir, "codex-auth.json")
	authJSON := mustRequestJSON(t, map[string]any{
		"auth_mode": "chatgpt",
		"tokens": map[string]string{
			"id_token": testJWT(&accountID, 3600), "access_token": testJWT(nil, 3600),
			"refresh_token": "not-imported-profile", "account_id": accountID,
		},
	})
	if err := os.WriteFile(authFile, authJSON, 0o600); err != nil {
		t.Fatal(err)
	}
	oauthMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "import-codex-auth", "--state-dir", coreStateDir,
		"--auth-file", authFile, "--issuer", upstream.URL,
		"--client-id", "profile-loopback-client", "--upstream-url", upstream.URL + "/responses",
	}, "")
	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	apiCredential := persistCredential(t, store, "Profile API key", apiMetadata)
	subscriptionCredential := persistCredential(t, store, "Profile subscription", oauthMetadata)
	apiKey := createDownstreamKey(t, store, apiCredential.ID, "Profile API client")
	subscriptionKey := createDownstreamKey(t, store, subscriptionCredential.ID, "Profile subscription client")
	supervisor, err := adapter.Start(context.Background(), adapter.Config{Binary: coreBinary, StateDir: coreStateDir})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = supervisor.Close() })
	public := httptest.NewServer(httpapi.NewHandler(store, supervisor, nil))
	t.Cleanup(public.Close)
	return responsesProfileHTTPFixture{
		apiKey: apiKey.Secret, subscriptionKey: subscriptionKey.Secret, public: public, captures: captures,
	}
}

func responsesProfileHTTPBodies(t *testing.T, model string) ([]byte, []byte) {
	t.Helper()
	first := responsesProfileRequest(model, []any{responsesProfileMessage("first")})
	secondInput := []any{
		responsesProfileMessage("first"),
		map[string]any{"type": "function_call", "call_id": "call_profile", "name": "lookup", "arguments": "{}"},
		map[string]any{"type": "function_call_output", "call_id": "call_profile", "output": []any{
			map[string]any{"type": "input_image", "image_url": "data:image/png;base64,Ag=="},
		}},
		map[string]any{
			"type": "shell_call", "call_id": "call_shell", "status": "completed",
			"action": map[string]any{"commands": []any{"true"}, "max_output_length": 256, "unsupported_shell_action": true},
			"agent":  map[string]any{"agent_name": "profile-worker", "unsupported_agent": true},
		},
		map[string]any{
			"type": "shell_call_output", "call_id": "call_shell", "status": "completed", "max_output_length": 256,
			"output": []any{map[string]any{
				"outcome": map[string]any{"type": "exit", "exit_code": 0, "unsupported_outcome": true},
				"stdout":  "ok", "stderr": "", "unsupported_shell_output": true,
			}},
		},
		responsesProfileMessage("second"),
	}
	second := responsesProfileRequest(model, secondInput)
	second["previous_response_id"] = "explicit-profile-previous"
	second["conversation"] = "explicit-profile-conversation"
	return mustRequestJSON(t, first), mustRequestJSON(t, second)
}

func responsesProfileRequest(model string, input []any) map[string]any {
	return map[string]any{
		"model": model, "input": input, "instructions": "unit instruction", "tools": []any{
			map[string]any{
				"type": "function", "name": "lookup",
				"parameters": map[string]any{
					"type": "object", "x-profile-schema-sentinel": map[string]any{"retain": true},
				},
			},
			map[string]any{
				"type": "custom", "name": "profile_custom", "description": "unit custom tool",
				"format": map[string]any{
					"type": "grammar", "syntax": "lark", "definition": "start: TOKEN",
					"unsupported_custom_format_sentinel": map[string]any{"must": "strip"},
				},
			},
		},
		"tool_choice": "required", "parallel_tool_calls": true,
		"reasoning": map[string]any{"effort": "high", "summary": "detailed"},
		"store":     true, "stream": false, "stream_options": map[string]any{"include_obfuscation": true},
		"include": []any{"reasoning.encrypted_content"}, "service_tier": "flex",
		"prompt_cache_key": "profile-cache", "text": map[string]any{"verbosity": "high"},
		"client_metadata": map[string]any{"session_id": "profile-session"},
		"background":      true, "context_management": []any{
			map[string]any{
				"type": "compaction", "compact_threshold": 1200,
				"unsupported_context_sentinel": true,
			},
		},
		"conversation": "profile-conversation", "max_output_tokens": 31, "max_tool_calls": 2,
		"metadata": map[string]any{
			"profile": "matrix", "opaque_metadata_sentinel": "retain",
		},
		"moderation": map[string]any{
			"model": "omni-moderation-latest", "policy": "default",
			"unsupported_moderation_sentinel": true,
		},
		"prompt": map[string]any{
			"id": "profile-prompt", "variables": map[string]any{"opaque_prompt_sentinel": "retain"},
		},
		"prompt_cache_options": map[string]any{
			"mode": "explicit", "ttl": "30m", "unsupported_cache_sentinel": true,
		},
		"prompt_cache_retention": "in_memory",
		"safety_identifier":      "profile-safety", "temperature": 0.25, "top_p": 0.75,
		"top_logprobs": 3, "truncation": "disabled", "user": "profile-user",
		"unsupported_profile_sentinel": map[string]any{"must": "strip"},
	}
}

func responsesProfileMessage(label string) map[string]any {
	return map[string]any{
		"type": "message", "role": "user", "content": []any{
			map[string]any{"type": "input_text", "text": label},
			map[string]any{"type": "input_image", "image_url": "data:image/png;base64,AA=="},
			map[string]any{"type": "input_image", "image_url": "data:image/png;base64,AQ==", "detail": "low"},
		},
	}
}

func assertHTTPProfileCredentialBoundary(t *testing.T, capture routingMatrixCapture, subscription bool) {
	t.Helper()
	if subscription {
		if capture.Headers.Get("ChatGPT-Account-ID") == "" || capture.Headers.Get("Content-Encoding") != "zstd" {
			t.Fatal("subscription profile did not retain subscription-only authentication or zstd")
		}
		return
	}
	if capture.Headers.Get("Authorization") != "Bearer "+upstreamAPIKey ||
		capture.Headers.Get("ChatGPT-Account-ID") != "" || capture.Headers.Get("Content-Encoding") != "" {
		t.Fatal("API-key profile crossed a subscription-only credential boundary")
	}
}

func assertResponsesProfileHTTPBody(t *testing.T, body []byte, lite, hasExplicitPrevious, subscription bool) {
	t.Helper()
	value := decodeRequestObject(t, body)
	assertResponsesProfileSurface(t, value, lite, false, subscription)
	if hasExplicitPrevious {
		if value["previous_response_id"] != "explicit-profile-previous" ||
			value["conversation"] != "explicit-profile-conversation" {
			t.Fatal("explicit HTTP continuation state was not preserved")
		}
		if !containsResponseProfileItem(value["input"], "function_call") ||
			!containsResponseProfileItem(value["input"], "function_call_output") ||
			!responsesProfileShellBoundariesValid(value["input"]) {
			t.Fatal("HTTP tool continuation was not sent as full input history")
		}
	} else if _, exists := value["previous_response_id"]; exists {
		t.Fatal("HTTP profile synthesized previous_response_id")
	}
	assertResponsesProfileImageDetails(t, value["input"], lite)
}

func responsesProfileShellBoundariesValid(input any) bool {
	items, _ := input.([]any)
	for _, item := range items {
		object, _ := item.(map[string]any)
		if object["type"] != "shell_call" {
			continue
		}
		action, _ := object["action"].(map[string]any)
		agent, _ := object["agent"].(map[string]any)
		_, actionUnknown := action["unsupported_shell_action"]
		_, agentUnknown := agent["unsupported_agent"]
		return action["max_output_length"] == float64(256) && agent["agent_name"] == "profile-worker" &&
			!actionUnknown && !agentUnknown
	}
	return false
}

func assertResponsesProfileSurface(t *testing.T, value map[string]any, lite, websocket, subscription bool) {
	t.Helper()
	model, _ := value["model"].(string)
	for _, field := range []string{
		"model", "input", "tool_choice", "parallel_tool_calls", "reasoning", "store",
		"stream_options", "include", "service_tier", "text", "context_management",
		"max_tool_calls", "metadata", "moderation", "prompt", "prompt_cache_options",
		"prompt_cache_retention", "safety_identifier", "top_logprobs", "truncation", "user",
	} {
		if _, exists := value[field]; !exists {
			t.Fatalf("Responses field %q was removed", field)
		}
	}
	for _, field := range []string{"max_output_tokens", "temperature", "top_p"} {
		_, exists := value[field]
		if exists == subscription {
			t.Fatalf("Subscription-only field policy mismatch for %q", field)
		}
	}
	if websocket {
		for _, field := range []string{"stream", "background"} {
			if _, exists := value[field]; exists {
				t.Fatalf("unsupported WebSocket field %q survived", field)
			}
		}
	} else {
		for _, field := range []string{"stream", "background"} {
			if _, exists := value[field]; !exists {
				t.Fatalf("HTTP Responses field %q was removed", field)
			}
		}
		if value["store"] != false || value["stream"] != true {
			t.Fatal("HTTP Codex profile did not pin store=false and stream=true")
		}
	}
	if _, exists := value["unsupported_profile_sentinel"]; exists {
		t.Fatal("unsupported top-level field was not stripped")
	}
	metadata, ok := value["metadata"].(map[string]any)
	if !ok || metadata["opaque_metadata_sentinel"] != "retain" {
		t.Fatal("metadata free-form key did not round trip")
	}
	prompt, ok := value["prompt"].(map[string]any)
	if !ok || !jsonEqual(prompt["variables"], map[string]any{"opaque_prompt_sentinel": "retain"}) {
		t.Fatal("prompt free-form key did not round trip")
	}
	context, ok := value["context_management"].([]any)
	if !ok || len(context) != 1 {
		t.Fatal("context_management did not retain its documented array shape")
	}
	contextEntry, ok := context[0].(map[string]any)
	if !ok || contextEntry["type"] != "compaction" || contextEntry["compact_threshold"] != float64(1200) {
		t.Fatal("context_management documented members did not round trip")
	}
	if _, exists := contextEntry["unsupported_context_sentinel"]; exists {
		t.Fatal("context_management unsupported member was not stripped")
	}
	moderation, ok := value["moderation"].(map[string]any)
	if !ok || moderation["model"] != "omni-moderation-latest" || moderation["policy"] != "default" {
		t.Fatal("moderation documented members did not round trip")
	}
	if _, exists := moderation["unsupported_moderation_sentinel"]; exists {
		t.Fatal("moderation unsupported member was not stripped")
	}
	cache, ok := value["prompt_cache_options"].(map[string]any)
	if !ok || cache["mode"] != "explicit" || cache["ttl"] != "30m" {
		t.Fatal("prompt_cache_options documented members did not round trip")
	}
	if _, exists := cache["unsupported_cache_sentinel"]; exists {
		t.Fatal("prompt_cache_options unsupported member was not stripped")
	}
	if !responsesProfileToolBoundariesValid(value) {
		t.Fatal("schema free-form content or structured custom-tool filtering is incorrect")
	}
	if lite {
		if _, exists := value["tools"]; exists {
			t.Fatal("Lite profile kept top-level tools")
		}
		if _, exists := value["instructions"]; exists {
			t.Fatal("Lite profile kept top-level instructions")
		}
		assertCodexBaseDeveloperMessage(t, value["input"], model)
		return
	}
	for _, field := range []string{"instructions", "tools", "prompt_cache_key", "client_metadata"} {
		if _, exists := value[field]; !exists {
			t.Fatalf("normal profile removed Responses field %q", field)
		}
	}
	assertCodexBaseInstructions(t, value["instructions"], model)
}

func responsesProfileToolBoundariesValid(value any) bool {
	var functionSchema, customFormatFiltered bool
	var visit func(any)
	visit = func(value any) {
		switch value := value.(type) {
		case []any:
			for _, item := range value {
				visit(item)
			}
		case map[string]any:
			if value["type"] == "function" && value["name"] == "lookup" {
				functionSchema = jsonEqual(value["parameters"], map[string]any{
					"type": "object", "x-profile-schema-sentinel": map[string]any{"retain": true},
				})
			}
			if value["type"] == "custom" && value["name"] == "profile_custom" {
				format, ok := value["format"].(map[string]any)
				_, unknownExists := format["unsupported_custom_format_sentinel"]
				customFormatFiltered = ok && !unknownExists && format["type"] == "grammar" &&
					format["syntax"] == "lark" && format["definition"] == "start: TOKEN"
			}
			for _, member := range value {
				visit(member)
			}
		}
	}
	visit(value)
	return functionSchema && customFormatFiltered
}

func containsResponseProfileItem(input any, kind string) bool {
	items, ok := input.([]any)
	if !ok {
		return false
	}
	for _, item := range items {
		if object, ok := item.(map[string]any); ok && object["type"] == kind {
			return true
		}
	}
	return false
}

func assertResponsesProfileImageDetails(t *testing.T, input any, lite bool) {
	t.Helper()
	images := responsesProfileImages(input)
	if len(images) < 2 {
		t.Fatal("profile input did not retain image items")
	}
	for index, image := range images {
		_, hasDetail := image["detail"]
		explicitLow := image["image_url"] == "data:image/png;base64,AQ=="
		if lite {
			if explicitLow && image["detail"] != "low" {
				t.Fatalf("Lite image %d did not preserve explicit detail", index)
			}
			if !explicitLow && hasDetail {
				t.Fatalf("Lite image %d synthesized detail", index)
			}
		} else {
			want := "high"
			if explicitLow {
				want = "low"
			}
			if image["detail"] != want {
				t.Fatalf("normal image %d detail = %v, want %s", index, image["detail"], want)
			}
		}
	}
}

func responsesProfileImages(value any) []map[string]any {
	var images []map[string]any
	var visit func(any)
	visit = func(value any) {
		switch value := value.(type) {
		case []any:
			for _, item := range value {
				visit(item)
			}
		case map[string]any:
			if value["type"] == "input_image" {
				images = append(images, value)
			}
			for _, member := range value {
				visit(member)
			}
		}
	}
	visit(value)
	return images
}
