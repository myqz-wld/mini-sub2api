package integration

import (
	"encoding/json"
	"fmt"
	"net/http"
	"reflect"
	"testing"
)

func agentMessageInput() map[string]any {
	return map[string]any{
		"type": "agent_message", "id": "amsg_synthetic_task",
		"author": "/root", "recipient": "/root/worker",
		"content": []any{
			map[string]any{"type": "input_text", "text": "Message Type: NEW_TASK\nTask name: /root/worker\nSender: /root\nPayload:\n"},
			map[string]any{"type": "encrypted_content", "encrypted_content": "synthetic-ciphertext"},
		},
	}
}

func agentHistoryInput(agent bool) []any {
	items := []any{fidelityUser()}
	if agent {
		items = append(items, agentMessageInput())
	}
	return items
}

func TestAgentMessageReplayIDs(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
			for _, agent := range []bool{false, true} {
				t.Run(fmt.Sprintf("ws=%t/%s/agent=%t", ws, model, agent), func(t *testing.T) {
					peer := newFidelityPeer(t, ws)
					input := append(agentHistoryInput(agent),
						map[string]any{"type": "reasoning", "id": "rs_synthetic_history", "summary": []any{}, "encrypted_content": "synthetic-reasoning"},
						map[string]any{"type": "message", "id": "msg_synthetic_history", "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": "synthetic historical answer"}}},
						fidelityUser())
					body := fidelityBody(input...)
					body["model"] = model
					wire, _, _ := peer.send(body, nil)
					for _, raw := range fidelityItems(wire) {
						item := raw.(map[string]any)
						if item["type"] == "agent_message" {
							if !reflect.DeepEqual(item["content"], agentMessageInput()["content"]) || item["author"] != "/root" || item["recipient"] != "/root/worker" {
								t.Error("agent content or routing changed")
							}
						}
						if item["type"] == "reasoning" {
							assertContentPseudonym(t, item["id"], "rs_synthetic_history", "rs_")
							if item["encrypted_content"] != "synthetic-reasoning" {
								t.Error("reasoning ciphertext changed")
							}
						}
						if item["role"] == "assistant" {
							assertContentPseudonym(t, item["id"], "msg_synthetic_history", "msg_")
						}
					}
				})
			}
		}
	}
}

func TestAgentMessageRestartReplay(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
		for _, explicit := range []bool{false, true} {
			for _, agent := range []bool{false, true} {
				t.Run(fmt.Sprintf("%s/explicit=%t/agent=%t", model, explicit, agent), func(t *testing.T) {
					fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, body []byte) (string, string) {
						id := fmt.Sprintf("resp_agent_restart_%d", loopbackResponseSequence.Add(1))
						w.Header().Set("Content-Type", "text/event-stream")
						for _, event := range historyImportEvents(id, historyImportTurn(t, body)) {
							fmt.Fprintf(w, "data: %s\n\n", mustRequestJSON(t, event))
						}
						return id, "synthetic-provider"
					})
					input := agentHistoryInput(agent)
					for step := 0; step < 3; step++ {
						if step == 2 {
							fixture.restartRuntime(t)
						}
						body := map[string]any{"model": model, "input": input}
						if explicit {
							body["client_metadata"] = map[string]any{"session_id": "synthetic-agent-session"}
						}
						status, raw, _ := publicRequestWithHeaders(t, fixture.public, fixture.subscriptionKey, string(mustRequestJSON(t, body)), nil)
						t.Logf("step=%d status=%d", step, status)
						if status != http.StatusOK {
							t.Fatal("complete history replay rejected")
						}
						waitForRoutingCapture(t, fixture.captures)
						response := map[string]any{}
						if json.Unmarshal([]byte(raw), &response) != nil {
							t.Fatal("invalid probe response")
						}
						input = append(input, response["output"].([]any)...)
						input = append(input, fidelityUser())
					}
				})
			}
		}
	}
}

func TestAgentMessageNewTurn(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, agent := range []bool{false, true} {
			for _, explicit := range []bool{false, true} {
				t.Run(fmt.Sprintf("ws=%t/agent=%t/explicit-turn=%t", ws, agent, explicit), func(t *testing.T) {
					peer := newFidelityPeer(t, ws)
					body := fidelityBody(fidelityUser())
					metadata := map[string]any{"session_id": "synthetic-agent-turns"}
					if explicit {
						metadata["turn_id"] = "synthetic-turn-one"
					}
					body["client_metadata"] = metadata
					first, _, response := peer.send(body, nil)
					input := append([]any{fidelityUser()}, response["output"].([]any)...)
					if agent {
						input = append(input, agentMessageInput())
					} else {
						input = append(input, fidelityUser())
					}
					body = fidelityBody(input...)
					if explicit {
						metadata["turn_id"] = "synthetic-turn-two"
					}
					body["client_metadata"] = metadata
					second, _, _ := peer.send(body, nil)
					firstTurn := historyImportTurn(t, mustRequestJSON(t, first))
					secondTurn := historyImportTurn(t, mustRequestJSON(t, second))
					if firstTurn == secondTurn {
						t.Error("NEW_TASK reused the completed prior turn ID")
					}
				})
			}
		}
	}
}

func TestAgentMessageCompactionDelta(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
		for _, agent := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/agent=%t", model, agent), func(t *testing.T) {
				calls := 0
				fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, body []byte) (string, string) {
					calls++
					id := fmt.Sprintf("resp_agent_compaction_%d", calls)
					var output []any
					if calls == 1 {
						output = []any{map[string]any{"type": "compaction", "id": "cmp_synthetic_agent", "encrypted_content": "synthetic-checkpoint"}}
					} else {
						output = []any{map[string]any{"type": "message", "id": "msg_synthetic_post_compaction", "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": "synthetic answer"}}}}
					}
					w.Header().Set("Content-Type", "text/event-stream")
					for _, event := range []map[string]any{
						{"type": "response.created", "response": map[string]any{"id": id}},
						{"type": "response.output_item.done", "output_index": 0, "item": output[0]},
						{"type": "response.completed", "response": map[string]any{"id": id, "status": "completed", "output": output}},
					} {
						fmt.Fprintf(w, "data: %s\n\n", mustRequestJSON(t, event))
					}
					return id, "synthetic-provider"
				})
				turn := map[string]any{"session_id": "synthetic-agent-compaction", "request_kind": "compaction", "compaction": map[string]any{"implementation": "responses_compaction_v2"}}
				body := map[string]any{"model": model, "input": append(agentHistoryInput(agent), map[string]any{"type": "compaction_trigger"}),
					"client_metadata": map[string]any{"session_id": "synthetic-agent-compaction", "x-codex-turn-metadata": string(mustRequestJSON(t, turn))}}
				status, raw, _ := publicRequestWithHeaders(t, fixture.public, fixture.subscriptionKey, string(mustRequestJSON(t, body)), nil)
				if status != http.StatusOK {
					t.Fatalf("compaction status=%d", status)
				}
				waitForRoutingCapture(t, fixture.captures)
				response := map[string]any{}
				if json.Unmarshal([]byte(raw), &response) != nil {
					t.Fatal("invalid compaction response")
				}
				body = map[string]any{"model": model, "previous_response_id": response["id"], "input": []any{fidelityUser()}, "client_metadata": map[string]any{"session_id": "synthetic-agent-compaction"}}
				status, _, _ = publicRequestWithHeaders(t, fixture.public, fixture.subscriptionKey, string(mustRequestJSON(t, body)), nil)
				t.Logf("post-compaction status=%d", status)
				if status != http.StatusOK {
					t.Error("post-compaction delta rejected")
				} else {
					waitForRoutingCapture(t, fixture.captures)
				}
				input := append(agentHistoryInput(agent), response["output"].([]any)...)
				input = append(input, fidelityUser())
				body = map[string]any{"model": model, "input": input, "client_metadata": map[string]any{"session_id": "synthetic-agent-compaction", "turn_id": "synthetic-after-compaction"}}
				status, _, _ = publicRequestWithHeaders(t, fixture.public, fixture.subscriptionKey, string(mustRequestJSON(t, body)), nil)
				t.Logf("post-compaction full replacement status=%d", status)
				if status != http.StatusOK {
					t.Fatal("post-compaction full replacement rejected")
				}
				waitForRoutingCapture(t, fixture.captures)
			})
		}
	}
}
