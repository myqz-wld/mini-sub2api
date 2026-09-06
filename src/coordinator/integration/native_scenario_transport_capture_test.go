//go:build nativeparity

package integration

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"testing"

	"github.com/coder/websocket"
	"github.com/klauspost/compress/zstd"
)

// Each scripted response offers a different token. Two tool calls make replacement of
// the first token observable. Only decoded, bounded memory is retained by the fixture.
type nativeTransportCapture struct {
	*nativeCapture
	handshakeToken bool
	prewarmToken   bool
	closeAfterTool bool
	closed         chan struct{}
}

func newNativeTransportCapture(t *testing.T, handshakeToken, closeAfterTool bool) *nativeTransportCapture {
	c := &nativeTransportCapture{nativeCapture: &nativeCapture{t: t}, handshakeToken: handshakeToken, closeAfterTool: closeAfterTool, closed: make(chan struct{})}
	c.server, c.tap = newNativeTappedServer(t, http.HandlerFunc(c.serveTransport))
	return c
}

func (c *nativeTransportCapture) serveTransport(w http.ResponseWriter, r *http.Request) {
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
		if c.handshakeToken {
			w.Header().Set("X-Codex-Turn-State", "transport-handshake-token")
		}
		connection, err := websocket.Accept(w, r, &websocket.AcceptOptions{CompressionMode: websocket.CompressionContextTakeover})
		if err != nil {
			return
		}
		defer connection.CloseNow()
		connection.SetReadLimit(nativeCaptureLimit)
		c.mu.Lock()
		c.connections++
		number := c.connections
		c.mu.Unlock()
		for {
			kind, body, err := connection.Read(context.Background())
			if err != nil || kind != websocket.MessageText {
				return
			}
			events, _, closeAfter := c.captureTransport(r, body, body, number)
			for _, event := range events {
				encoded, _ := json.Marshal(event)
				if connection.Write(context.Background(), websocket.MessageText, encoded) != nil {
					return
				}
			}
			if closeAfter {
				_ = connection.CloseNow()
				close(c.closed)
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
			w.WriteHeader(400)
			return
		}
		defer decoder.Close()
		body, err = decoder.DecodeAll(raw, nil)
		if err != nil || len(body) > nativeCaptureLimit {
			w.WriteHeader(400)
			return
		}
	}
	events, token, _ := c.captureTransport(r, body, raw, 0)
	if token != "" {
		w.Header().Set("X-Codex-Turn-State", token)
	}
	w.Header().Set("Content-Type", "text/event-stream")
	for _, event := range events {
		encoded, _ := json.Marshal(event)
		_, _ = fmt.Fprintf(w, "data: %s\n\n", encoded)
	}
}

func (c *nativeTransportCapture) captureTransport(r *http.Request, body, encoded []byte, connection int) ([]map[string]any, string, bool) {
	var value map[string]any
	if json.Unmarshal(body, &value) != nil {
		c.t.Error("transport capture received invalid JSON")
		return nil, "", false
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if len(c.requests) >= 128 || len(body)+len(encoded) > nativeCaptureLimit-c.bytes {
		c.t.Error("transport application capture capacity exceeded")
		return nil, "", false
	}
	c.bytes += len(body) + len(encoded)
	c.requests = append(c.requests, nativeWire{encodedBody: append([]byte(nil), encoded...), method: r.Method, headers: r.Header.Clone(), body: append([]byte(nil), body...), value: value, connection: connection})
	id := fmt.Sprintf("resp_transport_%d", len(c.requests))
	output := []any{}
	token := ""
	closeAfter := false
	if value["generate"] != false {
		c.business++
		n := c.business
		token = fmt.Sprintf("transport-token-%d", n)
		if n <= 2 {
			reasoning := map[string]any{"type": "reasoning", "id": fmt.Sprintf("rs_transport_%d", n), "summary": []any{map[string]any{"type": "summary_text", "text": "synthetic reasoning summary"}}, "content": []any{map[string]any{"type": "reasoning_text", "text": "synthetic reasoning content"}}, "encrypted_content": fmt.Sprintf("transport-opaque-%d", n)}
			if n == 2 {
				delete(reasoning, "content")
				reasoning["summary"] = []any{}
			}
			output = append(output,
				reasoning,
				map[string]any{"type": "function_call", "id": fmt.Sprintf("fc_transport_%d", n), "call_id": fmt.Sprintf("call_transport_%d", n), "name": "native_probe", "arguments": "{}"})
		} else {
			output = append(output, map[string]any{"type": "message", "id": fmt.Sprintf("msg_transport_%d", n), "role": "assistant", "phase": "final_answer", "content": []any{map[string]any{"type": "output_text", "text": "synthetic transport answer"}}})
		}
		closeAfter = c.closeAfterTool && n == 1
	} else if c.prewarmToken {
		token = "transport-prewarm-token"
	}
	events := []map[string]any{{"type": "response.created", "response": map[string]any{"id": id}}}
	if token != "" {
		// Mixed casing also verifies case-insensitive response.metadata acquisition.
		events = append(events, map[string]any{"type": "response.metadata", "headers": map[string]any{"X-CoDeX-TuRn-StAtE": token}})
	}
	for index, item := range output {
		events = append(events, map[string]any{"type": "response.output_item.done", "output_index": index, "item": item})
	}
	events = append(events, map[string]any{"type": "response.completed", "response": map[string]any{"id": id, "output": output, "usage": map[string]any{"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}})
	return events, token, closeAfter
}
