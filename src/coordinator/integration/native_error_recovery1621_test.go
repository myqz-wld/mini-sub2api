//go:build nativeparity

package integration

import (
	"encoding/json"
	"fmt"
	"net/http"
	"sync/atomic"
	"testing"
)

// Exercise the real consumer: context exhaustion must reach its typed error and mark
// the context full, allowing native compaction on the next turn without a gateway replay.
func TestNative1621ContextFailureRecovery(t *testing.T) {
	for _, mode := range []string{"http400", "standalone_error"} {
		t.Run(mode, func(t *testing.T) {
			var sends, compactions atomic.Int32
			fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, body []byte) (string, string) {
				n := sends.Add(1)
				id := fmt.Sprintf("resp_context_recovery_%d", n)
				if n == 2 {
					failure := map[string]any{"code": "context_length_exceeded", "message": "synthetic-private-context-error"}
					if mode == "http400" {
						w.Header().Set("Content-Type", "application/json")
						w.WriteHeader(http.StatusBadRequest)
						_ = json.NewEncoder(w).Encode(map[string]any{"error": failure})
					} else {
						w.Header().Set("Content-Type", "text/event-stream")
						_, _ = fmt.Fprintf(w, "data: %s\n\ndata: [DONE]\n\n", mustRequestJSONValue(map[string]any{"type": "error", "error": failure}))
					}
					return "", ""
				}
				request := decodeRequestObject(t, body)
				compact := false
				if metadata, ok := request["client_metadata"].(map[string]any); ok {
					if encoded, ok := metadata["x-codex-turn-metadata"].(string); ok {
						var turn map[string]any
						_ = json.Unmarshal([]byte(encoded), &turn)
						compact = turn["request_kind"] == "compaction"
					}
				}
				for _, raw := range request["input"].([]any) {
					item, _ := raw.(map[string]any)
					compact = compact || item["type"] == "compaction_trigger"
				}
				var item map[string]any
				if compact {
					compactions.Add(1)
					item = map[string]any{"type": "compaction", "id": "cmp_context_recovery", "encrypted_content": "synthetic-compact-state"}
				} else {
					item = map[string]any{"type": "message", "id": "msg_" + id, "role": "assistant", "phase": "final_answer", "content": []any{map[string]any{"type": "output_text", "text": "synthetic completed answer"}}}
				}
				w.Header().Set("Content-Type", "text/event-stream")
				for _, event := range []map[string]any{
					{"type": "response.created", "response": map[string]any{"id": id}},
					{"type": "response.output_item.done", "output_index": 0, "item": item},
					{"type": "response.completed", "response": map[string]any{"id": id, "output": []any{item}, "usage": map[string]any{"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}},
				} {
					_, _ = fmt.Fprintf(w, "data: %s\n\n", mustRequestJSONValue(event))
				}
				return id, ""
			})
			contextError := false
			options := nativeOptions{endpoint: fixture.public.URL, bearer: fixture.subscriptionKey, model: "gpt-5.5", base: "synthetic context recovery", neutralPersonality: true,
				configOverrides: map[string]string{"features.remote_compaction_v2": "true", "features.token_budget": "false", "model_providers.native_capture.stream_max_retries": "1"},
				observe: func(event map[string]any) {
					params, _ := event["params"].(map[string]any)
					failure, _ := params["error"].(map[string]any)
					contextError = contextError || failure["codexErrorInfo"] == "contextWindowExceeded"
				}}
			client := startNativeClient(t, options)
			thread := client.thread(options)
			client.turn(thread, "Seed a completed conversation.")
			turn := failureStartTurn(t, client, thread, "Exercise the context limit.")
			if failureWaitTurn(t, client, thread, turn) != "failed" || !contextError || sends.Load() != 2 {
				t.Fatalf("native context classification/replay: typed=%t sends=%d", contextError, sends.Load())
			}
			client.turn(thread, "Continue after the context limit.")
			if compactions.Load() != 1 || sends.Load() != 4 {
				t.Fatalf("native context recovery: compactions=%d sends=%d", compactions.Load(), sends.Load())
			}
		})
	}
}

// The synthesized failed event must make envelope-only advice visible to the real
// native consumer. Without advice overload is terminal; valid advice permits one retry.
func TestNative1621StandaloneOverloadRetryAdvice(t *testing.T) {
	for _, mode := range []string{"no_advice", "top_only", "invalid_nested"} {
		t.Run(mode, func(t *testing.T) {
			var sends atomic.Int32
			fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
				n := sends.Add(1)
				id := fmt.Sprintf("resp_overload_recovery_%d", n)
				w.Header().Set("Content-Type", "text/event-stream")
				if n == 1 {
					failure := map[string]any{"code": "server_is_overloaded", "message": "synthetic-private-overload"}
					event := map[string]any{"type": "error", "error": failure}
					if mode != "no_advice" {
						event["headers"] = map[string]any{"Retry-After": "0"}
					}
					if mode == "invalid_nested" {
						failure["headers"] = map[string]any{"Retry-After": "invalid"}
					}
					_, _ = fmt.Fprintf(w, "data: %s\n\ndata: [DONE]\n\n", mustRequestJSONValue(event))
					return "", ""
				}
				item := map[string]any{"type": "message", "id": "msg_" + id, "role": "assistant", "phase": "final_answer", "content": []any{map[string]any{"type": "output_text", "text": "synthetic retry completed"}}}
				for _, event := range []map[string]any{
					{"type": "response.created", "response": map[string]any{"id": id}},
					{"type": "response.output_item.done", "output_index": 0, "item": item},
					{"type": "response.completed", "response": map[string]any{"id": id, "output": []any{item}}},
				} {
					_, _ = fmt.Fprintf(w, "data: %s\n\n", mustRequestJSONValue(event))
				}
				return id, ""
			})
			options := nativeOptions{endpoint: fixture.public.URL, bearer: fixture.subscriptionKey, model: "gpt-5.5", base: "synthetic overload retry", neutralPersonality: true,
				configOverrides: map[string]string{"model_providers.native_capture.stream_max_retries": "1"}}
			client := startNativeClient(t, options)
			thread := client.thread(options)
			turn := failureStartTurn(t, client, thread, "Reply once.")
			status := failureWaitTurn(t, client, thread, turn)
			wantStatus, wantSends := "failed", int32(1)
			if mode != "no_advice" {
				wantStatus, wantSends = "completed", 2
			}
			if status != wantStatus || sends.Load() != wantSends {
				t.Fatalf("native overload advice: status=%s sends=%d want=%s/%d", status, sends.Load(), wantStatus, wantSends)
			}
		})
	}
}
