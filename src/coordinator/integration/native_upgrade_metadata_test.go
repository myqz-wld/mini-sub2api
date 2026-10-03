//go:build nativeparity

package integration

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
)

const upgradeRequestedModel = "gpt-5.5"
const upgradeHandshakeModel = "synthetic-inference-model"
const upgradeEventModel = "synthetic-event-model"

// The upstream emits models only through handshake/event headers. Track unexpected
// auth-only connections so native model checks also enforce deferred admission.
type nativeUpgradeCapture struct {
	*nativeCapture
	subscription    bool
	modelMode       string
	reasoningHeader *string
	accounting      bool
	probes          int
	probeFrames     int
	sampling        int
	compactions     int
	activeSockets   map[int]bool
}

func newNativeUpgradeCapture(t *testing.T, subscription bool) *nativeUpgradeCapture {
	c := &nativeUpgradeCapture{nativeCapture: &nativeCapture{t: t}, subscription: subscription, activeSockets: map[int]bool{}}
	c.server, c.tap = newNativeTappedServer(t, http.HandlerFunc(c.serveUpgrade))
	return c
}

func (c *nativeUpgradeCapture) serveUpgrade(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/v1/responses" {
		c.nativeCapture.serve(w, r)
		return
	}
	if r.Method != http.MethodGet {
		c.t.Error("upgrade metadata fixture unexpectedly received HTTP inference")
		w.WriteHeader(http.StatusMethodNotAllowed)
		return
	}
	c.mu.Lock()
	c.connections++
	number := c.connections
	probe := c.subscription && r.Header.Get("Session-Id") == ""
	if probe {
		c.probes++
	}
	c.mu.Unlock()
	model := upgradeRequestedModel
	if strings.HasPrefix(c.modelMode, "handshake") {
		model = upgradeHandshakeModel
	}
	if probe {
		model = "synthetic-probe-model"
	}
	w.Header().Set("OpenAI-Model", model)
	if c.reasoningHeader != nil {
		w.Header().Set("X-Reasoning-Included", *c.reasoningHeader)
	}
	connection, err := websocket.Accept(w, r, &websocket.AcceptOptions{CompressionMode: websocket.CompressionContextTakeover})
	if err != nil {
		return
	}
	defer connection.CloseNow()
	connection.SetReadLimit(nativeCaptureLimit)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	for {
		kind, body, err := connection.Read(ctx)
		if err != nil {
			return
		}
		if kind != websocket.MessageText {
			c.t.Error("upgrade metadata fixture received non-text application data")
			return
		}
		if probe {
			c.mu.Lock()
			c.probeFrames++
			c.mu.Unlock()
			c.t.Error("authenticated upgrade probe sent an inference frame")
			return
		}
		for _, event := range c.upgradeEvents(r, body, number) {
			encoded, _ := json.Marshal(event)
			if connection.Write(ctx, websocket.MessageText, encoded) != nil {
				return
			}
		}
	}
}

func (c *nativeUpgradeCapture) upgradeEvents(r *http.Request, body []byte, connection int) []map[string]any {
	// Retain only the existing bounded, in-memory capture; no payload enters logs.
	c.capture(r, body, body, connection)
	var request map[string]any
	if json.Unmarshal(body, &request) != nil {
		c.t.Error("invalid upgrade metadata request JSON")
		return nil
	}
	prewarm := request["generate"] == false
	compact := false
	input, _ := request["input"].([]any)
	for _, raw := range input {
		item, _ := raw.(map[string]any)
		compact = compact || item["type"] == "compaction_trigger"
	}
	c.mu.Lock()
	if !prewarm {
		c.activeSockets[connection] = true
		if compact {
			c.compactions++
		} else {
			c.sampling++
		}
	}
	sampling, compactions, requestIndex := c.sampling, c.compactions, len(c.requests)
	c.mu.Unlock()
	if sampling > 6 || compactions > 2 {
		c.t.Error("upgrade metadata fixture exceeded its inference bound")
		return nil
	}
	if !prewarm && c.modelMode == "handshake-error" {
		// No created event or model-bearing application event precedes this error.
		return []map[string]any{{"type": "error", "status": 400, "error": map[string]any{
			"type": "invalid_request_error", "code": "invalid_prompt", "message": "synthetic rejected request",
		}}}
	}
	id := fmt.Sprintf("resp_upgrade_%d", requestIndex)
	output := []any{}
	tokens := 10
	if !prewarm {
		if compact {
			output = append(output, map[string]any{"type": "compaction", "id": "cmp_upgrade", "encrypted_content": "synthetic-compacted-state"})
		} else if c.accounting && sampling == 2 {
			output = append(output, map[string]any{"type": "function_call", "id": "fc_upgrade", "call_id": "call_upgrade", "name": "native_probe", "arguments": "{}"})
			tokens = 80
		} else {
			if c.accounting && sampling == 1 {
				// Matches native core/tests/common/responses.rs's encrypted reasoning
				// fixture. Accounting estimates about 575 tokens before the next user.
				ciphertext := base64.StdEncoding.EncodeToString([]byte(strings.Repeat("b", 550) + strings.Repeat("a", 2400)))
				output = append(output, map[string]any{"type": "reasoning", "id": "rs_upgrade", "summary": []any{}, "encrypted_content": ciphertext})
			}
			output = append(output, map[string]any{"type": "message", "id": fmt.Sprintf("msg_upgrade_%d", requestIndex), "role": "assistant", "phase": "final_answer", "content": []any{map[string]any{"type": "output_text", "text": "synthetic completed answer"}}})
		}
	}
	created := map[string]any{"type": "response.created", "response": map[string]any{"id": id}}
	switch c.modelMode {
	case "event":
		created["headers"] = map[string]any{"OpenAI-Model": upgradeEventModel}
	case "event-array":
		created["headers"] = map[string]any{"OpenAI-Model": []any{upgradeEventModel}}
	case "event-alias-array":
		created["headers"] = map[string]any{"X-OpenAI-Model": []any{[]any{upgradeEventModel}}}
	}
	events := []map[string]any{created}
	if c.modelMode == "handshake-rate-limits" {
		// Native continues after rate limits without inspecting its model headers.
		events = append([]map[string]any{{"type": "codex.rate_limits", "plan_type": "plus", "rate_limits": map[string]any{
			"allowed": true, "limit_reached": false, "primary": map[string]any{"used_percent": 1, "window_minutes": 60, "reset_at": 1700000000},
		}}}, events...)
	}
	for index, item := range output {
		events = append(events, map[string]any{"type": "response.output_item.done", "output_index": index, "item": item})
	}
	return append(events, map[string]any{"type": "response.completed", "response": map[string]any{"id": id, "status": "completed", "output": output, "usage": map[string]any{"input_tokens": tokens, "output_tokens": 0, "total_tokens": tokens}}})
}

func (c *nativeUpgradeCapture) assertOperations(t *testing.T, sampling, compactions int, requireReuse bool) {
	t.Helper()
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.sampling != sampling || c.compactions != compactions || c.probeFrames != 0 {
		t.Fatalf("native upgrade operations: sampling=%d compactions=%d probe_frames=%d", c.sampling, c.compactions, c.probeFrames)
	}
	if requireReuse && len(c.activeSockets) != 1 {
		t.Fatalf("native upgrade did not reuse an inference socket: sockets=%d", len(c.activeSockets))
	}
	if c.probes != 0 {
		t.Fatalf("native upgrade auth probe count=%d subscription=%t", c.probes, c.subscription)
	}
}

func nativeUpgradeOptions(t *testing.T, capture *nativeUpgradeCapture) nativeOptions {
	t.Helper()
	options := nativeOptions{endpoint: capture.server.URL, metadataEndpoint: capture.server.URL, model: upgradeRequestedModel, ws: true, base: "Synthetic upgrade metadata probe.", neutralPersonality: true}
	if capture.subscription {
		gateway := newNativeGateway(t, capture.server.URL, true)
		options.endpoint, options.bearer = gateway.server.URL, gateway.secret
	}
	return options
}

func TestNativeWebSocketUpgradeModelObservation(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, mode := range []string{"handshake", "handshake-rate-limits", "event", "event-array", "event-alias-array"} {
			t.Run(fmt.Sprintf("subscription=%t/%s", subscription, mode), func(t *testing.T) {
				capture := newNativeUpgradeCapture(t, subscription)
				capture.modelMode = mode
				options := nativeUpgradeOptions(t, capture)
				wantModel := upgradeEventModel
				if strings.HasPrefix(mode, "handshake") {
					wantModel = upgradeHandshakeModel
				}
				reroutes, completions := 0, 0
				turns := map[string]bool{}
				options.observe = func(event map[string]any) {
					switch event["method"] {
					case "model/rerouted":
						params, _ := event["params"].(map[string]any)
						if params["fromModel"] != upgradeRequestedModel || params["toModel"] != wantModel {
							t.Fatal("native model observation differs from the actual inference header")
						}
						turn, _ := params["turnId"].(string)
						if turn == "" || turns[turn] {
							t.Fatal("native model observation missing turn or repeated within one turn")
						}
						turns[turn] = true
						reroutes++
					case "turn/completed":
						completions++
						if reroutes != completions {
							t.Fatal("native completed a turn before its model observation")
						}
					}
				}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				client.turn(thread, "First synthetic metadata turn.")
				client.turn(thread, "Second synthetic metadata turn.")
				if reroutes != 2 || completions != 2 {
					t.Fatalf("native model observations=%d completed_turns=%d", reroutes, completions)
				}
				capture.assertOperations(t, 2, 0, true)
				t.Logf("actual native CLI: subscription=%t source=%s reroutes=2 turns=2 reused_inference_socket=true", subscription, mode)
			})
		}
	}
}

func TestNativeWebSocketUpgradeModelBeforeError(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		t.Run(fmt.Sprintf("subscription=%t", subscription), func(t *testing.T) {
			capture := newNativeUpgradeCapture(t, subscription)
			capture.modelMode = "handshake-error"
			options := nativeUpgradeOptions(t, capture)
			reroutes, failedTurns := 0, 0
			options.observe = func(event map[string]any) {
				switch event["method"] {
				case "model/rerouted":
					params, _ := event["params"].(map[string]any)
					if params["fromModel"] != upgradeRequestedModel || params["toModel"] != upgradeHandshakeModel {
						t.Fatal("native first-error observation lost the actual handshake model")
					}
					reroutes++
				case "turn/completed":
					params, _ := event["params"].(map[string]any)
					turn, _ := params["turn"].(map[string]any)
					if turn["status"] != "failed" || reroutes != 1 {
						t.Fatal("native error turn ended before its model observation")
					}
					failedTurns++
				}
			}
			client := startNativeClient(t, options)
			thread := client.thread(options)
			turn := failureStartTurn(t, client, thread, "Synthetic first-error turn.")
			if status := failureWaitTurn(t, client, thread, turn); status != "failed" {
				t.Fatalf("native first-error turn status=%s", status)
			}
			if reroutes != 1 || failedTurns != 1 {
				t.Fatalf("native first-error observations=%d failed_turns=%d", reroutes, failedTurns)
			}
			capture.assertOperations(t, 1, 0, false)
			t.Logf("actual native CLI: subscription=%t first_event=error model_observed=true failed_turns=1 sampling=1", subscription)
		})
	}
}

func TestNativeWebSocketUpgradeReasoningAccounting(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, present := range []bool{false, true} {
			t.Run(fmt.Sprintf("subscription=%t/present=%t", subscription, present), func(t *testing.T) {
				capture := newNativeUpgradeCapture(t, subscription)
				capture.accounting = true
				if present {
					// Native tests presence, even when the header spells false.
					value := "false"
					capture.reasoningHeader = &value
				}
				options := nativeUpgradeOptions(t, capture)
				options.configOverrides = map[string]string{"features.token_budget": "false", "features.remote_compaction_v2": "true", "model_auto_compact_token_limit": "300"}
				calls := 0
				options.observe = func(event map[string]any) {
					if event["method"] == "item/tool/call" {
						calls++
					}
				}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				client.turn(thread, "Synthetic accounting seed.")
				// Core resets the header flag at regular-turn start. The second turn's
				// tool loop observes the direct WS flag before checking context usage.
				// Deferred Subscription upgrades cannot forward a later upstream flag.
				client.turn(thread, "Synthetic accounting tool turn.")
				if calls != 1 {
					t.Fatalf("native accounting tool callbacks=%d", calls)
				}
				wantCompactions := 1
				if present && !subscription {
					wantCompactions = 0
				}
				capture.assertOperations(t, 3, wantCompactions, false)
				t.Logf("actual native CLI: subscription=%t reasoning_header_present=%t sampling=3 tool_callbacks=1 compactions=%d", subscription, present, wantCompactions)
			})
		}
	}
}
