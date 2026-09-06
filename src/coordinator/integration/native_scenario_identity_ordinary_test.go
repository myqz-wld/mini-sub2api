//go:build nativeparity

package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
)

const identityRoot = "01930000-0000-7000-8000-000000000001"
const identityChild = "01930000-0000-7000-8000-000000000002"
const identityFork = "01930000-0000-7000-8000-000000000003"
const identityRootTurn = "01940000-0000-7000-8000-000000000001"
const identityChildTurn = "01940000-0000-7000-8000-000000000002"
const identityForkTurn = "01940000-0000-7000-8000-000000000003"

// The result is an HTTP status or a typed WS terminal. Error payloads remain in memory.
func identitySend(t *testing.T, gateway nativeGateway, ws bool, headers http.Header, body map[string]any) (int, string) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
	defer cancel()
	if headers == nil {
		headers = make(http.Header)
	} else {
		headers = headers.Clone()
	}
	headers.Set("Authorization", "Bearer "+gateway.secret)
	headers.Set("User-Agent", "OpenAI/Go synthetic identity")
	body["stream"] = false
	if ws {
		connection, response, err := websocket.Dial(ctx, strings.Replace(gateway.server.URL, "http", "ws", 1)+"/v1/responses", &websocket.DialOptions{HTTPHeader: headers})
		if err != nil {
			if response != nil {
				return response.StatusCode, "handshake_error"
			}
			t.Fatal("identity ordinary websocket handshake")
		}
		defer connection.CloseNow()
		body["type"] = "response.create"
		if connection.Write(ctx, websocket.MessageText, mustRequestJSON(t, body)) != nil {
			t.Fatal("identity ordinary websocket write")
		}
		for {
			_, data, err := connection.Read(ctx)
			if err != nil {
				if closeStatus := websocket.CloseStatus(err); closeStatus != -1 {
					return 400, fmt.Sprintf("ws_close_%d", closeStatus)
				}
				t.Fatal("identity ordinary websocket terminal missing")
			}
			var event map[string]any
			if json.Unmarshal(data, &event) != nil {
				t.Fatal("identity ordinary invalid websocket JSON")
			}
			typeName, _ := event["type"].(string)
			if typeName == "response.completed" {
				return 200, typeName
			}
			if typeName == "error" || typeName == "response.failed" {
				return 400, typeName
			}
		}
	}
	request, _ := http.NewRequestWithContext(ctx, http.MethodPost, gateway.server.URL+"/v1/responses", bytes.NewReader(mustRequestJSON(t, body)))
	request.Header = headers
	request.Header.Set("Content-Type", "application/json")
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal("identity ordinary HTTP transport")
	}
	defer response.Body.Close()
	_, err = io.Copy(io.Discard, io.LimitReader(response.Body, nativeCaptureLimit+1))
	if err != nil {
		t.Fatal("identity ordinary HTTP body read")
	}
	return response.StatusCode, "http"
}

func identityRequest(model, session, thread, turn, parent, fork string, historicalTurn string) map[string]any {
	rootTurn := turn
	nested := map[string]any{"session_id": session, "thread_id": thread, "turn_id": turn, "root_turn_id": rootTurn, "window_id": thread + ":0", "window_number": 0, "request_kind": "turn"}
	flat := map[string]any{"session_id": session, "thread_id": thread, "turn_id": turn, "root_turn_id": rootTurn, "x-codex-window-id": thread + ":0"}
	if parent != "" {
		nested["parent_thread_id"], nested["parent_turn_id"], nested["root_turn_id"] = parent, identityRootTurn, identityRootTurn
		flat["x-codex-parent-thread-id"], flat["parent_turn_id"], flat["root_turn_id"] = parent, identityRootTurn, identityRootTurn
	}
	if fork != "" {
		nested["forked_from_thread_id"] = fork
	}
	nestedJSON, _ := json.Marshal(nested)
	flat["x-codex-turn-metadata"] = string(nestedJSON)
	input := []any{}
	if historicalTurn != "" {
		input = append(input, map[string]any{"type": "message", "role": "user", "id": "msg_identity_history", "content": []any{map[string]any{"type": "input_text", "text": "synthetic inherited history"}}, "internal_chat_message_metadata_passthrough": map[string]any{"turn_id": historicalTurn}})
	}
	input = append(input, map[string]any{"type": "message", "role": "user", "id": "msg_identity_current", "content": []any{map[string]any{"type": "input_text", "text": "synthetic current identity"}}, "internal_chat_message_metadata_passthrough": map[string]any{"turn_id": turn}})
	return map[string]any{"model": model, "instructions": "synthetic identity base", "tools": []any{}, "input": input, "client_metadata": flat}
}

func TestNativeScenarioIdentityHeaderOnlyObservedMismatch(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				for _, child := range []bool{false, true} {
					t.Run(fmt.Sprintf("subscription=%t/ws=%t/%s/child=%t", subscription, ws, model, child), func(t *testing.T) {
						capture := newNativeIdentityCapture(t, "disabled")
						gateway := newNativeGateway(t, capture.server.URL, subscription)
						thread := identityRoot
						headers := http.Header{"Session-Id": []string{identityRoot}}
						if child {
							thread = identityChild
							headers.Set("Thread-Id", thread)
							headers.Set("X-Codex-Parent-Thread-Id", identityRoot)
						}
						headers.Set("X-Codex-Window-Id", thread+":9")
						body := identityRequest(model, identityRoot, thread, identityChildTurn, "", "", "")
						delete(body, "client_metadata")
						status, _ := identitySend(t, gateway, ws, headers, body)
						wires := businessWires(capture.snapshot())
						if status != 200 || len(wires) != 1 {
							t.Fatal("header-only scenario did not reach one inference")
						}
						wire := wires[0]
						if !subscription {
							if wire.headers.Get("X-Codex-Window-Id") != thread+":9" || wire.headers.Get("X-Codex-Parent-Thread-Id") != headers.Get("X-Codex-Parent-Thread-Id") || wire.value["client_metadata"] != nil {
								t.Fatal("API-key header-only control changed")
							}
						} else {
							flat, nested := identityMetadata(t, wire)
							window, _ := nested["window_id"].(string)
							if flat["session_id"] != flat["thread_id"] || nested["parent_thread_id"] != nil || !strings.HasSuffix(window, ":0") || wire.headers.Get("X-Codex-Parent-Thread-Id") != "" {
								t.Fatal("G-B baseline behavior changed; reassess conformance")
							}
						}
						t.Logf("G-B source-backed ordinary case: accepted=true requested_window=9 emitted_window_preserved=%t requested_child=%t child_preserved=%t", !subscription, child, child && !subscription)
					})
				}
			}
		}
	}
}

func TestNativeScenarioIdentityRootForkObservedMismatch(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				for _, mode := range []string{"mapped", "unseen", "no_fork"} {
					t.Run(fmt.Sprintf("subscription=%t/ws=%t/%s/%s", subscription, ws, model, mode), func(t *testing.T) {
						capture := newNativeIdentityCapture(t, "disabled")
						gateway := newNativeGateway(t, capture.server.URL, subscription)
						if mode == "mapped" {
							status, _ := identitySend(t, gateway, ws, nil, identityRequest(model, identityRoot, identityRoot, identityRootTurn, "", "", ""))
							if status != 200 {
								t.Fatal("root fork source seed rejected")
							}
						}
						before := len(businessWires(capture.snapshot()))
						source := identityRoot
						if mode == "no_fork" {
							source = ""
						}
						status, terminal := identitySend(t, gateway, ws, nil, identityRequest(model, identityFork, identityFork, identityForkTurn, "", source, ""))
						reached := len(businessWires(capture.snapshot())) > before
						mismatch := subscription && source != ""
						if mismatch && (status != 400 || reached) || !mismatch && (status != 200 || !reached) {
							t.Fatal("G-C baseline/control changed; reassess conformance")
						}
						t.Logf("G-C source-backed root fork: status=%d terminal=%s inference_reached=%t conformance_mismatch=%t", status, terminal, reached, mismatch)
					})
				}
			}
		}
	}
}
