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
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestNativeControlHistoryInvalidation(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
			for _, kind := range []string{"response.inject", "response.append_input_item", "future.control"} {
				for _, explicit := range []bool{false, true} {
					t.Run(fmt.Sprintf("subscription=%t/%s/%s/explicit=%t", subscription, model, kind, explicit), func(t *testing.T) {
						var httpCalls atomic.Int32
						upstream, tap := newNativeTappedServer(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
							if r.Method == http.MethodPost {
								httpCalls.Add(1)
								_, _ = io.Copy(io.Discard, io.LimitReader(r.Body, nativeCaptureLimit))
								w.Header().Set("Content-Type", "text/event-stream")
								_, _ = io.WriteString(w, "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_rebuilt\",\"output\":[]}}\n\n")
								return
							}
							connection, err := websocket.Accept(w, r, nil)
							if err != nil {
								return
							}
							defer connection.CloseNow()
							ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
							defer cancel()
							if _, _, err := connection.Read(ctx); err != nil {
								return
							}
							if connection.Write(ctx, websocket.MessageText, []byte(`{"type":"response.created","response":{"id":"resp_control_capture"}}`)) != nil {
								return
							}
							if _, _, err := connection.Read(ctx); err != nil {
								return
							}
							_ = connection.Write(ctx, websocket.MessageText, []byte(`{"type":"response.completed","response":{"id":"resp_control_capture","output":[]}}`))
							_, _, _ = connection.Read(ctx)
						}))
						gateway := newNativeGateway(t, upstream.URL, subscription)
						client := ordinaryClient(t, gateway, true, true, http.Header{"Originator": {"synthetic-marked-caller"}})
						ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
						defer cancel()
						first := map[string]any{"type": "response.create", "stream": true, "model": model, "input": "original"}
						if client.ws.Write(ctx, websocket.MessageText, mustRequestJSON(t, first)) != nil {
							t.Fatal("control initial request failed")
						}
						created := readControlEvent(t, ctx, client.ws)
						response, _ := created["response"].(map[string]any)
						if created["type"] != "response.created" || response["id"] == nil {
							t.Fatal("control created response missing")
						}
						control := map[string]any{"type": kind}
						item := map[string]any{"role": "user", "content": "interleaved"}
						if kind == "response.inject" {
							control["input"] = []any{item}
						} else {
							control["item"] = item
						}
						if explicit {
							control["response_id"] = response["id"]
						}
						controlBody := append([]byte(" "), mustRequestJSON(t, control)...)
						if client.ws.Write(ctx, websocket.MessageText, controlBody) != nil {
							t.Fatal("control send failed")
						}
						completed := readControlEvent(t, ctx, client.ws)
						if completed["type"] != "response.completed" {
							t.Fatal("control completion missing")
						}
						_ = client.ws.CloseNow()
						for step := 0; step < 2; step++ {
							request := map[string]any{"model": model, "stream": true, "input": "replacement full context"}
							if step == 0 {
								request["previous_response_id"] = response["id"]
							}
							req, _ := http.NewRequestWithContext(ctx, http.MethodPost, gateway.server.URL+"/v1/responses", bytes.NewReader(mustRequestJSON(t, request)))
							req.Header.Set("Authorization", "Bearer "+gateway.secret)
							req.Header.Set("Content-Type", "application/json")
							got, err := gateway.server.Client().Do(req)
							if err != nil {
								t.Fatal("control follow-up delivery failed")
							}
							reply, readErr := io.ReadAll(io.LimitReader(got.Body, 4096))
							_ = got.Body.Close()
							if readErr != nil {
								t.Fatal("control follow-up response failed")
							}
							expected := 200
							if subscription && step == 0 {
								expected = http.StatusServiceUnavailable
							}
							if subscription && step == 0 {
								var failure struct {
									Error struct {
										Code string `json:"code"`
									} `json:"error"`
								}
								if json.Unmarshal(reply, &failure) != nil || failure.Error.Code != "state_unavailable" {
									t.Fatal("stale history did not report state_unavailable")
								}
							}
							if got.StatusCode != expected {
								t.Fatal("control reused stale history or rejected full replacement")
							}
						}
						wantHTTP := int32(2)
						if subscription {
							wantHTTP = 1
						}
						if httpCalls.Load() != wantHTTP {
							t.Fatal("unavailable control history reached upstream")
						}
						packets := tap.packets(t)
						if len(packets) != int(wantHTTP)+2 {
							t.Fatal("control capture count differs")
						}
						if !subscription && !bytes.Equal(packets[1].payload, controlBody) {
							t.Fatal("API-key control payload changed")
						}
						if explicit && !strings.Contains(string(packets[1].payload), "resp_control_capture") {
							t.Fatal("control response reference did not reverse correctly")
						}
					})
				}
			}
		}
	}
}

func readControlEvent(t *testing.T, ctx context.Context, connection *websocket.Conn) map[string]any {
	t.Helper()
	_, raw, err := connection.Read(ctx)
	if err != nil {
		t.Fatal("control event receive failed")
	}
	var result map[string]any
	if json.Unmarshal(raw, &result) != nil {
		t.Fatal("control event invalid")
	}
	return result
}
