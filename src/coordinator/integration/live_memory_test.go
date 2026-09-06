//go:build nativeparity && liveparity

package integration

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"time"
)

const liveMemoryBase = "Follow the synthetic conversation memory task. Keep the stated label and update its numeric count across turns. Never call tools. Reply ACK to updates, and only the requested JSON object to state queries, without Markdown."

type liveMemoryStep struct {
	prompt string
	count  int
}

func liveMemoryScenario(t *testing.T) (string, []liveMemoryStep) {
	t.Helper()
	var nonce [6]byte
	if _, err := rand.Read(nonce[:]); err != nil {
		t.Fatal("create synthetic memory label")
	}
	label := "SYNTHETIC_" + hex.EncodeToString(nonce[:])
	return label, []liveMemoryStep{
		{"Remember our label is " + label + " and count is 7. Reply ACK.", 0},
		{"Increase our remembered count by 5, keeping the label unchanged. Reply ACK.", 0},
		{"Return a JSON object with exactly label and count, using our remembered current values.", 12},
		{"Replace our remembered count with 3; preserve the label. Reply ACK.", 0},
		{"Return a JSON object with exactly label and count, using our remembered current values.", 3},
	}
}

func assertLiveMemory(t *testing.T, text, label string, count int) {
	t.Helper()
	if count == 0 {
		if strings.TrimSpace(text) != "ACK" {
			t.Fatal("live memory update acknowledgment differs")
		}
		return
	}
	var state map[string]any
	if json.Unmarshal([]byte(strings.TrimSpace(text)), &state) != nil || len(state) != 2 || state["label"] != label || state["count"] != float64(count) {
		t.Logf("LIVE_MEMORY_VERDICT valid_json=%t label_matches=%t count_matches=%t fields=%d", state != nil, state["label"] == label, state["count"] == float64(count), len(state))
		t.Fatal("live answer did not retain and update earlier conversation facts")
	}
}

func TestLiveSubscriptionMemoryBare(t *testing.T) {
	gateway := newLiveSubscriptionGatewayBounded(t, 40)
	for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
		for _, delivery := range []string{"json", "sse", "ws"} {
			for _, mode := range []string{"full", "reference"} {
				t.Run(fmt.Sprintf("%s/%s/%s", model, delivery, mode), func(t *testing.T) {
					label, steps := liveMemoryScenario(t)
					client := ordinaryClient(t, gateway, delivery == "ws", delivery != "json", nil)
					var history []any
					var previous any
					for _, step := range steps {
						message := map[string]any{"role": "user", "content": step.prompt}
						input := []any{message}
						if mode == "full" {
							history = append(history, message)
							input = history
						}
						request := map[string]any{"model": model, "instructions": liveMemoryBase, "input": input, "reasoning": map[string]any{"effort": "low"}}
						if mode == "reference" && previous != nil {
							request["previous_response_id"] = previous
						}
						response := liveResponse(t, client, request)
						assertLiveMemory(t, liveText(response), label, step.count)
						previous = response["id"]
						if previous == nil {
							t.Fatal("live memory response reference absent")
						}
						if mode == "full" {
							output, _ := response["output"].([]any)
							history = append(history, output...)
						}
					}
				})
			}
		}
	}
}

func TestLiveSubscriptionMemoryNative(t *testing.T) {
	gateway := newLiveSubscriptionGatewayBounded(t, 10)
	for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
		for _, ws := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/ws=%t", model, ws), func(t *testing.T) {
				label, steps := liveMemoryScenario(t)
				var answers []string
				options := nativeOptions{endpoint: gateway.server.URL, bearer: gateway.secret, model: model, ws: ws, deadline: 3 * time.Minute, base: liveMemoryBase,
					configOverrides: map[string]string{"model_reasoning_effort": `"low"`},
					observe: func(event map[string]any) {
						if event["method"] == "item/tool/call" {
							t.Fatal("live memory task unexpectedly invoked a dynamic tool")
						}
						if event["method"] == "item/completed" {
							params, _ := event["params"].(map[string]any)
							item, _ := params["item"].(map[string]any)
							if item["type"] == "agentMessage" {
								text, _ := item["text"].(string)
								answers = append(answers, text)
							}
						}
					},
				}
				if model == "gpt-6-astra" {
					options.configOverrides["features.code_mode"] = "true"
					options.configOverrides["features.code_mode_host"] = "true"
				}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				for _, step := range steps {
					before := len(answers)
					client.turn(thread, step.prompt)
					if len(answers) <= before {
						t.Fatal("live native memory answer absent")
					}
					assertLiveMemory(t, answers[len(answers)-1], label, step.count)
				}
			})
		}
	}
}
