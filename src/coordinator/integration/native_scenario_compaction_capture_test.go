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

	"github.com/coder/websocket"
	"github.com/klauspost/compress/zstd"
)

// Captures remain in memory. A configured responder controls only synthetic compaction output.
type nativeCompactionCapture struct {
	*nativeCapture
	mode            string
	fail            bool
	mu              sync.Mutex
	compactRequests int
	outputs         map[string][]any
}

func newNativeCompactionCapture(t *testing.T, mode string, fail bool) *nativeCompactionCapture {
	c := &nativeCompactionCapture{nativeCapture: &nativeCapture{t: t}, mode: mode, fail: fail, outputs: map[string][]any{}}
	c.server, c.tap = newNativeTappedServer(t, http.HandlerFunc(c.serveCompaction))
	return c
}

func (c *nativeCompactionCapture) serveCompaction(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path == "/backend-api/wham/settings/user" {
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"commit_attribution_enabled":false}`)
		return
	}
	if r.URL.Path != "/v1/responses" && r.URL.Path != "/v1/responses/compact" {
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
		c.nativeCapture.mu.Lock()
		c.connections++
		number := c.connections
		c.nativeCapture.mu.Unlock()
		for {
			kind, body, err := connection.Read(context.Background())
			if err != nil || kind != websocket.MessageText {
				return
			}
			events, _ := c.recordCompaction(r, body, body, number)
			for _, event := range events {
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
		w.WriteHeader(413)
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
		if err != nil {
			w.WriteHeader(400)
			return
		}
	}
	events, unary := c.recordCompaction(r, body, raw, 0)
	if unary != nil {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(unary)
		return
	}
	var request map[string]any
	_ = json.Unmarshal(body, &request)
	if request["stream"] == false {
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

func (c *nativeCompactionCapture) recordCompaction(r *http.Request, body, raw []byte, connection int) ([]map[string]any, map[string]any) {
	c.nativeCapture.capture(r, body, raw, connection)
	var value map[string]any
	if json.Unmarshal(body, &value) != nil {
		return nil, nil
	}
	items, _ := value["input"].([]any)
	isCompact := r.URL.Path == "/v1/responses/compact"
	for _, item := range items {
		m, _ := item.(map[string]any)
		isCompact = isCompact || m["type"] == "compaction_trigger"
	}
	metadata, _ := value["client_metadata"].(map[string]any)
	var nested map[string]any
	if text, ok := metadata["x-codex-turn-metadata"].(string); ok {
		_ = json.Unmarshal([]byte(text), &nested)
	}
	isCompact = isCompact || nested["request_kind"] == "compaction"
	prewarm := value["generate"] == false
	c.mu.Lock()
	fail := c.fail
	inBand := c.mode == "in-band" && c.compactRequests == 0 && !prewarm
	if (isCompact || inBand) && !prewarm {
		c.compactRequests++
	}
	c.mu.Unlock()
	n := len(c.snapshot())
	id := fmt.Sprintf("resp_compaction_%d", n)
	output := []any{}
	if !prewarm {
		if inBand {
			output = append(output, map[string]any{"type": "compaction", "id": "cmp_inline_fixture", "encrypted_content": "synthetic_inline_state"})
		}
		if isCompact && c.mode != "local" && !fail {
			output = append(output, map[string]any{"type": "compaction", "id": "cmp_compaction_fixture", "encrypted_content": "synthetic_compacted_state"})
		} else {
			text := "synthetic precompaction assistant"
			if isCompact {
				text = "synthetic compact summary"
			}
			output = append(output, map[string]any{"type": "message", "id": fmt.Sprintf("msg_compaction_%d", n), "role": "assistant", "phase": "final_answer", "content": []any{map[string]any{"type": "output_text", "text": text}}})
		}
	}
	if r.URL.Path == "/v1/responses/compact" {
		if !fail {
			output = append([]any{
				map[string]any{"type": "message", "id": "msg_v1_stale", "role": "developer", "content": []any{map[string]any{"type": "input_text", "text": "synthetic stale server developer"}}},
				map[string]any{"type": "message", "id": "msg_v1_retained", "role": "user", "content": []any{map[string]any{"type": "input_text", "text": "synthetic server retained user"}}},
				map[string]any{"type": "function_call", "id": "fc_v1_discarded", "call_id": "call_v1_discarded", "name": "native_probe", "arguments": "{}"},
			}, output...)
		}
		c.mu.Lock()
		c.outputs[id] = output
		c.mu.Unlock()
		return nil, map[string]any{"id": id, "object": "response.compaction", "output": output}
	}
	c.mu.Lock()
	c.outputs[id] = output
	c.mu.Unlock()
	events := []map[string]any{{"type": "response.created", "response": map[string]any{"id": id}}}
	for i, item := range output {
		events = append(events, map[string]any{"type": "response.output_item.done", "output_index": i, "item": item})
	}
	return append(events, map[string]any{"type": "response.completed", "response": map[string]any{"id": id, "output": output, "usage": map[string]any{"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}}), nil
}

// Expand only references actually returned by this mock. This is the observed upstream
// conversation, not an assumption that every captured WS frame is a complete prompt.
func (c *nativeCompactionCapture) effectiveWires(t *testing.T) []nativeWire {
	t.Helper()
	c.mu.Lock()
	defer c.mu.Unlock()
	contexts := map[string][]any{}
	connections := map[string]int{}
	wires := c.snapshot()
	for i := range wires {
		wire := &wires[i]
		input, _ := wire.value["input"].([]any)
		if previous, ok := wire.value["previous_response_id"].(string); ok {
			prior, found := contexts[previous]
			if !found || connections[previous] != wire.connection {
				t.Fatal("compaction reused an unavailable socket baseline")
			}
			input = append(append([]any(nil), prior...), input...)
		}
		value := map[string]any{}
		for k, v := range wire.value {
			value[k] = v
		}
		value["input"] = input
		wire.value = value
		id := fmt.Sprintf("resp_compaction_%d", i+1)
		contexts[id] = append(append([]any(nil), input...), c.outputs[id]...)
		connections[id] = wire.connection
	}
	return wires
}

func runNativeManualCompaction(t *testing.T, client *nativeClient, thread string) (bool, bool) {
	t.Helper()
	client.id++
	id := client.id
	client.send(map[string]any{"id": id, "method": "thread/compact/start", "params": map[string]any{"threadId": thread}})
	ack, complete, accepted, failed := false, false, false, false
	for !ack || !complete {
		event := client.read()
		if event["id"] == float64(id) && event["method"] == nil {
			if event["error"] != nil {
				t.Fatal("native compaction RPC rejected")
			}
			ack = true
		}
		params, _ := event["params"].(map[string]any)
		item, _ := params["item"].(map[string]any)
		if event["method"] == "item/completed" && item["type"] == "contextCompaction" {
			accepted = true
		}
		if event["method"] == "error" {
			failed = true
		}
		if event["method"] == "turn/completed" {
			complete = true
		}
	}
	return accepted, failed
}

func compactionMetadata(t *testing.T, wire nativeWire) map[string]any {
	t.Helper()
	metadata, _ := wire.value["client_metadata"].(map[string]any)
	encoded, _ := metadata["x-codex-turn-metadata"].(string)
	if encoded == "" {
		encoded = wire.headers.Get("X-Codex-Turn-Metadata")
	}
	var nested map[string]any
	if json.Unmarshal([]byte(encoded), &nested) != nil {
		t.Fatal("compaction nested metadata missing")
	}
	return nested
}
