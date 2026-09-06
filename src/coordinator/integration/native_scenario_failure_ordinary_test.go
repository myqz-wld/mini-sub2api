//go:build nativeparity

package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"path/filepath"
	"testing"
	"time"

	"github.com/coder/websocket"
	"mini-sub2api/src/coordinator/internal/storage"
)

type failureOrdinaryResult struct {
	status   int
	terminal string
	response map[string]any
	created  string
	call     string
	err      string
}

func failureOrdinarySend(client nativeOrdinaryClient, request map[string]any) failureOrdinaryResult {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	request["stream"] = client.stream
	if client.ws != nil {
		request["type"] = "response.create"
	}
	body, err := json.Marshal(request)
	if err != nil {
		return failureOrdinaryResult{err: "encode request"}
	}
	result := failureOrdinaryResult{}
	if client.ws != nil {
		if client.ws.Write(ctx, websocket.MessageText, body) != nil {
			return failureOrdinaryResult{err: "write frame"}
		}
		result.status = http.StatusSwitchingProtocols
		for n := 0; n < 128; n++ {
			kind, body, err := client.ws.Read(ctx)
			if err != nil {
				result.terminal = "disconnect"
				return result
			}
			if kind != websocket.MessageText || !result.observe(body) {
				continue
			}
			return result
		}
		return failureOrdinaryResult{err: "event capacity"}
	}
	req, _ := http.NewRequestWithContext(ctx, http.MethodPost, client.gateway.server.URL+"/v1/responses", bytes.NewReader(body))
	req.Header = client.headers.Clone()
	req.Header.Set("Content-Type", "application/json")
	response, err := http.DefaultClient.Do(req)
	if err != nil {
		return failureOrdinaryResult{err: "HTTP transport"}
	}
	defer response.Body.Close()
	result.status = response.StatusCode
	raw, err := io.ReadAll(io.LimitReader(response.Body, nativeCaptureLimit+1))
	if len(raw) > nativeCaptureLimit {
		return failureOrdinaryResult{err: "response capacity"}
	}
	if !client.stream {
		var value map[string]any
		if json.Unmarshal(raw, &value) == nil {
			result.response = value
			if value["id"] != nil {
				status, _ := value["status"].(string)
				if status == "" {
					status = "completed"
				}
				result.terminal = "response." + status
			} else if value["error"] != nil {
				result.terminal = "error"
			}
		}
	} else {
		for _, line := range bytes.Split(raw, []byte("\n")) {
			if bytes.HasPrefix(line, []byte("data: ")) {
				result.observe(line[6:])
			}
		}
	}
	if result.terminal == "" || err != nil {
		result.terminal = "disconnect"
	}
	return result
}

func (r *failureOrdinaryResult) observe(body []byte) bool {
	var event map[string]any
	if json.Unmarshal(body, &event) != nil {
		return false
	}
	if response, ok := event["response"].(map[string]any); ok {
		if id, ok := response["id"].(string); ok {
			r.created = id
		}
	}
	if event["type"] == "response.output_item.done" {
		item, _ := event["item"].(map[string]any)
		if call, ok := item["call_id"].(string); ok {
			r.call = call
		}
	}
	switch event["type"] {
	case "response.completed", "response.failed", "response.incomplete", "error":
		r.terminal, _ = event["type"].(string)
		r.response, _ = event["response"].(map[string]any)
		return true
	}
	return false
}

func failureRequest(model, session string) map[string]any {
	value := map[string]any{"model": model, "instructions": "synthetic failure base", "tools": []any{}, "input": []any{map[string]any{"role": "user", "content": "synthetic failure user"}}}
	if session != "" {
		value["client_metadata"] = map[string]any{"session_id": session}
	}
	return value
}

func TestNativeScenarioFailureOrdinaryTerminal(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, delivery := range []string{"json", "sse", "ws"} {
			for _, kind := range []string{"failed", "incomplete", "disconnect"} {
				t.Run(fmt.Sprintf("subscription=%t/%s/%s", subscription, delivery, kind), func(t *testing.T) {
					capture := newNativeFailureCapture(t, kind, 1, 0)
					gateway := newNativeGateway(t, capture.server.URL, subscription)
					client := ordinaryClient(t, gateway, delivery == "ws", delivery != "json", nil)
					model := "gpt-5.4"
					if delivery == "ws" {
						model = "gpt-5.6-sol"
					}
					failed := failureOrdinarySend(client, failureRequest(model, "ordinary-failure"))
					if failed.err != "" || failed.terminal == "response.completed" || len(businessWires(capture.snapshot())) != 1 {
						t.Fatalf("ordinary failure became success or replay: status=%d terminal=%s control=%s", failed.status, failed.terminal, failed.err)
					}
					wantJSONStatus := http.StatusOK
					if subscription && kind == "disconnect" {
						wantJSONStatus = http.StatusBadGateway
					}
					if delivery == "json" && failed.status != wantJSONStatus {
						t.Fatalf("ordinary failed JSON status=%d", failed.status)
					}
					if delivery != "json" && kind != "disconnect" && failed.terminal != "response."+kind {
						t.Fatalf("ordinary terminal changed: got=%s kind=%s", failed.terminal, kind)
					}
					if delivery == "ws" && (failed.terminal == "disconnect" || failed.terminal == "error") {
						client = ordinaryClient(t, gateway, true, true, nil)
					}
					recovered := failureOrdinarySend(client, failureRequest(model, "ordinary-failure"))
					if recovered.err != "" || recovered.terminal != "response.completed" || len(businessWires(capture.snapshot())) != 2 {
						t.Fatalf("ordinary future full request not healthy: status=%d terminal=%s control=%s", recovered.status, recovered.terminal, recovered.err)
					}
					wires := businessWires(capture.snapshot())
					if previous := wires[1].value["previous_response_id"]; previous != nil {
						// A reconnect may legitimately complete a fresh hidden setup before reuse.
						setup := false
						all := capture.snapshot()
						for i, wire := range all {
							if previous == fmt.Sprintf("resp_failure_%d", i+1) && wire.value["generate"] == false && wire.connection == wires[1].connection {
								setup = true
							}
						}
						if !setup {
							t.Fatal("failed ordinary response became automatic incremental baseline")
						}
					}
					t.Logf("ordinary terminal=%s status=%d no_gateway_replay=true future_full_completed=true", failed.terminal, failed.status)
				})
			}
		}
	}
}

func TestNativeScenarioFailureUnsuccessfulToolReference(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, ws := range []bool{false, true} {
			t.Run(fmt.Sprintf("subscription=%t/ws=%t", subscription, ws), func(t *testing.T) {
				capture := newNativeFailureCapture(t, "failed", 1, 0)
				capture.partialTool = true
				gateway := newNativeGateway(t, capture.server.URL, subscription)
				client := ordinaryClient(t, gateway, ws, true, nil)
				first := failureOrdinarySend(client, failureRequest("gpt-5.4", "failed-tool-session"))
				if first.err != "" || first.terminal != "response.failed" || first.created == "" || first.call == "" {
					t.Fatal("unsuccessful response did not expose its created identity and partial tool item")
				}
				second := failureRequest("gpt-5.4", "failed-tool-session")
				second["previous_response_id"] = first.created
				second["input"] = []any{map[string]any{"type": "function_call_output", "call_id": first.call, "output": "synthetic unsuccessful tool output"}}
				result := failureOrdinarySend(client, second)
				wantCount := 2
				if subscription {
					wantCount = 1
					if result.terminal == "response.completed" {
						t.Fatal("Subscription accepted continuation/tool output of failed response")
					}
				} else if result.terminal != "response.completed" {
					t.Fatal("API-key did not transparently deliver mock-accepted continuation")
				}
				if result.err != "" || len(businessWires(capture.snapshot())) != wantCount {
					t.Fatal("unsuccessful reference admission count changed")
				}
				t.Logf("unsuccessful response reference: subscription=%t upstream_continuation_admitted=%t", subscription, !subscription)
			})
		}
	}
}

func failureSecondKey(t *testing.T, gateway nativeGateway) nativeGateway {
	t.Helper()
	store, err := storage.Open(context.Background(), filepath.Dir(gateway.stateDir), time.Now)
	if err != nil {
		t.Fatal("open existing synthetic gateway store")
	}
	defer store.Close()
	route, err := store.AuthenticateConnection(context.Background(), gateway.secret)
	if err != nil {
		t.Fatal("resolve synthetic distribution binding")
	}
	key, err := store.CreateAPIKey(context.Background(), route.CredentialID, "Independent failure control")
	if err != nil {
		t.Fatal("create second synthetic distribution key")
	}
	gateway.secret = key.Secret
	return gateway
}
