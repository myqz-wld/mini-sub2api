//go:build nativeparity

package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"testing"
	"time"
)

func TestNativeScenarioCompactionOrdinaryRebuild(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("subscription=%t/ws=%t/%s", subscription, ws, model), func(t *testing.T) {
					capture := newNativeCompactionCapture(t, "v2", false)
					gateway := newNativeGateway(t, capture.server.URL, subscription)
					client := ordinaryClient(t, gateway, ws, true, nil)
					seed := map[string]any{"type": "message", "role": "user", "content": []any{map[string]any{"type": "input_text", "text": "synthetic ordinary compaction seed"}}}
					request := ordinaryCompactionRequest(model, []any{seed, map[string]any{"type": "compaction_trigger"}}, nil)
					metadata := map[string]any{"session_id": "ordinary-compaction-session", "thread_id": "ordinary-compaction-session", "turn_id": "ordinary-compaction-turn", "request_kind": "compaction", "compaction": map[string]any{"trigger": "manual", "reason": "user_requested", "implementation": "responses_compaction_v2", "phase": "standalone_turn", "strategy": "memento"}}
					request["client_metadata"] = map[string]any{"session_id": "ordinary-compaction-session", "x-codex-turn-metadata": string(mustRequestJSON(t, metadata))}
					compacted := client.send(request)
					output, _ := compacted["output"].([]any)
					if len(output) != 1 || output[0].(map[string]any)["type"] != "compaction" {
						t.Fatal("ordinary compaction output changed")
					}
					next := map[string]any{"type": "message", "role": "user", "content": []any{map[string]any{"type": "input_text", "text": "synthetic ordinary postcompaction user"}}}
					delta := ordinaryCompactionRequest(model, []any{next}, compacted["id"])
					client.send(delta)
					continued := businessWires(capture.snapshot())
					lastDelta := continued[len(continued)-1]
					if subscription && !ws {
						types, texts := compactionItemCounts(lastDelta)
						if lastDelta.value["previous_response_id"] != nil || types["compaction"] != 1 || types["compaction_trigger"] != 0 || texts["synthetic ordinary compaction seed"] != 1 || texts["synthetic ordinary postcompaction user"] != 1 {
							t.Fatal("HTTP compaction reference did not reconstruct its replacement window")
						}
					} else {
						if lastDelta.value["previous_response_id"] == nil {
							t.Fatal("valid upstream continuation was unnecessarily expanded")
						}
					}
					// A caller remains free to install its own complete replacement window.
					full := ordinaryCompactionRequest(model, []any{seed, output[0], next}, nil)
					full["client_metadata"] = map[string]any{"session_id": "ordinary-compaction-session"}
					rebuilt := client.send(full)
					httpClient := ordinaryClient(t, gateway, false, true, nil)
					lastUser := map[string]any{"type": "message", "role": "user", "content": []any{map[string]any{"type": "input_text", "text": "synthetic ordinary reconstructed suffix"}}}
					httpClient.send(ordinaryCompactionRequest(model, []any{lastUser}, rebuilt["id"]))
					wires := businessWires(capture.snapshot())
					last := wires[len(wires)-1]
					if subscription {
						if last.value["previous_response_id"] != nil {
							t.Fatal("rebuilt HTTP request remained incremental")
						}
						types, texts := compactionItemCounts(last)
						if types["compaction"] != 1 || types["compaction_trigger"] != 0 || texts["synthetic ordinary compaction seed"] != 1 || texts["synthetic ordinary postcompaction user"] != 1 || texts["synthetic ordinary reconstructed suffix"] != 1 {
							t.Fatal("rebuild restored incorrect compacted history")
						}
						input := last.value["input"].([]any)
						for _, raw := range input {
							item, _ := raw.(map[string]any)
							if item["type"] == "compaction" && item["encrypted_content"] != "synthetic_compacted_state" {
								t.Fatal("reconstruction changed opaque compaction content")
							}
						}
					} else {
						if last.value["previous_response_id"] == nil || len(last.value["input"].([]any)) != 1 {
							t.Fatal("API-key compaction history was reconstructed")
						}
						packets := gateway.tap.packets(t)
						all := capture.snapshot()
						if len(packets) != len(all) {
							t.Fatal("API-key ordinary compaction capture count")
						}
						for i, wire := range all {
							if !bytes.Equal(packets[i].payload, wire.encodedBody) {
								t.Fatal("API-key ordinary compaction request bytes changed")
							}
						}
					}
					t.Logf("ordinary compaction policy: subscription=%t initial_ws=%t upstream_reference_then_client_full_rebuild=true", subscription, ws)
				})
			}
		}
	}
}

func ordinaryCompactionRequest(model string, input []any, previous any) map[string]any {
	request := map[string]any{"model": model, "instructions": "synthetic ordinary compaction base", "tools": []any{}, "input": input}
	if previous != nil {
		request["previous_response_id"] = previous
	}
	return request
}

func compactionHTTPError(t *testing.T, gateway nativeGateway, request map[string]any) (int, string) {
	t.Helper()
	request["stream"] = false
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	req, _ := http.NewRequestWithContext(ctx, http.MethodPost, gateway.server.URL+"/v1/responses", bytes.NewReader(mustRequestJSON(t, request)))
	req.Header.Set("Authorization", "Bearer "+gateway.secret)
	req.Header.Set("Content-Type", "application/json")
	response, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal("ordinary compaction HTTP probe")
	}
	defer response.Body.Close()
	body, err := io.ReadAll(io.LimitReader(response.Body, 1<<20))
	if err != nil {
		t.Fatal("ordinary compaction error read")
	}
	var value map[string]any
	if json.Unmarshal(body, &value) != nil {
		t.Fatal("ordinary compaction error JSON")
	}
	failure, _ := value["error"].(map[string]any)
	code, _ := failure["code"].(string)
	return response.StatusCode, code
}
