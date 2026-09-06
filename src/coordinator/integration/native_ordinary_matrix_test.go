//go:build nativeparity

package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/klauspost/compress/zstd"
)

type nativeOrdinaryClient struct {
	t       *testing.T
	gateway nativeGateway
	ws      *websocket.Conn
	stream  bool
	headers http.Header
}

func ordinaryClient(t *testing.T, gateway nativeGateway, ws, stream bool, headers http.Header) nativeOrdinaryClient {
	t.Helper()
	if headers == nil {
		headers = make(http.Header)
	}
	headers.Set("Authorization", "Bearer "+gateway.secret)
	headers.Set("User-Agent", "OpenAI/Go synthetic")
	headers.Set("X-Stainless-Lang", "go")
	client := nativeOrdinaryClient{t: t, gateway: gateway, stream: stream, headers: headers}
	if ws {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		connection, _, err := websocket.Dial(ctx, strings.Replace(gateway.server.URL, "http", "ws", 1)+"/v1/responses", &websocket.DialOptions{HTTPHeader: headers, CompressionMode: websocket.CompressionContextTakeover})
		if err != nil {
			t.Fatal("ordinary public handshake")
		}
		t.Cleanup(func() { _ = connection.CloseNow() })
		client.ws = connection
	}
	return client
}
func (c nativeOrdinaryClient) send(request map[string]any) map[string]any {
	c.t.Helper()
	request["stream"] = c.stream
	if c.ws != nil {
		request["type"] = "response.create"
	}
	return c.sendEncoded(mustRequestJSON(c.t, request))
}
func (c nativeOrdinaryClient) sendEncoded(body []byte) map[string]any {
	c.t.Helper()
	output := make(map[int]any)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if c.ws != nil {
		if c.ws.Write(ctx, websocket.MessageText, body) != nil {
			c.t.Fatal("ordinary frame send")
		}
		for {
			kind, body, err := c.ws.Read(ctx)
			if err != nil || kind != websocket.MessageText {
				c.t.Fatal("ordinary frame receive")
			}
			var event map[string]any
			if json.Unmarshal(body, &event) != nil {
				c.t.Fatal("ordinary response JSON")
			}
			if response := collectOrdinaryOutput(c.t, event, output); response != nil {
				return response
			}
			if event["type"] == "error" || event["type"] == "response.failed" {
				c.t.Fatal("ordinary unexpected terminal error")
			}
		}
	}
	req, _ := http.NewRequestWithContext(ctx, http.MethodPost, c.gateway.server.URL+"/v1/responses", bytes.NewReader(body))
	req.Header = c.headers.Clone()
	req.Header.Set("Content-Type", "application/json")
	response, err := http.DefaultClient.Do(req)
	if err != nil {
		c.t.Fatal("ordinary HTTP request")
	}
	defer response.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(response.Body, nativeCaptureLimit+1))
	if err != nil || len(raw) > nativeCaptureLimit || response.StatusCode != 200 {
		c.t.Fatalf("ordinary HTTP delivery status=%d", response.StatusCode)
	}
	if !c.stream {
		var result map[string]any
		if json.Unmarshal(raw, &result) != nil || result["id"] == nil {
			c.t.Fatal("ordinary aggregated response")
		}
		return result
	}
	for _, line := range bytes.Split(raw, []byte("\n")) {
		if !bytes.HasPrefix(line, []byte("data: ")) {
			continue
		}
		var event map[string]any
		if json.Unmarshal(line[6:], &event) == nil {
			if response := collectOrdinaryOutput(c.t, event, output); response != nil {
				return response
			}
		}
	}
	c.t.Fatal("ordinary SSE terminal missing")
	return nil
}

// Streaming clients consume item events; Codex may send only metadata in the final footer.
// Keep this consumer assembly separate from checks of the gateway's actual transmitted bytes.
func collectOrdinaryOutput(t *testing.T, event map[string]any, items map[int]any) map[string]any {
	t.Helper()
	if event["type"] == "response.output_item.done" {
		index, ok := event["output_index"].(float64)
		if !ok || index < 0 || index != float64(int(index)) || event["item"] == nil {
			t.Fatal("invalid streamed completed item")
		}
		items[int(index)] = event["item"]
	}
	if event["type"] != "response.completed" {
		return nil
	}
	response, ok := event["response"].(map[string]any)
	if !ok {
		t.Fatal("invalid streamed terminal response")
	}
	if output, ok := response["output"].([]any); !ok || len(output) == 0 {
		output := make([]any, len(items))
		for index := range output {
			item, exists := items[index]
			if !exists {
				t.Fatal("streamed response item gap")
			}
			output[index] = item
		}
		response["output"] = output
	}
	return response
}
func ordinaryInput() []any {
	return []any{
		map[string]any{"role": "system", "content": "system fixture"},
		map[string]any{"role": "developer", "content": "duplicate developer"},
		map[string]any{"role": "developer", "content": "duplicate developer"},
		map[string]any{"role": "user", "content": "first ordinary user"},
	}
}
func ordinaryRequest(format, identity string) map[string]any {
	model := "gpt-5.4"
	if format != "ordinary" {
		model = "gpt-5.6-sol"
	}
	tools := []any{map[string]any{"type": "function", "name": "native_probe", "parameters": map[string]any{"type": "object", "properties": map[string]any{}}}}
	input := ordinaryInput()
	request := map[string]any{"model": model, "input": input, "instructions": "  基础 {{literal}}  ", "tools": tools}
	if identity != "anonymous" {
		metadata := map[string]any{"session_id": "ordinary-session"}
		if identity == "turn" {
			metadata["turn_id"] = "ordinary-turn"
		}
		request["client_metadata"] = metadata
	}
	if format == "native-lite" {
		request["input"] = append([]any{
			map[string]any{"type": "additional_tools", "role": "developer", "tools": tools},
			map[string]any{"type": "message", "role": "developer", "content": []any{map[string]any{"type": "input_text", "text": request["instructions"]}}},
		}, input...)
		delete(request, "instructions")
		delete(request, "tools")
	}
	return request
}
func TestNativeOrdinaryCallerMatrix(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, delivery := range []string{"http-json", "http-sse", "ws"} {
			for _, format := range []string{"ordinary", "converted-lite", "native-lite"} {
				for _, identity := range []string{"turn", "session", "anonymous"} {
					t.Run(fmt.Sprintf("subscription=%t/%s/%s/%s", subscription, delivery, format, identity), func(t *testing.T) {
						capture := newNativeCapture(t)
						gateway := newNativeGateway(t, capture.server.URL, subscription)
						client := ordinaryClient(t, gateway, delivery == "ws", delivery != "http-json", nil)
						first := ordinaryRequest(format, identity)
						result := client.send(first)
						output, ok := result["output"].([]any)
						if !ok || len(output) != 1 {
							t.Fatal("ordinary tool response output")
						}
						call, _ := output[0].(map[string]any)
						if call["type"] != "function_call" || call["call_id"] == nil {
							t.Fatal("ordinary tool call linkage")
						}
						second := map[string]any{"model": first["model"], "previous_response_id": result["id"], "input": []any{map[string]any{"type": "function_call_output", "call_id": call["call_id"], "output": "synthetic ordinary result"}}}
						// Ordinary Responses settings belong to each request. Re-send the base/tools
						// to keep them unchanged; native Lite setup is inherited from its input prefix.
						if format != "native-lite" {
							second["instructions"] = first["instructions"]
							second["tools"] = first["tools"]
						}
						result = client.send(second)
						if result["id"] == nil {
							t.Fatal("ordinary continuation completion")
						}
						business := businessWires(capture.snapshot())
						if len(business) != 2 {
							t.Fatalf("ordinary upstream business count=%d", len(business))
						}
						next := business[1]
						if !subscription {
							incoming := gateway.tap.packets(t)
							if len(incoming) != 2 {
								t.Fatal("ordinary API-key capture count")
							}
							for i, wire := range business {
								if !bytes.Equal(incoming[i].payload, wire.encodedBody) {
									t.Fatal("ordinary API-key payload changed")
								}
							}
							if next.value["previous_response_id"] == nil || next.headers.Get("X-Stainless-Lang") != "go" {
								t.Fatal("ordinary API-key continuation/header changed")
							}
						} else {
							if next.headers.Get("Originator") != "codex-tui" || next.headers.Get("X-Stainless-Lang") != "" {
								t.Fatal("ordinary Subscription identity/filter")
							}
							if delivery == "ws" {
								if next.value["previous_response_id"] == nil {
									t.Fatal("ordinary WS tool delta missing")
								}
							} else {
								if next.value["previous_response_id"] != nil {
									t.Fatal("ordinary HTTP increment not expanded")
								}
								input, _ := next.value["input"].([]any)
								roles := []string{}
								counts := map[string]int{}
								for _, raw := range input {
									item, _ := raw.(map[string]any)
									if role, ok := item["role"].(string); ok {
										roles = append(roles, role)
									}
									content, _ := item["content"].([]any)
									for _, part := range content {
										p, _ := part.(map[string]any)
										if s, ok := p["text"].(string); ok {
											counts[s]++
										}
									}
								}
								if counts["duplicate developer"] != 2 || counts["first ordinary user"] != 1 || strings.Contains(strings.Join(roles, ","), "system") {
									t.Fatal("ordinary HTTP reconstruction lost order/duplicates/roles")
								}
								if format == "ordinary" && next.value["instructions"] != first["instructions"] {
									t.Fatal("ordinary base inheritance")
								}
								if format != "ordinary" && format != "native-lite" {
									assertNativeLiteIDs(t, next)
								}
							}
						}
						saveFinalCapture(t, gateway.tap.packets(t), capture.snapshot())
					})
				}
			}
		}
	}
}

func TestNativeOrdinaryCompressedAndSpoofedMarkers(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, marker := range []string{"", "not-codex", "codex_exec"} {
			t.Run(fmt.Sprintf("subscription=%t/marker=%s", subscription, marker), func(t *testing.T) {
				capture := newNativeCapture(t)
				gateway := newNativeGateway(t, capture.server.URL, subscription)
				request := ordinaryRequest("ordinary", "anonymous")
				request["stream"] = true
				body := mustRequestJSON(t, request)
				encoder, _ := zstd.NewWriter(nil)
				encoded := encoder.EncodeAll(body, nil)
				_ = encoder.Close()
				req, _ := http.NewRequest(http.MethodPost, gateway.server.URL+"/v1/responses", bytes.NewReader(encoded))
				req.Header.Set("Authorization", "Bearer "+gateway.secret)
				req.Header.Set("Content-Encoding", "zstd")
				req.Header.Set("Content-Type", "application/json")
				req.Header.Set("Originator", marker)
				response, err := http.DefaultClient.Do(req)
				if err != nil {
					t.Fatal("compressed ordinary request")
				}
				defer response.Body.Close()
				_, _ = io.Copy(io.Discard, response.Body)
				if response.StatusCode != 200 {
					t.Fatalf("compressed ordinary status=%d", response.StatusCode)
				}
				wires := capture.snapshot()
				if len(wires) != 1 {
					t.Fatal("compressed ordinary count")
				}
				if !subscription && !bytes.Equal(wires[0].encodedBody, encoded) {
					t.Fatal("API-key zstd bytes changed")
				}
				if subscription && wires[0].headers.Get("Originator") != "codex-tui" {
					t.Fatal("spoof marker bypassed Subscription emulation")
				}
			})
		}
	}
}
