//go:build nativeparity

package integration

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/klauspost/compress/zstd"
)

// All scripted failures occur after response.created; partial items never become a
// completed response. Capture storage is bounded and no payload enters test output.
type nativeFailureCapture struct {
	*nativeCapture
	kind        string
	failCount   int
	holdCount   int
	partialTool bool
	started     chan int
	release     chan struct{}
	once        sync.Once
}

func newNativeFailureCapture(t *testing.T, kind string, failCount, holdCount int) *nativeFailureCapture {
	c := &nativeFailureCapture{nativeCapture: &nativeCapture{t: t}, kind: kind, failCount: failCount, holdCount: holdCount, started: make(chan int, 16), release: make(chan struct{})}
	c.server, c.tap = newNativeTappedServer(t, http.HandlerFunc(c.serveFailure))
	t.Cleanup(c.unblock)
	return c
}

func (c *nativeFailureCapture) unblock() { c.once.Do(func() { close(c.release) }) }

func (c *nativeFailureCapture) waitStarted(t *testing.T) int {
	t.Helper()
	select {
	case n := <-c.started:
		return n
	case <-time.After(8 * time.Second):
		t.Fatal("failure fixture did not receive business inference")
		return 0
	}
}

func (c *nativeFailureCapture) serveFailure(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path == "/backend-api/wham/settings/user" {
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"commit_attribution_enabled":false}`)
		return
	}
	if r.URL.Path != "/v1/responses" {
		w.WriteHeader(http.StatusNotFound)
		return
	}
	if r.Method == http.MethodGet {
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
			events, business, held, closeAfter := c.failureEvents(r, body, body, number)
			var delayed map[string]any
			if held && c.kind == "overlap" {
				delayed, events = events[len(events)-1], events[:len(events)-1]
			}
			for _, event := range events {
				ctx, cancel := context.WithTimeout(r.Context(), 5*time.Second)
				data, _ := json.Marshal(event)
				err := conn.Write(ctx, websocket.MessageText, data)
				cancel()
				if err != nil {
					return
				}
			}
			if business > 0 {
				c.started <- business
			}
			if held {
				select {
				case <-c.release:
				case <-r.Context().Done():
				}
			}
			if delayed != nil {
				ctx, cancel := context.WithTimeout(r.Context(), 5*time.Second)
				data, _ := json.Marshal(delayed)
				err := conn.Write(ctx, websocket.MessageText, data)
				cancel()
				if err != nil {
					return
				}
			}
			if closeAfter || held && delayed == nil {
				return
			}
		}
	}
	if r.Method != http.MethodPost {
		w.WriteHeader(http.StatusMethodNotAllowed)
		return
	}
	raw, err := io.ReadAll(io.LimitReader(r.Body, nativeCaptureLimit+1))
	if err != nil || len(raw) > nativeCaptureLimit {
		w.WriteHeader(http.StatusRequestEntityTooLarge)
		return
	}
	body := raw
	if r.Header.Get("Content-Encoding") == "zstd" {
		decoder, err := zstd.NewReader(nil, zstd.WithDecoderMaxMemory(nativeCaptureLimit), zstd.WithDecoderConcurrency(1))
		if err != nil {
			w.WriteHeader(http.StatusBadRequest)
			return
		}
		defer decoder.Close()
		body, err = decoder.DecodeAll(raw, nil)
		if err != nil || len(body) > nativeCaptureLimit {
			w.WriteHeader(http.StatusBadRequest)
			return
		}
	}
	events, business, held, _ := c.failureEvents(r, body, raw, 0)
	var request map[string]any
	_ = json.Unmarshal(body, &request)
	if request["stream"] == false {
		w.Header().Set("Content-Type", "application/json")
		if business > 0 {
			c.started <- business
		}
		if held {
			select {
			case <-c.release:
			case <-r.Context().Done():
				return
			}
		}
		if response, ok := events[len(events)-1]["response"].(map[string]any); ok && events[len(events)-1]["type"] != "response.created" {
			_ = json.NewEncoder(w).Encode(response)
		} else {
			_, _ = io.WriteString(w, `{"status":`)
		}
		return
	}
	var delayed map[string]any
	if held && c.kind == "overlap" {
		delayed, events = events[len(events)-1], events[:len(events)-1]
	}
	w.Header().Set("Content-Type", "text/event-stream")
	for _, event := range events {
		data, _ := json.Marshal(event)
		if _, err := fmt.Fprintf(w, "data: %s\n\n", data); err != nil {
			return
		}
	}
	if f, ok := w.(http.Flusher); ok {
		f.Flush()
	}
	if business > 0 {
		c.started <- business
	}
	if held {
		select {
		case <-c.release:
		case <-r.Context().Done():
		}
	}
	if delayed != nil {
		data, _ := json.Marshal(delayed)
		_, _ = fmt.Fprintf(w, "data: %s\n\n", data)
	}
}

func (c *nativeFailureCapture) failureEvents(r *http.Request, body, encoded []byte, connection int) ([]map[string]any, int, bool, bool) {
	var value map[string]any
	if json.Unmarshal(body, &value) != nil {
		c.t.Error("failure capture received invalid JSON")
		return nil, 0, false, true
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if len(c.requests) >= 128 || len(body)+len(encoded) > nativeCaptureLimit-c.bytes {
		c.t.Error("failure capture exceeded bounded capacity")
		return nil, 0, false, true
	}
	c.bytes += len(body) + len(encoded)
	c.requests = append(c.requests, nativeWire{encodedBody: append([]byte(nil), encoded...), body: append([]byte(nil), body...), value: value, headers: r.Header.Clone(), method: r.Method, connection: connection})
	id := fmt.Sprintf("resp_failure_%d", len(c.requests))
	n, held, failing := 0, false, false
	if value["generate"] != false {
		c.business++
		n = c.business
		held, failing = n <= c.holdCount, n <= c.failCount
	}
	events := []map[string]any{{"type": "response.created", "response": map[string]any{"id": id, "status": "in_progress"}}}
	output := []any{}
	if n > 0 && (!held || c.kind == "overlap") {
		item := map[string]any{"type": "message", "id": fmt.Sprintf("msg_failure_%d", n), "role": "assistant", "phase": "final_answer", "content": []any{map[string]any{"type": "output_text", "text": "synthetic failure scenario answer"}}}
		if failing {
			item["phase"] = "commentary"
		}
		if failing && c.partialTool {
			item = map[string]any{"type": "function_call", "id": "fc_failure_partial", "call_id": "call_failure_partial", "name": "native_probe", "arguments": "{}"}
		}
		output = append(output, item)
		events = append(events, map[string]any{"type": "response.output_item.done", "output_index": 0, "item": item})
	}
	if held && c.kind != "overlap" || failing && c.kind == "disconnect" {
		return events, n, held, true
	}
	response := map[string]any{"id": id, "output": output, "status": "completed", "usage": map[string]any{"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}
	kind := "response.completed"
	if failing {
		kind = "response." + c.kind
		response["status"] = c.kind
		if c.kind == "failed" {
			response["error"] = map[string]any{"code": "invalid_prompt", "message": "synthetic model rejection"}
		} else {
			response["incomplete_details"] = map[string]any{"reason": "max_output_tokens"}
		}
	}
	events = append(events, map[string]any{"type": kind, "response": response})
	return events, n, held, false
}

func failureStartTurn(t *testing.T, client *nativeClient, thread, text string) string {
	t.Helper()
	result := client.call("turn/start", map[string]any{"threadId": thread, "input": []any{map[string]any{"type": "text", "text": text}}})
	turn, _ := result["turn"].(map[string]any)
	id, _ := turn["id"].(string)
	if id == "" {
		t.Fatal("failure native turn identity missing")
	}
	return id
}

func failureWaitTurn(t *testing.T, client *nativeClient, thread, turn string) string {
	t.Helper()
	for attempts := 0; attempts < 256; attempts++ {
		for i, event := range client.pending {
			params, _ := event["params"].(map[string]any)
			terminal, _ := params["turn"].(map[string]any)
			if params["threadId"] == thread && terminal["id"] == turn {
				client.pending = append(client.pending[:i], client.pending[i+1:]...)
				status, _ := terminal["status"].(string)
				return status
			}
		}
		client.handle(client.read())
	}
	t.Fatal("failure native control event bound exceeded")
	return ""
}
