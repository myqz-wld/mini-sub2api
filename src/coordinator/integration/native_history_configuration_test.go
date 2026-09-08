//go:build nativeparity

package integration

import (
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"reflect"
	"testing"
)

// Four anonymous full requests: original setup, changed setup, unchanged setup, omitted setup.
// Reconnect controls prove association without socket binding; same-socket controls prove that
// association neither grants an invalid delta nor prevents reuse of the next valid baseline.
func TestNativeAnonymousConfigurationChanges(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, delivery := range []string{"http-json", "http-sse", "ws", "ws-reconnect"} {
			for _, format := range []string{"ordinary", "converted-lite", "native-lite"} {
				for _, marked := range []bool{false, true} {
					t.Run(fmt.Sprintf("subscription=%t/%s/%s/marked=%t", subscription, delivery, format, marked), func(t *testing.T) {
						// No compaction trigger is sent; this existing responder also independently
						// reconstructs effective input from actual socket references and saved output.
						capture := newNativeCompactionCapture(t, "v2", false)
						gateway := newNativeGateway(t, capture.server.URL, subscription)
						headers := make(http.Header)
						if marked {
							headers.Set("Originator", "codex_cli_rs")
						}
						ws := delivery == "ws" || delivery == "ws-reconnect"
						client := ordinaryClient(t, gateway, ws, delivery != "http-json", headers.Clone())
						request := ordinaryRequest(format, "anonymous")
						for step := 0; step < 4; step++ {
							if step > 0 && delivery == "ws-reconnect" {
								_ = client.ws.CloseNow()
								client = ordinaryClient(t, gateway, true, true, headers.Clone())
							}
							changeAnonymousConfiguration(request, format, step)
							response := client.send(request)
							output, ok := response["output"].([]any)
							if !ok || len(output) != 1 || output[0].(map[string]any)["type"] != "message" {
								t.Fatal("configuration capture response output missing")
							}
							history := append(request["input"].([]any), output...)
							request["input"] = append(history, compactionUser(fmt.Sprintf("configuration user %d", step+1)))
						}
						all := capture.snapshot()
						business := businessWires(all)
						packets := gateway.tap.packets(t)
						if len(business) != 4 || len(packets) != 4 {
							t.Fatal("configuration capture business count changed")
						}
						for _, packet := range packets {
							caller := decodeNativePacket(t, packet)
							if caller["client_metadata"] != nil || caller["previous_response_id"] != nil || packet.headers.Get("Session-Id") != "" {
								t.Fatal("configuration fixture supplied an explicit locator")
							}
						}
						assertCheckpointRawCapture(t, capture, all)
						wantFrames := 4
						if subscription && ws && !marked {
							wantFrames++
							if delivery == "ws-reconnect" {
								wantFrames += 3
							}
						}
						if len(all) != wantFrames {
							t.Fatal("configuration change added unexpected warmup frames")
						}
						if !subscription {
							assertCompactionPassthroughPackets(t, packets, business)
							return
						}
						assertBareHistoryIdentity(t, business, false, delivery == "ws-reconnect")
						effective := businessWires(capture.effectiveWires(t))
						for step, wire := range effective {
							assertAnonymousCurrentConfiguration(t, wire, format, step)
							_, texts := compactionItemCounts(wire)
							if texts["first ordinary user"] != 1 || texts["system fixture"] != 1 || texts["duplicate developer"] != 2 ||
								texts["synthetic precompaction assistant"] != step {
								t.Fatal("configuration association changed full history or duplicate instructions")
							}
							for turn := 1; turn <= 3; turn++ {
								want := 0
								if turn <= step {
									want = 1
								}
								if texts[fmt.Sprintf("configuration user %d", turn)] != want {
									t.Fatal("configuration association omitted or duplicated a user turn")
								}
							}
							if delivery == "ws" {
								if step == 1 || step == 3 || marked {
									if business[step].value["previous_response_id"] != nil {
										t.Fatal("changed configuration incorrectly reused WS context")
									}
								} else if step == 2 {
									if business[step].value["previous_response_id"] == nil || len(business[step].value["input"].([]any)) != 1 {
										t.Fatal("unchanged current configuration lost valid WS suffix reuse")
									}
								}
							} else if !ws && wire.value["previous_response_id"] != nil {
								t.Fatal("HTTP configuration continuation sent an upstream reference")
							}
						}
						for _, wire := range all {
							assertAnonymousConfigurationIDs(t, wire, format)
						}
					})
				}
			}
		}
	}
}

func changeAnonymousConfiguration(request map[string]any, format string, step int) {
	if step == 1 {
		request["reasoning"] = map[string]any{"effort": "high"}
		request["text"] = map[string]any{"verbosity": "low"}
		request["model"] = "gpt-5.5"
		if format == "converted-lite" {
			request["model"] = "gpt-5.6-terra"
		}
		if format == "native-lite" {
			request["model"] = "gpt-5.4"
		}
		if format != "native-lite" {
			request["instructions"] = "  current base {{caller}}  "
			request["tools"] = []any{map[string]any{"type": "function", "name": "current_probe", "parameters": map[string]any{"type": "object"}}}
		}
	} else if step == 3 {
		request["text"] = map[string]any{"verbosity": "medium"}
		if format != "native-lite" {
			delete(request, "instructions")
			delete(request, "tools")
		}
	}
}

func assertAnonymousCurrentConfiguration(t *testing.T, wire nativeWire, format string, step int) {
	t.Helper()
	model := "gpt-5.4"
	if format != "ordinary" {
		model = "gpt-5.6-sol"
	}
	if step > 0 {
		model = "gpt-5.5"
		if format == "converted-lite" {
			model = "gpt-5.6-terra"
		}
		if format == "native-lite" {
			model = "gpt-5.4"
		}
		reasoning, _ := wire.value["reasoning"].(map[string]any)
		text, _ := wire.value["text"].(map[string]any)
		verbosity := "low"
		if step == 3 {
			verbosity = "medium"
		}
		if reasoning["effort"] != "high" || text["verbosity"] != verbosity {
			t.Fatal("configuration capture retained stale request settings")
		}
	}
	if wire.value["model"] != model {
		t.Fatal("configuration capture retained stale model")
	}
	if wire.method == "POST" && wire.headers.Get("X-Codex-Routing-Hint") != "model="+model {
		t.Fatal("current configuration disagrees with HTTP routing header")
	}
	base := "  基础 {{literal}}  "
	names := []string{"native_probe"}
	if step > 0 && format != "native-lite" {
		base = "  current base {{caller}}  "
		names = []string{"current_probe"}
	}
	if step == 3 && format != "native-lite" {
		base = ""
		names = []string{}
	}
	items := wire.value["input"].([]any)
	wantItems := 4 + 2*step
	if format != "ordinary" {
		wantItems += 2
		if format == "converted-lite" && step == 3 {
			wantItems--
		}
	}
	if len(items) != wantItems {
		t.Fatal("configuration association added or lost full input items")
	}
	tools, _ := wire.value["tools"].([]any)
	if format == "ordinary" {
		if base == "" {
			if wire.value["instructions"] != nil || wire.value["tools"] != nil {
				t.Fatal("omitted ordinary setup inherited prior values")
			}
		} else if wire.value["instructions"] != base {
			t.Fatal("ordinary current base changed")
		}
	} else {
		if wire.value["instructions"] != nil || wire.value["tools"] != nil {
			t.Fatal("Lite current setup retained top-level carriers")
		}
		tools = items[0].(map[string]any)["tools"].([]any)
		if base != "" {
			instruction := items[1].(map[string]any)
			content, _ := instruction["content"].([]any)
			if instruction["role"] != "developer" || len(content) != 1 || content[0].(map[string]any)["text"] != base {
				t.Fatal("Lite current base content or position changed")
			}
		}
	}
	got := []string{}
	for _, raw := range tools {
		tool := raw.(map[string]any)
		if tool["type"] == "namespace" {
			for _, child := range tool["tools"].([]any) {
				got = append(got, child.(map[string]any)["name"].(string))
			}
		} else {
			got = append(got, tool["name"].(string))
		}
	}
	if !reflect.DeepEqual(got, names) {
		t.Fatal("current tools were not used after configuration change")
	}
	_, texts := compactionItemCounts(wire)
	for _, candidate := range []string{"  基础 {{literal}}  ", "  current base {{caller}}  "} {
		want := 0
		if format != "ordinary" && candidate == base {
			want = 1
		}
		if texts[candidate] != want {
			t.Fatal("configuration association inserted a stale or duplicate base")
		}
	}
}

func assertAnonymousConfigurationIDs(t *testing.T, wire nativeWire, format string) {
	t.Helper()
	var body struct {
		Input []struct {
			Type    string          `json:"type"`
			ID      string          `json:"id"`
			Tools   json.RawMessage `json:"tools"`
			Content []struct {
				Text string `json:"text"`
			} `json:"content"`
		} `json:"input"`
	}
	if json.Unmarshal(wire.body, &body) != nil {
		t.Fatal("configuration ID oracle could not decode capture")
	}
	if len(body.Input) == 0 || body.Input[0].Type != "additional_tools" {
		return
	}
	metadata, _ := wire.value["client_metadata"].(map[string]any)
	thread, _ := metadata["thread_id"].(string)
	oid, _ := hex.DecodeString("6ba7b8129dad11d180b400c04fd430c8")
	namespace := uuid5(oid, []byte(thread))
	if thread == "" || body.Input[0].ID != "at_"+uuidText(uuid5(namespace, body.Input[0].Tools)) {
		t.Fatal("configuration tools ID differs from current thread and raw schema")
	}
	if len(body.Input) < 2 {
		return
	}
	base := body.Input[1]
	if format == "native-lite" {
		if base.ID != "" {
			t.Fatal("formed caller base acquired an invented native ID")
		}
	} else if len(base.Content) == 1 && (base.Content[0].Text == "  基础 {{literal}}  " || base.Content[0].Text == "  current base {{caller}}  ") {
		if base.ID != "msg_"+uuidText(uuid5(namespace, []byte(base.Content[0].Text))) {
			t.Fatal("configuration base ID differs from current thread and text")
		}
	}
}
