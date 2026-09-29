//go:build nativeparity && liveparity && scaffoldparity

package integration

import (
	"fmt"
	"strings"
	"testing"
	"time"
)

// Release-specific live coverage: both new catalog models, all three real callers,
// an initial turn and a memory-dependent continuation, with captured Core egress.
func TestLiveSubscription1580Models(t *testing.T) {
	for _, model := range []string{"gpt-6-sol", "gpt-6-luna"} {
		for _, caller := range []string{"bare", "codex", "opencode"} {
			t.Run(fmt.Sprintf("%s/%s", model, caller), func(t *testing.T) {
				runLive1580Model(t, model, caller)
			})
		}
	}
}

func runLive1580Model(t *testing.T, model, caller string) {
	t.Helper()
	gateway, relay := newLiveCallerWireGateway(t, 4)
	label, scenario := liveMemoryScenario(t)
	steps := []liveMemoryStep{scenario[0], {prompt: "Increase our remembered count by 5 and return a JSON object with exactly label and count. Do not use tools or Markdown.", count: 12}}
	switch caller {
	case "bare":
		client := ordinaryClient(t, gateway, false, false, nil)
		var previous any
		for _, step := range steps {
			request := map[string]any{"model": model, "instructions": liveMemoryBase, "input": step.prompt, "reasoning": map[string]any{"effort": "low"}}
			if previous != nil {
				request["previous_response_id"] = previous
			}
			response := liveResponse(t, client, request)
			assertLiveMemory(t, liveText(response), label, step.count)
			previous = response["id"]
		}
	case "codex":
		var answers []string
		options := nativeOptions{endpoint: gateway.server.URL, bearer: gateway.secret, model: model, ws: true, deadline: 3 * time.Minute, base: liveMemoryBase,
			configOverrides: map[string]string{"model_reasoning_effort": `"low"`}, observe: func(event map[string]any) {
				if event["method"] == "item/completed" {
					params, _ := event["params"].(map[string]any)
					item, _ := params["item"].(map[string]any)
					if item["type"] == "agentMessage" {
						text, _ := item["text"].(string)
						answers = append(answers, text)
					}
				}
			}}
		client := startNativeClient(t, options)
		thread := client.thread(options)
		for _, step := range steps {
			before := len(answers)
			client.turn(thread, step.prompt)
			if len(answers) <= before {
				t.Fatal("live Codex model answer missing")
			}
			assertLiveMemory(t, answers[len(answers)-1], label, step.count)
		}
	case "opencode":
		client := startOpenCode(t, gateway.server.URL, gateway.secret, model)
		created := client.call(t, "/session", map[string]any{"title": "Synthetic release model check"})
		session, _ := created["id"].(string)
		if session == "" {
			t.Fatal("live OpenCode model session missing")
		}
		for _, step := range steps {
			result := client.turn(t, session, step.prompt+" Do not use tools or Markdown.")
			info, _ := result["info"].(map[string]any)
			if info["error"] != nil {
				t.Fatal("live OpenCode model inference failed")
			}
			var answer strings.Builder
			parts, _ := result["parts"].([]any)
			for _, raw := range parts {
				part, _ := raw.(map[string]any)
				if part["type"] == "text" {
					s, _ := part["text"].(string)
					answer.WriteString(s)
				}
			}
			assertLiveMemory(t, answer.String(), label, step.count)
		}
	}
	assertLiveCallerWire(t, gateway, relay)
	t.Log("LIVE_1580_MODEL user_turns=2 memory_verified=true egress_captured=true")
}
