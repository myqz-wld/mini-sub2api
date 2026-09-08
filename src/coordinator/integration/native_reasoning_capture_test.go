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

// This responder always supplies ciphertext, even when not requested, so API-key transparency
// cannot pass merely because a friendly upstream already omitted optional output.
type reasoningCapture struct {
	*nativeTransportCapture
	emptyFooter bool
	effective   [][]any
	previous    map[string][]any
}

func newReasoningCapture(t *testing.T, emptyFooter bool) *reasoningCapture {
	c := &reasoningCapture{nativeTransportCapture: &nativeTransportCapture{nativeCapture: &nativeCapture{t: t}}, emptyFooter: emptyFooter, previous: make(map[string][]any)}
	c.server, c.tap = newNativeTappedServer(t, http.HandlerFunc(c.serveReasoning))
	return c
}

func (c *reasoningCapture) serveReasoning(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/v1/responses" {
		w.WriteHeader(http.StatusNotFound)
		return
	}
	if r.Method == http.MethodGet {
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
			for _, event := range c.events(r, body, body, number, true) {
				encoded, _ := json.Marshal(event)
				if connection.Write(context.Background(), websocket.MessageText, encoded) != nil {
					return
				}
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
	var request map[string]any
	if json.Unmarshal(body, &request) != nil {
		w.WriteHeader(http.StatusBadRequest)
		return
	}
	stream := request["stream"] != false
	events := c.events(r, body, raw, 0, stream)
	if len(events) == 0 {
		w.WriteHeader(http.StatusInternalServerError)
		return
	}
	if !stream {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(events[len(events)-1]["response"])
		return
	}
	w.Header().Set("Content-Type", "text/event-stream")
	for _, event := range events {
		encoded, _ := json.Marshal(event)
		_, _ = fmt.Fprintf(w, "data: %s\n\n", encoded)
	}
}

func (c *reasoningCapture) events(r *http.Request, body, encoded []byte, connection int, stream bool) []map[string]any {
	events, _, _ := c.captureTransport(r, body, encoded, connection)
	if len(events) == 0 {
		return nil
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	wire := c.requests[len(c.requests)-1].value
	terminal := events[len(events)-1]["response"].(map[string]any)
	output := terminal["output"].([]any)
	if wire["generate"] != false && c.business > 2 {
		message := output[0].(map[string]any)
		part := message["content"].([]any)[0].(map[string]any)
		part["logprobs"] = []any{map[string]any{"token": "synthetic", "logprob": -1, "bytes": []any{115}, "top_logprobs": []any{}}}
		output = append([]any{map[string]any{"type": "reasoning", "id": fmt.Sprintf("rs_transport_%d", c.business), "summary": []any{}, "encrypted_content": fmt.Sprintf("transport-opaque-%d", c.business)}}, output...)
		terminal["output"] = output
	}
	input, _ := wire["input"].([]any)
	full := append([]any(nil), input...)
	if previous, ok := wire["previous_response_id"].(string); ok {
		base, found := c.previous[previous]
		if !found {
			c.t.Error("reasoning responder received an unknown previous reference")
			return nil
		}
		full = append(append([]any(nil), base...), input...)
	}
	if wire["generate"] != false {
		c.effective = append(c.effective, full)
	}
	c.previous[terminal["id"].(string)] = append(append([]any(nil), full...), output...)
	retained, _ := json.Marshal(c.previous[terminal["id"].(string)])
	if len(retained) > nativeCaptureLimit-c.bytes {
		c.t.Error("reasoning responder context budget exceeded")
		return nil
	}
	c.bytes += len(retained)
	// Cover ciphertext in created, added, done and populated terminal output independently.
	result := []map[string]any{{"type": "response.created", "response": map[string]any{"id": terminal["id"], "output": output}}}
	for _, event := range events {
		if event["type"] == "response.metadata" {
			result = append(result, event)
		}
	}
	for index, item := range output {
		result = append(result, map[string]any{"type": "response.output_item.added", "output_index": index, "item": item}, map[string]any{"type": "response.output_item.done", "output_index": index, "item": item})
	}
	if stream && c.emptyFooter {
		terminal["output"] = []any{}
	}
	return append(result, map[string]any{"type": "response.completed", "response": terminal})
}

func (c *reasoningCapture) contexts() [][]any {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([][]any(nil), c.effective...)
}
