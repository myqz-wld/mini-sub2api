//go:build nativeparity && liveparity

package integration

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/klauspost/compress/zstd"
)

func TestLiveSubscriptionNative(t *testing.T) {
	gateway := newLiveSubscriptionGateway(t)
	for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
		for _, ws := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/ws=%t", model, ws), func(t *testing.T) {
				var answers []string
				toolCalls := 0
				options := nativeOptions{endpoint: gateway.server.URL, bearer: gateway.secret, model: model, ws: ws, deadline: 3 * time.Minute,
					base:            "When asked, call native_probe exactly once and then reply exactly PASS. Never call any other tool. For a plain text request reply with exactly the word requested.",
					configOverrides: map[string]string{"model_reasoning_effort": `"low"`},
					observe: func(event map[string]any) {
						if event["method"] == "item/tool/call" {
							toolCalls++
							if toolCalls > 1 {
								t.Fatal("live native tool-call bound exceeded")
							}
						}
						if event["method"] == "item/completed" {
							params, _ := event["params"].(map[string]any)
							item, _ := params["item"].(map[string]any)
							if item["type"] == "agentMessage" {
								if text, ok := item["text"].(string); ok {
									answers = append(answers, text)
								}
							}
						}
					},
				}
				if model == "gpt-6-astra" {
					options.configOverrides["features.code_mode"] = "true"
					options.configOverrides["features.code_mode_host"] = "true"
					options.base = "Use the available code-mode execution or wait tools when necessary to call native_probe exactly once. After receiving its result, reply exactly PASS. Do not call other business tools. For a plain text request reply with exactly the word requested."
				}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				client.turn(thread, "Call native_probe once, then reply PASS.")
				if toolCalls != 1 || len(answers) == 0 || strings.TrimSpace(answers[len(answers)-1]) != "PASS" {
					last := ""
					if len(answers) > 0 {
						last = answers[len(answers)-1]
					}
					declared := false
					for _, packet := range gateway.tap.packets(t) {
						raw := packet.payload
						if packet.headers.Get("Content-Encoding") == "zstd" {
							decoder, err := zstd.NewReader(nil)
							if err != nil {
								t.Fatal("live capture decoder")
							}
							raw, err = decoder.DecodeAll(raw, nil)
							decoder.Close()
							if err != nil {
								t.Fatal("live capture decompression")
							}
						}
						var value map[string]any
						if json.Unmarshal(raw, &value) != nil {
							t.Fatal("live caller JSON")
						}
						declared = declared || liveHasProbe(value["tools"])
						if input, ok := value["input"].([]any); ok {
							for _, raw := range input {
								item, _ := raw.(map[string]any)
								if item["type"] == "additional_tools" {
									declared = declared || liveHasProbe(item["tools"])
								}
							}
						}
					}
					t.Logf("LIVE_NATIVE_SHAPE calls=%d answers=%d last_bytes=%d exact=%t contains_expected=%t declared=%t mentions_tool=%t", toolCalls, len(answers), len(last), strings.TrimSpace(last) == "PASS", strings.Contains(last, "PASS"), declared, strings.Contains(strings.ToLower(last), "tool"))
					t.Fatal("live native tool loop did not produce the requested result")
				}
				client.turn(thread, "Reply exactly NEXT without calling tools.")
				if len(answers) < 2 || strings.TrimSpace(answers[len(answers)-1]) != "NEXT" {
					t.Fatal("live native next turn did not produce the requested result")
				}
			})
		}
	}
}

func liveHasProbe(raw any) bool {
	tools, _ := raw.([]any)
	for _, raw := range tools {
		tool, _ := raw.(map[string]any)
		if tool["name"] == "native_probe" {
			return true
		}
		if liveHasProbe(tool["tools"]) {
			return true
		}
	}
	return false
}
