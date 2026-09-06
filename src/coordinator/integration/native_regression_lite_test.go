//go:build nativeparity

package integration

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"testing"
	"time"

	"github.com/coder/websocket"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestNativeRejectedCompactionReference(t *testing.T) {
	for _, finalOnly := range []bool{false, true} {
		t.Run(fmt.Sprintf("final_only=%t", finalOnly), func(t *testing.T) {
			capture := boundaryCompactionCapture(t, finalOnly)
			gateway := newNativeGateway(t, capture.server.URL, true)
			client := ordinaryClient(t, gateway, true, true, nil)
			request := ordinaryCompactionRequest("gpt-5.4", []any{
				map[string]any{"type": "message", "role": "user", "content": "synthetic compact context"},
				map[string]any{"type": "compaction_trigger"},
			}, nil)
			metadata := map[string]any{"session_id": "format-context-session", "thread_id": "format-context-session",
				"turn_id": "format-context-compact", "request_kind": "compaction",
				"compaction": map[string]any{"implementation": "responses_compaction_v2"}}
			request["client_metadata"] = map[string]any{"x-codex-turn-metadata": string(mustRequestJSON(t, metadata))}
			result := client.send(request)
			delta := ordinaryCompactionRequest("gpt-5.4", []any{
				map[string]any{"type": "message", "role": "user", "content": "synthetic compact reference probe"},
			}, result["id"])
			delta["type"] = "response.create"
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			if client.ws.Write(ctx, websocket.MessageText, mustRequestJSON(t, delta)) != nil {
				t.Fatal("send context boundary probe")
			}
			terminal := ""
			for terminal == "" {
				kind, body, err := client.ws.Read(ctx)
				if err != nil {
					var closeError websocket.CloseError
					var failure protocolv1.FailureMetadata
					if !finalOnly || !errors.As(err, &closeError) ||
						closeError.Code != websocket.StatusCode(protocolv1.FailureCloseCode) ||
						json.Unmarshal([]byte(closeError.Reason), &failure) != nil || !failure.Valid() ||
						failure.RetryAdvice != protocolv1.RetrySafe || failure.Phase != protocolv1.PhaseInternal ||
						failure.DeliveryState != protocolv1.DeliveryNotDelivered {
						t.Fatal("expected structured pre-delivery state-unavailable close")
					}
					terminal = "error"
					break
				}
				if kind != websocket.MessageText {
					t.Fatal("context boundary returned a non-text event")
				}
				var event map[string]any
				if json.Unmarshal(body, &event) != nil {
					t.Fatal("decode context boundary event")
				}
				switch event["type"] {
				case "response.completed":
					terminal = "completed"
				case "error", "response.failed":
					t.Fatal("expected state-unavailable close rather than upstream failure")
				}
			}
			business := businessWires(capture.snapshot())
			usedReference := len(business) == 2 && business[1].value["previous_response_id"] != nil
			if finalOnly {
				if terminal != "error" || usedReference || len(business) != 1 {
					t.Fatal("native-rejected final-only compaction published a usable WS reference")
				}
			} else if terminal != "completed" || !usedReference {
				t.Fatal("valid streamed compaction reference was not usable")
			}
		})
	}
}

func TestNativeOrdinaryLiteToolChanges(t *testing.T) {
	for _, ws := range []bool{false, true} {
		t.Run(fmt.Sprintf("ws=%t", ws), func(t *testing.T) {
			capture := boundaryCompactionCapture(t, false)
			gateway := newNativeGateway(t, capture.server.URL, true)
			client := ordinaryClient(t, gateway, ws, true, nil)
			tool := func(name string) any {
				return map[string]any{"type": "function", "name": name, "description": "Unicode 工具 <>&", "parameters": map[string]any{"type": "object", "properties": map[string]any{"字符": map[string]any{"type": "string"}}}}
			}
			steps := [][]any{{tool("a"), tool("b")}, {tool("a"), tool("b"), tool("a")}, {tool("b"), tool("a"), tool("a")}, {}, {}, {tool("a"), tool("b")}}
			wantNames := [][]string{{"a", "b"}, {"a", "b", "a"}, {"b", "a", "a"}, {}, {}, {"a", "b"}}
			history := []any{}
			var initial, previous nativeWire
			for step, tools := range steps {
				if ws && step == 5 {
					_ = client.ws.CloseNow()
					client = ordinaryClient(t, gateway, true, true, nil)
				}
				history = append(history, map[string]any{"type": "message", "role": "user", "content": "synthetic repeated setup probe"})
				request := map[string]any{"model": "gpt-5.6-sol", "instructions": "  基础 {{literal}} <>&\n", "tools": tools,
					"input": history, "client_metadata": map[string]any{"session_id": "format-tools-session"}}
				result := client.send(request)
				all := capture.snapshot()
				business := businessWires(all)
				if len(business) != step+1 {
					t.Fatal("duplicate-tool probe sampling count changed")
				}
				current := business[len(business)-1]
				if ws && step == 4 {
					if current.value["previous_response_id"] == nil {
						t.Fatal("unchanged empty setup lost reuse")
					}
				} else {
					if current.value["previous_response_id"] != nil {
						current = all[len(all)-2]
						if current.value["generate"] != false || current.connection != business[len(business)-1].connection {
							t.Fatal("new prefix is not on a full current-connection setup")
						}
					}
					assertNativeLiteIDs(t, current)
					items := current.value["input"].([]any)
					projected := items[0].(map[string]any)["tools"].([]any)
					if len(wantNames[step]) > 0 {
						if len(projected) != 1 {
							t.Fatal("ordinary functions lost namespace grouping")
						}
						projected = projected[0].(map[string]any)["tools"].([]any)
					}
					if len(projected) != len(wantNames[step]) {
						t.Fatal("tool occurrence count changed")
					}
					for i, raw := range projected {
						p := raw.(map[string]any)
						if p["name"] != wantNames[step][i] || p["description"] != "Unicode 工具 <>&" {
							t.Fatal("ordered duplicate/Unicode tool meaning changed")
						}
					}
					if step == 0 {
						initial = current
					}
					if current.value["client_metadata"].(map[string]any)["thread_id"] != initial.value["client_metadata"].(map[string]any)["thread_id"] {
						t.Fatal("prefix changes split the thread namespace")
					}
					if step > 0 && step < 4 && items[0].(map[string]any)["id"] == previous.value["input"].([]any)[0].(map[string]any)["id"] {
						t.Fatal("changed tools retained content ID")
					}
					if items[1].(map[string]any)["id"] != initial.value["input"].([]any)[1].(map[string]any)["id"] {
						t.Fatal("stable base changed content ID")
					}
					if step == 5 && items[0].(map[string]any)["id"] != initial.value["input"].([]any)[0].(map[string]any)["id"] {
						t.Fatal("repeated setup/reconnect changed content ID")
					}
					previous = current
				}
				history = append(history, result["output"].([]any)...)
			}
		})
	}
}

// Native tools/src/tool_spec.rs folds all default namespaces and loose functions
// in encounter order, at the first encountered groupable position.
func TestNativeOrdinaryLiteNamespaceGrouping(t *testing.T) {
	tool := func(name string) any {
		return map[string]any{"type": "function", "name": name, "description": "Synthetic namespace child", "parameters": map[string]any{"type": "object", "properties": map[string]any{}}}
	}
	namespace := func(name string) any {
		return map[string]any{"type": "namespace", "name": "functions", "description": "", "tools": []any{tool(name)}}
	}
	for _, ws := range []bool{false, true} {
		for _, scenario := range []struct {
			name     string
			tools    []any
			children []string
		}{
			{"function-before-namespace", []any{tool("a"), namespace("b")}, []string{"a", "b"}},
			{"namespace-before-function", []any{namespace("b"), tool("a")}, []string{"b", "a"}},
			{"two-default-namespaces", []any{namespace("a"), namespace("b")}, []string{"a", "b"}},
		} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, scenario.name), func(t *testing.T) {
				capture := boundaryCompactionCapture(t, false)
				gateway := newNativeGateway(t, capture.server.URL, true)
				client := ordinaryClient(t, gateway, ws, true, nil)
				client.send(map[string]any{"model": "gpt-5.6-sol", "instructions": "synthetic namespace base", "tools": scenario.tools, "input": "synthetic namespace probe"})
				wire := capture.snapshot()[0]
				assertNativeLiteIDs(t, wire)
				prefix := wire.value["input"].([]any)[0].(map[string]any)
				groups := prefix["tools"].([]any)
				ordered := len(groups) == 1
				childCount := 0
				for _, raw := range groups {
					group := raw.(map[string]any)
					if group["type"] != "namespace" || group["name"] != "functions" {
						t.Fatal("unexpected namespace shape")
					}
					for _, rawChild := range group["tools"].([]any) {
						child := rawChild.(map[string]any)
						ordered = ordered && childCount < len(scenario.children) && child["name"] == scenario.children[childCount]
						childCount++
					}
				}
				ordered = ordered && childCount == len(scenario.children)
				if !ordered {
					t.Fatal("ordinary Lite grouping differs from pinned native encounter order")
				}
			})
		}
	}
}
