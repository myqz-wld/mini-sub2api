//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"reflect"
	"testing"
)

func TestSubscriptionReasoningVisibility(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, delivery := range []string{"http-json", "http-sse", "ws", "ws-reconnect"} {
			for _, format := range []string{"ordinary", "converted-lite", "native-lite"} {
				for _, reference := range []bool{false, true} {
					for _, marked := range []bool{false, true} {
						t.Run(fmt.Sprintf("subscription=%t/%s/%s/reference=%t/marked=%t", subscription, delivery, format, reference, marked), func(t *testing.T) {
							capture := newReasoningCapture(t, reference)
							gateway := newNativeGateway(t, capture.server.URL, subscription)
							headers := make(http.Header)
							if marked {
								headers.Set("Originator", "codex_cli_rs")
							}
							ws := delivery == "ws" || delivery == "ws-reconnect"
							client := ordinaryClient(t, gateway, ws, delivery != "http-json", headers.Clone())
							request := ordinaryRequest(format, "anonymous")
							includes := []any{[]any{}, []any{"reasoning.encrypted_content"}, []any{"message.output_text.logprobs"}, nil}
							for step := 0; step < 4; step++ {
								if step > 0 && delivery == "ws-reconnect" {
									_ = client.ws.CloseNow()
									client = ordinaryClient(t, gateway, true, true, headers.Clone())
								}
								request["include"] = includes[step]
								response := sendReasoningRequest(t, client, request, !subscription || step == 1)
								output, ok := response["output"].([]any)
								if !ok || len(output) != 2 {
									t.Fatal("reasoning returned output count changed")
								}
								if step >= 2 {
									message := output[1].(map[string]any)
									part := message["content"].([]any)[0].(map[string]any)
									if _, ok := part["logprobs"].([]any); !ok {
										t.Fatal("other included output was removed with reasoning ciphertext")
									}
								}
								var suffix []any
								if step < 2 {
									call := output[1].(map[string]any)
									if call["type"] != "function_call" || call["call_id"] == nil {
										t.Fatal("reasoning tool-loop call missing")
									}
									suffix = []any{map[string]any{"type": "function_call_output", "call_id": call["call_id"], "output": "synthetic result"}}
								} else {
									suffix = []any{compactionUser("next synthetic user")}
								}
								if reference {
									request["previous_response_id"] = response["id"]
									request["input"] = suffix
								} else {
									request["input"] = append(append(request["input"].([]any), output...), suffix...)
								}
							}
							assertReasoningWires(t, capture, gateway, subscription, ws, delivery == "ws-reconnect", reference, marked, format, includes)
						})
					}
				}
			}
		}
	}
}

func assertReasoningWires(t *testing.T, capture *reasoningCapture, gateway nativeGateway, subscription, ws, reconnect, reference, marked bool, format string, includes []any) {
	t.Helper()
	all := capture.snapshot()
	wires := businessWires(all)
	packets := gateway.tap.packets(t)
	raw := capture.tap.packets(t)
	contexts := capture.contexts()
	if len(wires) != 4 || len(packets) != 4 || len(contexts) != 4 || len(raw) != len(all) {
		t.Fatal("reasoning capture request count changed")
	}
	for i, wire := range all {
		if !bytes.Equal(raw[i].payload, wire.encodedBody) {
			t.Fatal("reasoning raw upstream frame differs from decoded request")
		}
		if subscription {
			include, _ := wire.value["include"].([]any)
			if !hasReasoningInclude(include) {
				t.Fatal("Subscription omitted encrypted reasoning upstream")
			}
			if wire.headers.Get("Originator") != "codex-tui" || wire.headers.Get("Version") != "0.153.4" {
				t.Fatal("reasoning change altered transport fingerprint")
			}
		}
	}
	var firstSession, firstTurn any
	for step, wire := range wires {
		caller := decodeNativePacket(t, packets[step])
		if caller["client_metadata"] != nil || packets[step].headers.Get("Session-Id") != "" {
			t.Fatal("reasoning anonymous fixture supplied a session")
		}
		if !subscription {
			if !bytes.Equal(packets[step].payload, wire.encodedBody) {
				t.Fatal("API-key reasoning request bytes changed")
			}
		} else {
			var expected []any
			if original, ok := includes[step].([]any); ok {
				expected = append([]any{}, original...)
			}
			if !hasReasoningInclude(expected) {
				expected = append(expected, "reasoning.encrypted_content")
			}
			if !reflect.DeepEqual(wire.value["include"], expected) {
				t.Fatal("Subscription include options changed beyond required ciphertext")
			}
			metadata := transportMetadata(t, wire.value)
			if step == 0 {
				firstSession, firstTurn = metadata["session_id"], metadata["turn_id"]
			}
			if metadata["session_id"] != firstSession {
				t.Fatal("hidden reasoning broke session association")
			}
			if step < 3 && metadata["turn_id"] != firstTurn {
				t.Fatal("hidden reasoning broke tool-turn association")
			}
			if step == 3 && metadata["turn_id"] == firstTurn {
				t.Fatal("hidden reasoning prevented a new user turn")
			}
			incremental := wire.value["previous_response_id"] != nil
			if step > 0 && ws && !reconnect && reference && !incremental {
				t.Fatal("explicit WS reasoning reference was unnecessarily expanded")
			}
			if step == 1 && ws && !reconnect && !marked && !reference && !incremental {
				t.Fatal("hidden ciphertext prevented eligible automatic WS incrementality")
			}
			if (!ws || (marked && !reference)) && incremental {
				t.Fatal("reasoning history association granted invalid socket reuse")
			}
			if reconnect && incremental {
				warmup := false
				for index, prior := range all {
					warmup = warmup || (wire.value["previous_response_id"] == fmt.Sprintf("resp_transport_%d", index+1) && prior.connection == wire.connection && prior.value["generate"] == false)
				}
				if !warmup || (reference && step > 0) {
					t.Fatal("reasoning reconnect reused an earlier socket context")
				}
			}
			if step >= 2 && ws && !reconnect && !marked && !reference && incremental {
				t.Fatal("changed upstream include reused an incompatible automatic baseline")
			}
			if !incremental && format != "ordinary" {
				assertAnonymousConfigurationIDs(t, wire, format)
			}
		}
		seen := 0
		for _, entry := range contexts[step] {
			item, ok := entry.(map[string]any)
			if !ok || item["type"] != "reasoning" {
				continue
			}
			seen++
			if item["id"] != fmt.Sprintf("rs_transport_%d", seen) || item["encrypted_content"] != fmt.Sprintf("transport-opaque-%d", seen) {
				t.Fatal("effective upstream history lost or changed private reasoning")
			}
			if seen == 1 {
				want := []any{map[string]any{"type": "summary_text", "text": "synthetic reasoning summary"}}
				if !reflect.DeepEqual(item["summary"], want) {
					t.Fatal("reasoning history summary changed")
				}
			}
		}
		if seen != step {
			t.Fatal("effective upstream reasoning history was duplicated or dropped")
		}
		// Reparse the tap independently; no request or response payload is saved to disk.
		var decoded map[string]any
		if json.Unmarshal(wire.body, &decoded) != nil || !reflect.DeepEqual(decoded, wire.value) {
			t.Fatal("reasoning decoded wire oracle diverged")
		}
	}
}

func hasReasoningInclude(values []any) bool {
	for _, value := range values {
		if value == "reasoning.encrypted_content" {
			return true
		}
	}
	return false
}
