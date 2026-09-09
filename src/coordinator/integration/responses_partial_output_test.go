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
	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

var partialOutputFooters = []string{"missing", "empty", "prefix", "in-progress", "complete-footer", "item-done", "failed", "incomplete"}

func partialOutputEvents(source, footer, id string) []any {
	message := func(id, status, text string) map[string]any {
		return map[string]any{"type": "message", "id": id, "status": status, "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": text}}}
	}
	first := message("msg_prefix", "completed", "finished prefix")
	partial := message("msg_partial", "in_progress", "partial suffix")
	last := message("msg_partial", "completed", "partial suffix")
	created := map[string]any{"id": id}
	if source == "snapshot" {
		created["output"] = []any{first, partial}
	}
	events := []any{
		map[string]any{"type": "response.created", "response": created},
		map[string]any{"type": "response.output_item.done", "output_index": 0, "item": first},
	}
	if source == "added" {
		events = append(events, map[string]any{"type": "response.output_item.added", "output_index": 1, "item": partial})
	} else if source == "delta" {
		events = append(events, map[string]any{"type": "response.output_text.delta", "output_index": 1, "item_id": "msg_partial", "content_index": 0, "delta": "partial suffix"})
	}
	response := map[string]any{"id": id, "status": "completed"}
	kind := "response.completed"
	switch footer {
	case "empty":
		response["output"] = []any{}
	case "prefix":
		response["output"] = []any{first}
	case "in-progress":
		response["output"] = []any{first, partial}
	case "complete-footer":
		response["output"] = []any{first, last}
	case "item-done":
		events = append(events, map[string]any{"type": "response.output_item.done", "output_index": 1, "item": last})
		response["output"] = []any{}
	case "failed", "incomplete":
		kind = "response." + footer
		response["status"] = footer
		response["output"] = []any{first}
	}
	return append(events, map[string]any{"type": kind, "response": response})
}

func partialOutputOutcome(footer string) (reject, success bool) {
	success = footer == "complete-footer" || footer == "item-done"
	return !success && footer != "failed" && footer != "incomplete", success
}

func TestResponsesUnfinishedOutputHTTP(t *testing.T) {
	for _, source := range []string{"added", "delta", "snapshot"} {
		for _, footer := range partialOutputFooters {
			for _, streaming := range []bool{false, true} {
				t.Run(fmt.Sprintf("%s/%s/stream=%t", source, footer, streaming), func(t *testing.T) {
					events := partialOutputEvents(source, footer, "resp_partial")
					var stream bytes.Buffer
					for _, event := range events {
						fmt.Fprintf(&stream, "data: %s\n\n", mustRequestJSON(t, event))
					}
					fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
						w.Header().Set("Content-Type", "text/event-stream")
						_, _ = w.Write(stream.Bytes())
						return "resp_partial", ""
					})
					ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
					defer cancel()
					payload := mustRequestJSON(t, map[string]any{"model": "gpt-5.4", "stream": streaming, "input": "partial output probe"})
					req, _ := http.NewRequestWithContext(ctx, http.MethodPost, fixture.public.URL+"/v1/responses", bytes.NewReader(payload))
					req.Header.Set("Authorization", "Bearer "+fixture.subscriptionKey)
					req.Header.Set("Content-Type", "application/json")
					response, err := fixture.public.Client().Do(req)
					if err != nil {
						t.Fatal("partial-output request failed")
					}
					defer response.Body.Close()
					body, err := io.ReadAll(response.Body)
					if err != nil {
						t.Fatal("partial-output body failed")
					}
					reject, success := partialOutputOutcome(footer)
					wantStatus := http.StatusOK
					if reject && !streaming {
						wantStatus = http.StatusBadGateway
					}
					if response.StatusCode != wantStatus {
						t.Fatal("unfinished output was not classified correctly")
					}
					if reject && !streaming && !bytes.Contains(body, []byte("upstream_response_failed")) {
						t.Fatal("aggregate did not expose the failure")
					}
					if streaming && reject {
						if bytes.Contains(body, []byte("response.completed")) {
							t.Fatal("unfinished output completed publicly")
						}
						if !bytes.Contains(body, []byte("finished prefix")) || !bytes.Contains(body, []byte("partial suffix")) {
							t.Fatal("observed prefix was discarded")
						}
						if response.Trailer.Get(protocolv1.FailurePhaseTrailer) != "upstream_stream" || response.Trailer.Get(protocolv1.DeliveryStateTrailer) != "delivered" || response.Trailer.Get(protocolv1.RetryAdviceTrailer) != "never" {
							t.Fatal("partial-output failure trailers missing")
						}
					}
					record := waitForProfileRequestRecord(t, fixture.store, fixture.subscriptionKeyID, response.Header.Get("X-Mini-Sub2Api-Request-Id"))
					if (record.Status == storage.RequestCompleted) != success {
						t.Fatal("partial output counted as success")
					}
					if streaming && !success {
						id := ""
						for _, line := range strings.Split(string(body), "\n") {
							if data, ok := strings.CutPrefix(line, "data: "); ok {
								var event struct {
									Type     string `json:"type"`
									Response struct {
										ID string `json:"id"`
									} `json:"response"`
								}
								if json.Unmarshal([]byte(data), &event) == nil && event.Type == "response.created" {
									id = event.Response.ID
								}
							}
						}
						assertPartialReferenceUnavailable(t, fixture.public.URL, fixture.public.Client(), fixture.subscriptionKey, id)
					}
				})
			}
		}
	}
}

func assertPartialReferenceUnavailable(t *testing.T, url string, client *http.Client, secret, id string) {
	t.Helper()
	if id == "" {
		t.Fatal("created response reference missing")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	payload := mustRequestJSON(t, map[string]any{"model": "gpt-5.4", "input": "continue", "previous_response_id": id})
	req, _ := http.NewRequestWithContext(ctx, http.MethodPost, url+"/v1/responses", bytes.NewReader(payload))
	req.Header.Set("Authorization", "Bearer "+secret)
	req.Header.Set("Content-Type", "application/json")
	response, err := client.Do(req)
	if err != nil {
		t.Fatal("reference rejection request failed")
	}
	defer response.Body.Close()
	body, err := io.ReadAll(response.Body)
	if err != nil || response.StatusCode != http.StatusServiceUnavailable || !bytes.Contains(body, []byte("state_unavailable")) {
		t.Fatal("unfinished output became reusable history")
	}
}

func TestResponsesUnfinishedOutputWebSocket(t *testing.T) {
	for _, source := range []string{"added", "delta", "snapshot"} {
		for _, footer := range partialOutputFooters {
			t.Run(source+"/"+footer, func(t *testing.T) {
				fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(conn *websocket.Conn, _ []byte, id string) {
					for _, event := range partialOutputEvents(source, footer, id) {
						_ = conn.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(event))
					}
				})
				conn := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, http.Header{"Originator": []string{"codex_exec"}})
				defer conn.CloseNow()
				ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
				defer cancel()
				if err := conn.Write(ctx, websocket.MessageText, []byte(`{"type":"response.create","model":"gpt-5.4","input":"partial output probe"}`)); err != nil {
					t.Fatal("create failed")
				}
				reject, success := partialOutputOutcome(footer)
				id, prefix := "", false
				for {
					_, body, err := conn.Read(ctx)
					if err != nil {
						if !reject || ctx.Err() != nil || !prefix {
							t.Fatal("unexpected partial-output close")
						}
						break
					}
					var event struct {
						Type     string `json:"type"`
						Response struct {
							ID string `json:"id"`
						} `json:"response"`
					}
					if json.Unmarshal(body, &event) != nil {
						t.Fatal("invalid public event")
					}
					if event.Type == "response.created" {
						id = event.Response.ID
					}
					prefix = prefix || event.Type == "response.output_item.done"
					if event.Type == "response.completed" || event.Type == "response.failed" || event.Type == "response.incomplete" {
						if reject || (event.Type == "response.completed") != success {
							t.Fatal("unfinished output completed publicly")
						}
						break
					}
				}
				if !success {
					assertPartialReferenceUnavailable(t, fixture.public.URL, fixture.public.Client(), fixture.subscriptionKey, id)
				}
			})
		}
	}
}
