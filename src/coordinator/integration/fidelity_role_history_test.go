package integration

import (
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"testing"
)

func fidelityMetadata(t *testing.T, wire map[string]any) (map[string]any, map[string]any) {
	t.Helper()
	flat := wire["client_metadata"].(map[string]any)
	var turn map[string]any
	if json.Unmarshal([]byte(flat["x-codex-turn-metadata"].(string)), &turn) != nil {
		t.Fatal("invalid turn metadata")
	}
	return flat, turn
}

func TestFidelityFollowupRolesAndExplicitReplay(t *testing.T) {
	for _, ws := range []bool{false, true} {
		t.Run(fmt.Sprintf("ws=%v", ws), func(t *testing.T) {
			peer := newFidelityPeer(t, ws)
			parent := fidelityBody(fidelityUser())
			parent["client_metadata"] = map[string]any{"session_id": "source-session", "thread_id": "source-session", "turn_id": "source-turn"}
			root, _, response := peer.send(parent, nil)
			flatRoot, _ := fidelityMetadata(t, root)
			for index := 0; index < 2; index++ {
				child := fidelityBody(
					map[string]any{"type": "additional_tools", "id": "at_classifier", "role": "developer", "tools": []any{}},
					map[string]any{"type": "message", "id": "msg_classifier_base", "role": "developer", "content": []any{map[string]any{"type": "input_text", "text": "synthetic classifier"}}}, fidelityUser())
				child["model"] = "gpt-5.6-luna"
				child["prompt_cache_key"] = "guardian-v2:source-session"
				child["client_metadata"] = map[string]any{"session_id": "source-session", "thread_id": fmt.Sprintf("classifier-%d", index), "turn_id": fmt.Sprintf("classification-%d", index), "parent_turn_id": "source-turn", "root_turn_id": "source-turn", "parent_response_id": response["id"], "x-openai-subagent": "guardian", "x-codex-turn-metadata": string(mustRequestJSON(t, map[string]any{"session_id": "source-session", "thread_id": fmt.Sprintf("classifier-%d", index), "turn_id": fmt.Sprintf("classification-%d", index), "parent_turn_id": "source-turn", "root_turn_id": "source-turn", "guardian_classifier_source_thread_id": "source-session", "thread_source": "guardian_classifier", "turn_trigger": "guardian_classifier"}))}
				callerHeaders := http.Header{"X-Codex-Guardian": []string{"classifier"}, "X-Openai-Subagent": []string{"guardian"}}
				if index == 1 {
					callerHeaders["Originator"] = []string{""}
					meta, turn := fidelityMetadata(t, child)
					for _, field := range []string{"x-codex-ws-stream-request-start-ms", "ws_request_header_traceparent", "ws_request_header_tracestate"} {
						meta[field] = "synthetic-role-incompatible"
					}
					delete(meta, "root_turn_id")
					delete(turn, "root_turn_id")
					meta["x-codex-turn-metadata"] = string(mustRequestJSON(t, turn))
				}
				wire, headers, _ := peer.send(child, callerHeaders)
				flat, turn := fidelityMetadata(t, wire)
				if index == 1 && (flat["root_turn_id"] != nil || turn["root_turn_id"] != nil) {
					t.Fatal("classifier invented omitted root identity")
				}
				if wire["prompt_cache_key"] != "guardian-v2:"+flatRoot["thread_id"].(string) || turn["guardian_classifier_source_thread_id"] != flatRoot["thread_id"] || turn["parent_turn_id"] != flatRoot["turn_id"] {
					t.Fatal("classifier source/cache/parent mapping diverged")
				}
				if flat["session_id"] != flatRoot["session_id"] || flat["thread_id"] == flatRoot["thread_id"] || flat["turn_id"] == flatRoot["turn_id"] {
					t.Fatal("classifier identity ownership conflated")
				}
				for _, field := range []string{"installation_id", "request_kind", "window_id", "parent_thread_id", "sandbox", "turn_started_at_unix_ms"} {
					if _, ok := turn[field]; ok {
						t.Fatalf("classifier inner field %s was synthesized", field)
					}
				}
				for _, field := range []string{"x-codex-installation-id", "guardian_credits_requested", "parent_thread_id", "x-codex-turn-state", "x-codex-ws-stream-request-start-ms", "ws_request_header_traceparent", "ws_request_header_tracestate"} {
					if _, ok := flat[field]; ok {
						t.Fatalf("classifier flat field %s was synthesized", field)
					}
				}
				for _, item := range fidelityItems(wire) {
					if _, ok := item.(map[string]any)["internal_chat_message_metadata_passthrough"]; ok {
						t.Fatal("classifier input metadata synthesized")
					}
				}
				if headers.Get("Content-Encoding") != "" || headers.Get("X-Openai-Internal-Codex-Responses-Lite") != "true" {
					t.Fatal("classifier transport policy lost")
				}
				if headers.Get("X-Codex-Turn-Metadata") != "" || headers.Get("X-Codex-Routing-Hint") != "" || headers.Get("X-Codex-Beta-Features") != "" {
					t.Fatal("ordinary headers leaked into classifier")
				}
				if _, ok := wire["text"]; ok {
					t.Fatal("classifier text synthesized")
				}
			}
			for _, strict := range []bool{true, false} {
				body := fidelityBody(fidelityUser())
				body["text"] = map[string]any{"format": map[string]any{"type": "json_schema", "strict": strict, "schema": map[string]any{"type": "object"}}}
				wire, _, _ := peer.send(body, http.Header{"X-Codex-Guardian": []string{"reviewer"}})
				if wire["text"].(map[string]any)["format"].(map[string]any)["strict"] != false {
					t.Fatal("basic reviewer strictness not fixed")
				}
			}
			memory := fidelityBody(fidelityUser())
			memory["client_metadata"] = map[string]any{"session_id": "memory-session", "thread_id": "memory-session", "x-codex-turn-metadata": `{"request_kind":"memory","turn_id":"memory-turn","root_turn_id":"memory-turn"}`}
			wire, _, _ := peer.send(memory, nil)
			flat, turn := fidelityMetadata(t, wire)
			for _, field := range []string{"installation_id", "session_id", "thread_id", "window_id"} {
				if _, ok := turn[field]; ok {
					t.Fatal("detached Memory has inner thread identity")
				}
			}
			if flat["session_id"] == nil || flat["thread_id"] == nil || turn["turn_id"] == nil || turn["root_turn_id"] == nil {
				t.Fatal("Memory flat identity or turn/root lost")
			}
			for _, explicit := range []bool{true, false} {
				body := fidelityBody(fidelityUser(), map[string]any{"type": "message", "id": "msg_provider_import", "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": "synthetic imported"}}},
					map[string]any{"type": "function_call", "id": "fc_provider_import", "call_id": "call_provider_import", "name": "synthetic", "arguments": "{}"},
					map[string]any{"type": "function_call_output", "call_id": "call_provider_import", "output": "synthetic"}, fidelityUser())
				if explicit {
					body["client_metadata"] = map[string]any{"session_id": "restored-session", "thread_id": "restored-session", "turn_id": "restored-turn"}
				}
				wire, _, reply := peer.send(body, nil)
				if fidelityItems(wire)[1].(map[string]any)["id"] != "msg_provider_import" {
					t.Fatal("full replay provider output ID changed")
				}
				if fidelityItems(wire)[2].(map[string]any)["id"] != "fc_provider_import" || fidelityItems(wire)[2].(map[string]any)["call_id"] != "call_provider_import" || fidelityItems(wire)[3].(map[string]any)["call_id"] != "call_provider_import" {
					t.Fatal("provider call identities or pairing changed")
				}
				body["input"] = append(body["input"].([]any), reply["output"].([]any)[0], fidelityUser())
				delete(body, "client_metadata")
				next, _, _ := peer.send(body, nil)
				found := false
				for _, item := range fidelityItems(next) {
					if item.(map[string]any)["id"] == "msg_"+reply["id"].(string) {
						t.Fatal("public response alias used as provider identity")
					}
					id, _ := item.(map[string]any)["id"].(string)
					if strings.HasPrefix(id, "msg_resp_") {
						found = true
					}
				}
				if !found {
					t.Fatal("known output alias was not reversed")
				}
			}
		})
	}
}

func TestFidelityFollowupWhitespaceInstructions(t *testing.T) {
	for _, ws := range []bool{false, true} {
		t.Run(fmt.Sprintf("ws=%v", ws), func(t *testing.T) {
			peer := newFidelityPeer(t, ws)
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				for _, base := range []string{"", " \t\r\n", "\u00a0\u3000", "ordinary"} {
					body := fidelityBody(fidelityUser())
					body["model"] = model
					body["instructions"] = base
					wire, _, _ := peer.send(body, nil)
					if model == "gpt-5.4" {
						got, ok := wire["instructions"]
						if (base == "" && ok) || (base != "" && got != base) {
							t.Fatal("ordinary base changed")
						}
					} else {
						items := fidelityItems(wire)
						want := 2
						if base != "" {
							want = 3
						}
						if len(items) != want {
							t.Fatal("Lite base missing")
						}
						if base != "" && items[1].(map[string]any)["content"].([]any)[0].(map[string]any)["text"] != base {
							t.Fatal("Lite base changed")
						}
					}
				}
			}
		})
	}
}
