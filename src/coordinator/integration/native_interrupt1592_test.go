//go:build nativeparity

package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"testing"
	"time"

	"github.com/coder/websocket"
)

const (
	interrupt1592Response = "resp_native_interrupt"
	interrupt1592Reason   = "rs_native_interrupt"
	interrupt1592Anchor   = "synthetic completed before interruption"
	interrupt1592First    = "synthetic initial interrupt input"
	interrupt1592Steer    = "synthetic steering interrupt input"
)

// Mirrors core/tests/suite/pending_input.rs::steer_interrupts_and_drains_websocket
// using the official app-server CLI. Payloads remain bounded and memory-only.
type nativeInterrupt1592Capture struct {
	*nativeCapture
	discard  bool
	stage    int
	prewarms int
}

func newNativeInterrupt1592Capture(t *testing.T, discard bool) *nativeInterrupt1592Capture {
	c := &nativeInterrupt1592Capture{nativeCapture: &nativeCapture{t: t}, discard: discard}
	c.server, c.tap = newNativeTappedServer(t, http.HandlerFunc(c.serveInterrupt))
	return c
}

func (c *nativeInterrupt1592Capture) serveInterrupt(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path == "/backend-api/wham/settings/user" {
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"commit_attribution_enabled":false}`)
		return
	}
	if r.URL.Path != "/v1/responses" {
		w.WriteHeader(http.StatusNotFound)
		return
	}
	if r.Method != http.MethodGet {
		c.t.Error("native interrupt unexpectedly fell back from WebSocket")
		w.WriteHeader(http.StatusMethodNotAllowed)
		return
	}
	conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{CompressionMode: websocket.CompressionContextTakeover})
	if err != nil {
		return
	}
	defer conn.CloseNow()
	conn.SetReadLimit(nativeCaptureLimit)
	c.mu.Lock()
	c.connections++
	number := c.connections
	c.mu.Unlock()
	for {
		ctx, cancel := context.WithTimeout(r.Context(), 12*time.Second)
		kind, body, err := conn.Read(ctx)
		cancel()
		if err != nil || kind != websocket.MessageText {
			return
		}
		events := c.interruptEvents(r, body, number)
		if len(events) == 0 {
			return
		}
		for _, event := range events {
			data, _ := json.Marshal(event)
			ctx, cancel := context.WithTimeout(r.Context(), 5*time.Second)
			err := conn.Write(ctx, websocket.MessageText, data)
			cancel()
			if err != nil {
				return
			}
		}
	}
}

func interrupt1592Message(id, text, phase string) map[string]any {
	return map[string]any{"type": "message", "id": id, "role": "assistant", "phase": phase,
		"content": []any{map[string]any{"type": "output_text", "text": text}}}
}

func interrupt1592Reasoning(done bool) map[string]any {
	item := map[string]any{"type": "reasoning", "id": interrupt1592Reason,
		"summary": []any{map[string]any{"type": "summary_text", "text": "synthetic interrupt reasoning"}}}
	if done {
		item["encrypted_content"] = "c3ludGhldGljLWludGVycnVwdC1yZWFzb25pbmc="
	}
	return item
}

func interrupt1592Terminal(id string, output []any, interrupted bool) map[string]any {
	response := map[string]any{"id": id, "status": "completed", "output": output,
		"usage": map[string]any{"input_tokens": 20, "output_tokens": 22, "total_tokens": 42}}
	kind := "response.completed"
	if interrupted {
		kind, response["status"] = "response.incomplete", "incomplete"
		response["incomplete_details"] = map[string]any{"reason": "interrupted"}
	}
	return map[string]any{"type": kind, "response": response}
}

func (c *nativeInterrupt1592Capture) interruptEvents(r *http.Request, body []byte, connection int) []map[string]any {
	var value map[string]any
	if json.Unmarshal(body, &value) != nil {
		c.t.Error("native interrupt capture received invalid JSON")
		return nil
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if len(c.requests) >= 8 || 2*len(body) > nativeCaptureLimit-c.bytes {
		c.t.Error("native interrupt capture capacity exceeded")
		return nil
	}
	c.bytes += 2 * len(body)
	c.requests = append(c.requests, nativeWire{method: r.Method, headers: r.Header.Clone(),
		body: append([]byte(nil), body...), encodedBody: append([]byte(nil), body...), value: value, connection: connection})
	created := func(id string) map[string]any {
		return map[string]any{"type": "response.created", "response": map[string]any{"id": id, "status": "in_progress"}}
	}
	anchor := interrupt1592Message("msg_native_interrupt_done", interrupt1592Anchor, "commentary")
	if value["type"] == "response.interrupt" {
		if c.stage != 1 || len(value) != 3 || value["response_id"] != interrupt1592Response || value["mode"] != "discard_partial_items" {
			c.t.Error("native interrupt frame shape, ownership or sequence differs")
			return nil
		}
		c.stage = 2
		output := []any{anchor}
		events := []map[string]any{{"type": "response.interrupt.accepted", "response_id": interrupt1592Response, "sequence_number": 3}}
		if c.discard {
			events = append(events, map[string]any{"type": "response.output_item.interrupted", "response_id": interrupt1592Response,
				"item_id": interrupt1592Reason, "output_index": 1, "sequence_number": 4})
		} else {
			reason := interrupt1592Reasoning(true)
			output = append(output, reason)
			events = append(events, map[string]any{"type": "response.output_item.done", "output_index": 1, "item": reason})
		}
		return append(events, interrupt1592Terminal(interrupt1592Response, output, true))
	}
	if value["type"] != "response.create" {
		c.t.Error("native interrupt peer received an unexpected control")
		return nil
	}
	if value["generate"] == false {
		if c.stage != 0 || c.prewarms != 0 {
			c.t.Error("native interrupt peer received an extra or late prewarm")
			return nil
		}
		c.prewarms++
		return []map[string]any{created("resp_native_interrupt_warm"), interrupt1592Terminal("resp_native_interrupt_warm", []any{}, false)}
	}
	c.business++
	switch c.stage {
	case 0:
		c.stage = 1
		return []map[string]any{created(interrupt1592Response),
			{"type": "response.output_item.done", "output_index": 0, "item": anchor},
			{"type": "response.output_item.added", "output_index": 1, "item": interrupt1592Reasoning(false)}}
	case 2:
		if value["previous_response_id"] != interrupt1592Response {
			c.t.Error("native interrupt follow-up lost the interrupted baseline")
			return nil
		}
		c.stage = 3
		answer := interrupt1592Message("msg_native_interrupt_followup", "synthetic interrupted turn completed", "final_answer")
		return []map[string]any{created("resp_native_interrupt_followup"),
			{"type": "response.output_item.done", "output_index": 0, "item": answer},
			interrupt1592Terminal("resp_native_interrupt_followup", []any{answer}, false)}
	default:
		c.t.Error("native interrupt replayed or overlapped an inference")
		return nil
	}
}

func TestNative1592InstantInterrupt(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, discard := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/discard=%t", route, discard), func(t *testing.T) {
				capture := newNativeInterrupt1592Capture(t, discard)
				started, completedReasoning, completedAnchor := false, 0, 0
				options := nativeOptions{endpoint: capture.server.URL, model: "gpt-6-sol", ws: true,
					base: "synthetic native interrupt base", configOverrides: map[string]string{"features.instant_interrupt": "true"},
					threadParams: map[string]any{"experimentalRawEvents": true},
					observe: func(event map[string]any) {
						params, _ := event["params"].(map[string]any)
						item, _ := params["item"].(map[string]any)
						if event["method"] == "item/started" && item["type"] == "reasoning" {
							started = true
						}
						if event["method"] == "rawResponseItem/completed" {
							if item["type"] == "reasoning" {
								completedReasoning++
							}
							if interrupt1592TextCount(item, interrupt1592Anchor) == 1 {
								completedAnchor++
							}
						}
					}}
				var gateway nativeGateway
				if route != "direct" {
					gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
					options.endpoint, options.bearer = gateway.server.URL, gateway.secret
				}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				turn := failureStartTurn(t, client, thread, interrupt1592First)
				// The app-server notification proves native processed the partial
				// reasoning before the test steers the active turn.
				for attempts := 0; !started && attempts < 256; attempts++ {
					client.handle(client.read())
				}
				if !started {
					t.Fatal("native interrupt reasoning barrier was not observed")
				}
				if steered := failureStartTurn(t, client, thread, interrupt1592Steer); steered != turn {
					t.Fatal("native interrupt steering replaced the active turn")
				}
				if failureWaitTurn(t, client, thread, turn) != "completed" {
					t.Fatal("native interrupted and steered turn did not complete")
				}
				wantReasoning := 1
				if discard {
					wantReasoning = 0
				}
				if completedReasoning != wantReasoning || completedAnchor != 1 {
					t.Fatalf("native retained completed item counts differ: reasoning=%d anchor=%d", completedReasoning, completedAnchor)
				}
				assertNativeInterrupt1592Wires(t, capture, route, gateway)
				t.Logf("native interrupt: business=2 controls=1 same_socket=true completed_reasoning=%d completed_anchor=1 discarded=%t", completedReasoning, discard)
			})
		}
	}
}

func interrupt1592TextCount(value any, text string) int {
	count := 0
	switch item := value.(type) {
	case map[string]any:
		for key, child := range item {
			if key == "text" && child == text {
				count++
			} else {
				count += interrupt1592TextCount(child, text)
			}
		}
	case []any:
		for _, child := range item {
			count += interrupt1592TextCount(child, text)
		}
	}
	return count
}

func assertNativeInterrupt1592Wires(t *testing.T, capture *nativeInterrupt1592Capture, route string, gateway nativeGateway) {
	t.Helper()
	wires := capture.snapshot()
	capture.mu.Lock()
	connections, stage := capture.connections, capture.stage
	capture.mu.Unlock()
	if connections != 1 || stage != 3 {
		t.Fatalf("native interrupt lifecycle differs: connections=%d stage=%d", connections, stage)
	}
	var business []nativeWire
	controls := 0
	for _, wire := range wires {
		if wire.connection != wires[0].connection || wire.method != http.MethodGet {
			t.Fatal("native interrupt did not reuse one upstream WebSocket")
		}
		if wire.value["type"] == "response.interrupt" {
			controls++
		} else if wire.value["generate"] != false {
			business = append(business, wire)
		}
	}
	if len(business) != 2 || controls != 1 {
		t.Fatalf("native interrupt request counts differ: business=%d controls=%d", len(business), controls)
	}
	first, next := business[0].value, business[1].value
	if next["previous_response_id"] != interrupt1592Response || interrupt1592TextCount(next["input"], interrupt1592Steer) != 1 || interrupt1592TextCount(next["input"], interrupt1592First) != 0 {
		t.Fatal("native interrupt continuation did not contain only the new user suffix")
	}
	// Native can produce this incremental request only when its local history
	// agrees with the drained done items. Replayed partial/done output breaks it.
	items, _ := next["input"].([]any)
	for _, raw := range items {
		item, _ := raw.(map[string]any)
		if item["type"] == "reasoning" || item["role"] == "assistant" {
			t.Fatal("native interrupt follow-up replayed drained or discarded output")
		}
	}
	before, after := transportMetadata(t, first), transportMetadata(t, next)
	for _, key := range []string{"session_id", "thread_id", "turn_id"} {
		if before[key] == nil || before[key] != after[key] {
			t.Fatal("native interrupt steering changed the active turn ownership")
		}
	}
	packets := capture.tap.packets(t)
	if len(packets) != len(wires) {
		t.Fatal("native interrupt upstream TCP capture missed a frame")
	}
	for i, packet := range packets {
		if !bytes.Equal(packet.payload, wires[i].encodedBody) {
			t.Fatal("native interrupt TCP/application capture differs")
		}
	}
	if route == "direct" {
		return
	}
	packets = gateway.tap.packets(t)
	if len(packets) != len(wires) {
		t.Fatal("native interrupt gateway added or lost a frame")
	}
	projection := &transportProjection{identities: map[string]map[string]string{}}
	for i, packet := range packets {
		if packet.connection != packets[0].connection || packet.method != http.MethodGet {
			t.Fatal("native interrupt gateway changed the caller WebSocket")
		}
		if route == "api-key" {
			if !bytes.Equal(packet.payload, wires[i].encodedBody) {
				t.Fatal("API-key native interrupt frame bytes changed")
			}
			continue
		}
		caller := decodeNativePacket(t, packet)
		if wires[i].value["type"] == "response.interrupt" {
			if len(caller) != 3 || caller["type"] != "response.interrupt" || caller["mode"] != "discard_partial_items" {
				t.Fatal("native interrupt caller control shape changed")
			}
			projection.identity(t, caller["response_id"], wires[i].value["response_id"], "response", "interrupt.response_id")
		} else {
			projection.compare(t, caller, wires[i].value, fmt.Sprintf("request[%d]", i))
		}
	}
	if route == "api-key" {
		assertProfileStateFileCount(t, gateway.stateDir, 0)
	}
}
