//go:build nativeparity && liveparity

package integration

import (
	"fmt"
	"net/http"
	"reflect"
	"testing"
	"time"
)

// Three user turns with a real tool round trip in the middle. The label is random and never
// repeated in later prompts, so a correct final answer needs the preceding conversation state.
func TestLiveSubscriptionConversation(t *testing.T) {
	t.Setenv("TERM_PROGRAM", "native-parity")
	t.Setenv("TERM_PROGRAM_VERSION", "1")
	t.Setenv("TERM", "dumb")
	for _, route := range []string{"direct", "gateway", "captured-gateway"} {
		for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
			for _, ws := range []bool{false, true} {
				t.Run(fmt.Sprintf("%s/%s/ws=%t", route, model, ws), func(t *testing.T) {
					label, _ := liveMemoryScenario(t)
					var answers []string
					toolCalls := 0
					options := nativeOptions{model: model, ws: ws, deadline: 3 * time.Minute, project: liveSyntheticDirectory(t),
						base:            "Follow this synthetic memory task. Remember the label and count across turns. Reply only ACK to an update. When asked to call native_probe, call it exactly once and wait for its result before updating the count. Do not call other business tools. On a state query return only a JSON object with exactly label and count, without Markdown.",
						configOverrides: map[string]string{"model_reasoning_effort": `"low"`},
						observe: func(event map[string]any) {
							if event["method"] == "item/tool/call" {
								toolCalls++
								if toolCalls > 1 {
									t.Fatal("live conversation exceeded its tool-call bound")
								}
							}
							if event["method"] == "item/completed" {
								params, _ := event["params"].(map[string]any)
								item, _ := params["item"].(map[string]any)
								if item["type"] == "agentMessage" {
									answer, _ := item["text"].(string)
									answers = append(answers, answer)
								}
							}
						},
					}
					if model == "gpt-6-astra" {
						options.configOverrides["features.code_mode"] = "true"
						options.configOverrides["features.code_mode_host"] = "true"
						options.base += " Use the code-mode exec/wait tools to invoke native_probe when needed."
					}
					var client *nativeClient
					var relay *liveWireRelay
					var gateway nativeGateway
					if route == "direct" {
						client = startLiveNativeDirect(t, options)
					} else {
						if route == "captured-gateway" {
							relay = newLiveWireRelay(t, !ws)
							gateway = newLiveSubscriptionGatewayAt(t, 8, relay.endpoint+"/v1/responses", !ws)
							metadata := newNativeCapture(t)
							options.builtinOpenAI = true
							options.metadataEndpoint = metadata.server.URL
						} else {
							gateway = newLiveSubscriptionGatewayBounded(t, 8)
						}
						options.endpoint, options.bearer = gateway.server.URL, gateway.secret
						client = startNativeClient(t, options)
					}
					thread := client.thread(options)
					steps := []liveMemoryStep{{"Remember label " + label + " and count 7. Reply ACK.", 0}, {"Call native_probe exactly once, then increase our remembered count by 5 and keep the label. Reply ACK.", 0}, {"Return our remembered label and current count as JSON.", 12}}
					for _, step := range steps {
						before := len(answers)
						client.turn(thread, step.prompt)
						if len(answers) <= before {
							t.Fatal("live conversation answer absent")
						}
						assertLiveMemory(t, answers[len(answers)-1], label, step.count)
					}
					if toolCalls != 1 {
						t.Fatal("live conversation did not exercise its tool round trip")
					}
					if relay != nil {
						in, out := gateway.tap.packets(t), relay.tap.packets(t)
						if len(in) != len(out) || len(in) < 4 {
							t.Fatal("live capture changed or missed the multi-turn request sequence")
						}
						projection := &transportProjection{identities: map[string]map[string]string{}}
						for i := range in {
							wantMethod := http.MethodPost
							if ws {
								wantMethod = http.MethodGet
							}
							if in[i].method != wantMethod || out[i].method != wantMethod {
								t.Fatal("live capture transport differs")
							}
							wire := nativeWire{method: out[i].method, headers: out[i].headers, body: nativePacketJSON(t, out[i]), encodedBody: out[i].payload}
							wire.value = transportPacketValue(t, out[i])
							assertNativeMessageParity(t, in[i], wire)
							projection.compare(t, transportPacketValue(t, in[i]), wire.value, fmt.Sprintf("request[%d]", i))
							assertNativeWireParity(t, in[i], out[i], wire, true)
							if !reflect.DeepEqual(in[i].headerSpellings, out[i].headerSpellings) {
								t.Error("live complete header shape changed")
							}
						}
						relay.report(t)
						t.Logf("LIVE_WIRE_SHAPE requests=%d recursive_shape_checked=true", len(in))
					}
					t.Logf("LIVE_CONVERSATION route=%s model=%s requested_ws=%t user_turns=3 tool_callbacks=%d memory_verified=true", route, model, ws, toolCalls)
				})
			}
		}
	}
}
