package integration

import (
	"context"
	"fmt"
	"net/http"
	"reflect"
	"testing"

	"github.com/coder/websocket"
)

type fidelityPeer struct {
	send func(map[string]any, http.Header) (map[string]any, http.Header, map[string]any)
}

func newFidelityPeer(t *testing.T, ws bool) fidelityPeer {
	t.Helper()
	events := func(id string) []map[string]any {
		item := map[string]any{"type": "message", "id": "msg_" + id, "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": "synthetic answer"}}}
		return []map[string]any{
			{"type": "response.created", "response": map[string]any{"id": id}},
			{"type": "response.output_item.done", "output_index": 0, "item": item},
			{"type": "response.completed", "response": map[string]any{"id": id, "status": "completed", "output": []any{item}}},
		}
	}
	if !ws {
		fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
			id := fmt.Sprintf("resp_followup_%d", loopbackResponseSequence.Add(1))
			w.Header().Set("Content-Type", "text/event-stream")
			for _, event := range events(id) {
				fmt.Fprintf(w, "data: %s\n\n", mustRequestJSON(t, event))
			}
			return id, "synthetic-provider"
		})
		return fidelityPeer{send: func(body map[string]any, headers http.Header) (map[string]any, http.Header, map[string]any) {
			t.Helper()
			status, response, _ := publicRequestWithHeaders(t, fixture.public, fixture.subscriptionKey, string(mustRequestJSON(t, body)), headers)
			if status != http.StatusOK {
				t.Fatalf("HTTP admission failed: %d", status)
			}
			capture := waitForRoutingCapture(t, fixture.captures)
			return decodeRequestObject(t, capture.Body), capture.Headers, decodeRequestObject(t, []byte(response))
		}}
	}
	fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(conn *websocket.Conn, _ []byte, id string) {
		for _, event := range events(id) {
			_ = conn.Write(context.Background(), websocket.MessageText, mustRequestJSON(t, event))
		}
	})
	return fidelityPeer{send: func(body map[string]any, headers http.Header) (map[string]any, http.Header, map[string]any) {
		t.Helper()
		if headers == nil {
			headers = http.Header{}
		} else {
			headers = headers.Clone()
		}
		if _, explicit := headers["Originator"]; !explicit {
			headers.Set("Originator", "codex_exec")
		}
		conn, _ := dialResponsesProfileWebSocketWithResponse(t, fixture.public, fixture.subscriptionKey, headers)
		defer conn.CloseNow()
		body["type"] = "response.create"
		writeE2EWebSocketText(t, conn, string(mustRequestJSON(t, body)))
		response := historyImportCompleted(t, readResponsesProfileTerminalEvents(t, conn))
		capture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
		return decodeRequestObject(t, capture.Frame), capture.Headers, response
	}}
}

func fidelityBody(input ...any) map[string]any {
	return map[string]any{"model": "gpt-5.4", "input": input}
}
func fidelityUser() map[string]any {
	return map[string]any{"role": "user", "content": "synthetic input"}
}
func fidelityItems(wire map[string]any) []any { return wire["input"].([]any) }

func TestFidelityFollowupContentAndTypedControls(t *testing.T) {
	for _, ws := range []bool{false, true} {
		t.Run(fmt.Sprintf("ws=%v", ws), func(t *testing.T) {
			peer := newFidelityPeer(t, ws)
			for _, custom := range []bool{false, true} {
				for _, mixed := range []bool{false, true} {
					kind, outputKind := "function_call", "function_call_output"
					if custom {
						kind, outputKind = "custom_tool_call", "custom_tool_call_output"
					}
					call := map[string]any{"type": kind, "name": "synthetic", "call_id": "call_encrypted", "arguments": "{}"}
					if custom {
						delete(call, "arguments")
						call["input"] = "synthetic"
					}
					content := []any{map[string]any{"type": "encrypted_content", "encrypted_content": "synthetic-ciphertext"}}
					if mixed {
						content = append([]any{map[string]any{"type": "input_text", "text": "synthetic text"}}, content...)
					}
					wire, _, _ := peer.send(fidelityBody(fidelityUser(), call, map[string]any{"type": outputKind, "call_id": "call_encrypted", "output": content}), nil)
					items := fidelityItems(wire)
					if !reflect.DeepEqual(items[2].(map[string]any)["output"], content) {
						t.Fatal("encrypted tool result lost")
					}
					if items[1].(map[string]any)["call_id"] != items[2].(map[string]any)["call_id"] {
						t.Fatal("call pairing changed")
					}
				}
			}
			for _, kind := range []string{"message", "agent_message"} {
				item := map[string]any{"type": kind, "role": "user", "author": "synthetic", "recipient": "all", "content": []any{map[string]any{"type": "encrypted_content", "encrypted_content": "synthetic-ciphertext"}, map[string]any{"type": "input_text", "text": "kept"}}}
				wire, _, _ := peer.send(fidelityBody(item), nil)
				content := fidelityItems(wire)[0].(map[string]any)["content"].([]any)
				want := 1
				if kind == "agent_message" {
					want = 2
				}
				if len(content) != want {
					t.Fatal("message and agent content enums conflated")
				}
			}
			for _, content := range []any{[]any{}, nil, []any{map[string]any{"type": "text", "text": "hidden"}}, []any{map[string]any{"type": "text", "text": "kept"}, map[string]any{"type": "reasoning_text", "text": "kept"}}} {
				wire, _, _ := peer.send(fidelityBody(fidelityUser(), map[string]any{"type": "reasoning", "id": "rs_synthetic", "summary": []any{}, "content": content, "encrypted_content": "synthetic-ciphertext"}), nil)
				got, present := fidelityItems(wire)[1].(map[string]any)["content"]
				list, isList := content.([]any)
				if isList && len(list) < 2 {
					if present {
						t.Fatal("reasoning content should be omitted")
					}
				} else if !present || !reflect.DeepEqual(got, content) {
					t.Fatal("reasoning null or mixed content changed")
				}
			}
			for _, bad := range []any{nil, 17, []any{}, true, map[string]any{}} {
				body := fidelityBody(fidelityUser())
				body["reasoning"] = map[string]any{"effort": bad}
				body["text"] = bad
				body["parallel_tool_calls"] = "yes"
				wire, _, _ := peer.send(body, nil)
				if _, ok := wire["parallel_tool_calls"].(bool); !ok {
					t.Fatal("invalid parallel flag reached peer")
				}
				if wire["reasoning"].(map[string]any)["effort"] != "medium" {
					t.Fatal("invalid effort did not use model default")
				}
				if wire["text"].(map[string]any)["verbosity"] != "low" {
					t.Fatal("invalid text did not use model default")
				}
			}
			body := fidelityBody(fidelityUser(), map[string]any{"type": "function_call", "call_id": "call_opaque", "name": "opaque", "arguments": `{"effort":17,"text":[],"instructions":" "}`}, map[string]any{"type": "function_call_output", "call_id": "call_opaque", "output": `{"parallel_tool_calls":"yes"}`})
			body["reasoning"] = map[string]any{"effort": "future-custom-effort"}
			body["text"] = map[string]any{"verbosity": nil}
			wire, _, _ := peer.send(body, nil)
			if wire["reasoning"].(map[string]any)["effort"] != "future-custom-effort" {
				t.Fatal("custom effort removed")
			}
			for _, index := range []int{1, 2} {
				field := "arguments"
				if index == 2 {
					field = "output"
				}
				if fidelityItems(wire)[index].(map[string]any)[field] != body["input"].([]any)[index].(map[string]any)[field] {
					t.Fatal("opaque business data changed")
				}
			}
		})
	}
}
