package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/httpapi"
	"mini-sub2api/src/coordinator/internal/storage"
)

type responsesProfileWebSocketCapture struct {
	Headers    http.Header
	Frame      []byte
	ResponseID string
}

type responsesProfileWebSocketFixture struct {
	apiKey          string
	subscriptionKey string
	public          *httptest.Server
	captures        <-chan responsesProfileWebSocketCapture
}

type responsesProfileWebSocketResponder func(*websocket.Conn, []byte, string)

func TestResponsesProfileWebSocketMatrixTwoTurnsAndToolFallback(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixture(t)
	tests := []struct {
		name                string
		secret              string
		headers             http.Header
		model               string
		lite                bool
		emulates            bool
		subscription        bool
		expectHiddenPrewarm bool
	}{
		{name: "bare_api_key_normal", secret: fixture.apiKey, model: "gpt-5.4"},
		{
			name: "codex_api_key_normal", secret: fixture.apiKey,
			headers: codexScenarioHeaders("profile-ws-api", "profile-ws/0.149.0"),
			model:   "gpt-5.4", emulates: true,
		},
		{
			name: "bare_subscription_lite", secret: fixture.subscriptionKey, model: "gpt-5.6-sol",
			lite: true, emulates: true, subscription: true, expectHiddenPrewarm: true,
		},
		{
			name: "codex_subscription_lite", secret: fixture.subscriptionKey,
			headers: codexScenarioHeaders("profile-ws-subscription", "profile-ws/0.149.0"),
			model:   "gpt-5.6-sol", lite: true, emulates: true, subscription: true,
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			first, second := responsesProfileWebSocketFrames(t, test.model)
			connection := dialResponsesProfileWebSocket(t, fixture.public, test.secret, test.headers)
			defer connection.CloseNow()
			for _, frame := range [][]byte{first, second} {
				writeE2EWebSocketText(t, connection, string(frame))
				readResponsesProfileTerminalEvents(t, connection)
			}
			captures := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 2+boolToInt(test.expectHiddenPrewarm))
			var hiddenResponseID string
			if test.expectHiddenPrewarm {
				assertResponsesProfilePrewarm(t, captures[0], test.lite)
				hiddenResponseID = captures[0].ResponseID
				captures = captures[1:]
			}
			for _, capture := range captures {
				assertWebSocketProfileCredentialBoundary(t, capture, test.subscription)
			}
			if !test.emulates {
				if !bytes.Equal(captures[0].Frame, first) || !bytes.Equal(captures[1].Frame, second) {
					t.Fatal("bare API-key WebSocket frame changed")
				}
				return
			}
			firstValue := decodeResponsesProfileWebSocketFrame(t, captures[0].Frame)
			secondValue := decodeResponsesProfileWebSocketFrame(t, captures[1].Frame)
			if !test.expectHiddenPrewarm {
				assertResponsesProfileSurface(t, firstValue, test.lite)
			}
			assertResponsesProfileSurface(t, secondValue, test.lite)
			assertResponsesProfileImageDetails(t, firstValue["input"], test.lite)
			if test.expectHiddenPrewarm {
				if firstValue["previous_response_id"] != hiddenResponseID {
					t.Fatal("first public frame did not reuse the hidden prewarm response")
				}
				if containsResponseProfileItem(firstValue["input"], "additional_tools") ||
					containsResponsesProfileRole(firstValue["input"], "developer") {
					t.Fatal("Lite first public frame did not send only the post-prewarm delta")
				}
			}
			if _, exists := secondValue["previous_response_id"]; exists {
				t.Fatal("tool continuation did not fall back to a full WebSocket frame")
			}
			if !containsResponseProfileItem(secondValue["input"], "function_call") ||
				!containsResponseProfileItem(secondValue["input"], "function_call_output") {
				t.Fatal("full WebSocket fallback did not preserve tool continuation")
			}
		})
	}
}

func TestResponsesProfileWebSocketExplicitStatePreventsSyntheticPrewarm(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixture(t)
	frame := responsesProfileRequest("gpt-5.4", []any{responsesProfileMessage("explicit-state")})
	frame["type"] = "response.create"
	frame["previous_response_id"] = "caller-previous"
	frame["conversation"] = "caller-conversation"
	frame["generate"] = false
	encoded := mustRequestJSON(t, frame)
	connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, nil)
	defer connection.CloseNow()
	writeE2EWebSocketText(t, connection, string(encoded))
	readResponsesProfileTerminalEvents(t, connection)
	captures := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)
	value := decodeResponsesProfileWebSocketFrame(t, captures[0].Frame)
	assertWebSocketProfileCredentialBoundary(t, captures[0], true)
	assertResponsesProfileSurface(t, value, false)
	if value["previous_response_id"] != "caller-previous" || value["conversation"] != "caller-conversation" ||
		value["generate"] != false {
		t.Fatal("explicit WebSocket continuation or state was not preserved")
	}
}

func newResponsesProfileWebSocketFixture(t *testing.T) responsesProfileWebSocketFixture {
	return newResponsesProfileWebSocketFixtureWithResponder(t, func(connection *websocket.Conn, payload []byte, responseID string) {
		writeResponsesProfileEvents(connection, responseID, isSyntheticResponsesProfilePrewarm(payload))
	})
}

func newResponsesProfileWebSocketFixtureWithResponder(
	t *testing.T,
	responder responsesProfileWebSocketResponder,
) responsesProfileWebSocketFixture {
	t.Helper()
	t.Setenv("NO_PROXY", "127.0.0.1,::1")
	t.Setenv("no_proxy", "127.0.0.1,::1")
	coreBinary := findCoreBinary(t)
	captures := make(chan responsesProfileWebSocketCapture, 16)
	var responseNumber int
	var responseMu sync.Mutex
	upstream := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		connection, err := websocket.Accept(writer, request, &websocket.AcceptOptions{CompressionMode: websocket.CompressionDisabled})
		if err != nil {
			return
		}
		defer connection.CloseNow()
		for {
			messageType, payload, err := connection.Read(context.Background())
			if err != nil || messageType != websocket.MessageText {
				return
			}
			responseMu.Lock()
			responseNumber++
			identifier := responseNumber
			responseMu.Unlock()
			responseID := responsesProfileResponseID(identifier)
			captures <- responsesProfileWebSocketCapture{
				Headers: request.Header.Clone(), Frame: append([]byte(nil), payload...), ResponseID: responseID,
			}
			responder(connection, payload, responseID)
		}
	}))
	t.Cleanup(upstream.Close)
	assertLoopbackURL(t, upstream.URL)

	stateDir := t.TempDir()
	coreStateDir := filepath.Join(stateDir, "core-codex")
	apiMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "add-api-key", "--state-dir", coreStateDir,
		"--upstream-url", upstream.URL + "/responses", "--secret-stdin",
	}, upstreamAPIKey+"\n")
	accountID := "profile-ws-loopback-account"
	authFile := filepath.Join(stateDir, "codex-auth.json")
	authJSON := mustRequestJSON(t, map[string]any{
		"auth_mode": "chatgpt",
		"tokens": map[string]string{
			"id_token": testJWT(&accountID, 3600), "access_token": testJWT(nil, 3600),
			"refresh_token": "not-imported-profile-ws", "account_id": accountID,
		},
	})
	if err := os.WriteFile(authFile, authJSON, 0o600); err != nil {
		t.Fatal(err)
	}
	oauthMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "import-codex-auth", "--state-dir", coreStateDir,
		"--auth-file", authFile, "--issuer", upstream.URL,
		"--client-id", "profile-ws-loopback-client", "--upstream-url", upstream.URL + "/responses",
	}, "")
	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	apiCredential := persistCredential(t, store, "Profile WebSocket API", apiMetadata)
	subscriptionCredential := persistCredential(t, store, "Profile WebSocket subscription", oauthMetadata)
	apiKey := createDownstreamKey(t, store, apiCredential.ID, "Profile WebSocket API client")
	subscriptionKey := createDownstreamKey(t, store, subscriptionCredential.ID, "Profile WebSocket subscription client")
	supervisor, err := adapter.Start(context.Background(), adapter.Config{Binary: coreBinary, StateDir: coreStateDir})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = supervisor.Close() })
	handler := httpapi.NewHandler(store, supervisor, nil)
	public := httptest.NewServer(handler)
	t.Cleanup(func() {
		handler.ShutdownWebSockets()
		public.Close()
	})
	return responsesProfileWebSocketFixture{
		apiKey: apiKey.Secret, subscriptionKey: subscriptionKey.Secret, public: public, captures: captures,
	}
}

func responsesProfileWebSocketFrames(t *testing.T, model string) ([]byte, []byte) {
	t.Helper()
	first := responsesProfileRequest(model, []any{responsesProfileMessage("ws-first")})
	delete(first, "conversation")
	first["type"] = "response.create"
	second := responsesProfileRequest(model, []any{
		responsesProfileMessage("ws-first"),
		map[string]any{"type": "function_call", "call_id": "call_ws_profile", "name": "lookup", "arguments": "{}"},
		map[string]any{"type": "function_call_output", "call_id": "call_ws_profile", "output": []any{}},
		responsesProfileMessage("ws-second"),
	})
	delete(second, "conversation")
	delete(second, "previous_response_id")
	second["type"] = "response.create"
	return mustRequestJSON(t, first), mustRequestJSON(t, second)
}

func dialResponsesProfileWebSocket(
	t *testing.T,
	public *httptest.Server,
	secret string,
	headers http.Header,
) *websocket.Conn {
	t.Helper()
	requestHeaders := http.Header{
		"Authorization": []string{"Bearer " + secret},
		"Openai-Beta":   []string{"responses_websockets=2026-02-06"},
	}
	for name, values := range headers {
		for _, value := range values {
			requestHeaders.Add(name, value)
		}
	}
	connection, response, err := websocket.Dial(context.Background(), public.URL+"/v1/responses", &websocket.DialOptions{HTTPHeader: requestHeaders})
	if err != nil {
		t.Fatal(err)
	}
	if response.StatusCode != http.StatusSwitchingProtocols {
		t.Fatalf("WebSocket handshake = %d", response.StatusCode)
	}
	return connection
}

func writeResponsesProfileEvents(connection *websocket.Conn, responseID string, hidden bool) {
	events := [][]byte{mustRequestJSONValue(map[string]any{
		"type": "response.created", "response": map[string]any{"id": responseID},
	})}
	if !hidden {
		events = append(events, mustRequestJSONValue(map[string]any{
			"type": "response.output_item.done",
			"item": map[string]any{"type": "message", "role": "assistant", "content": []any{}},
		}))
	}
	events = append(events, mustRequestJSONValue(map[string]any{
		"type": "response.completed", "response": map[string]any{
			"id": responseID, "usage": map[string]any{"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
		},
	}))
	for _, event := range events {
		_ = connection.Write(context.Background(), websocket.MessageText, event)
	}
}

func responsesProfileResponseID(number int) string {
	identifier, _ := json.Marshal(number)
	return "resp_profile_" + string(identifier)
}

func isSyntheticResponsesProfilePrewarm(payload []byte) bool {
	var value map[string]any
	if json.Unmarshal(payload, &value) != nil || value["generate"] != false {
		return false
	}
	for _, carrier := range []string{"previous_response_id", "conversation"} {
		if _, exists := value[carrier]; exists {
			return false
		}
	}
	return true
}

func mustRequestJSONValue(value any) []byte {
	encoded, err := json.Marshal(value)
	if err != nil {
		panic(err)
	}
	return encoded
}

func readResponsesProfileTerminalEvents(t *testing.T, connection *websocket.Conn) {
	t.Helper()
	for index := 0; index < 3; index++ {
		message := readE2EWebSocketText(t, connection)
		if index == 2 && !bytes.Contains([]byte(message), []byte("response.completed")) {
			t.Fatal("WebSocket turn did not complete")
		}
	}
}

func waitForResponsesProfileWebSocketCaptures(
	t *testing.T,
	captures <-chan responsesProfileWebSocketCapture,
	want int,
) []responsesProfileWebSocketCapture {
	t.Helper()
	result := make([]responsesProfileWebSocketCapture, 0, want)
	for len(result) < want {
		select {
		case capture := <-captures:
			result = append(result, capture)
		case <-time.After(2 * time.Second):
			t.Fatal("expected loopback WebSocket frames were not captured")
		}
	}
	return result
}

func assertResponsesProfilePrewarm(t *testing.T, capture responsesProfileWebSocketCapture, lite bool) {
	t.Helper()
	value := decodeResponsesProfileWebSocketFrame(t, capture.Frame)
	assertResponsesProfileSurface(t, value, lite)
	if value["type"] != "response.create" || value["generate"] != false {
		t.Fatal("synthetic prewarm did not use response.create generate=false")
	}
	input, ok := value["input"].([]any)
	if !ok || (!lite && len(input) != 0) || (lite && len(input) == 0) {
		t.Fatal("synthetic prewarm input did not match the profile")
	}
}

func assertWebSocketProfileCredentialBoundary(
	t *testing.T,
	capture responsesProfileWebSocketCapture,
	subscription bool,
) {
	t.Helper()
	if capture.Headers.Get("Content-Encoding") != "" {
		t.Fatal("WebSocket application frames used HTTP zstd compression")
	}
	if subscription {
		if capture.Headers.Get("ChatGPT-Account-ID") == "" || capture.Headers.Get("Originator") != "codex_cli_rs" {
			t.Fatal("subscription WebSocket profile lost canonical credential identity")
		}
		return
	}
	if capture.Headers.Get("Authorization") != "Bearer "+upstreamAPIKey || capture.Headers.Get("ChatGPT-Account-ID") != "" {
		t.Fatal("API-key WebSocket profile crossed a subscription credential boundary")
	}
}

func decodeResponsesProfileWebSocketFrame(t *testing.T, frame []byte) map[string]any {
	t.Helper()
	var value map[string]any
	if err := json.Unmarshal(frame, &value); err != nil {
		t.Fatal(err)
	}
	return value
}

func boolToInt(value bool) int {
	if value {
		return 1
	}
	return 0
}

func containsResponsesProfileRole(input any, role string) bool {
	items, ok := input.([]any)
	if !ok {
		return false
	}
	for _, item := range items {
		object, ok := item.(map[string]any)
		if ok && object["role"] == role {
			return true
		}
	}
	return false
}
