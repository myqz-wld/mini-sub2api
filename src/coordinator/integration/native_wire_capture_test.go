//go:build nativeparity

package integration

import (
	"context"
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/coder/websocket"
	"github.com/klauspost/compress/zstd"
)

type nativeWire struct {
	method      string
	headers     http.Header
	body        []byte
	encodedBody []byte
	value       map[string]any
	connection  int
}

type nativeCapture struct {
	t                  *testing.T
	server             *httptest.Server
	tap                *nativeTap
	mu                 sync.Mutex
	requests           []nativeWire
	connections        int
	business           int
	bytes              int
	metadataOnlyFooter bool
}

func newNativeCapture(t *testing.T) *nativeCapture {
	return newNativeCaptureWithFooter(t, false)
}

func newNativeCaptureWithFooter(t *testing.T, metadataOnly bool) *nativeCapture {
	t.Helper()
	capture := &nativeCapture{t: t, metadataOnlyFooter: metadataOnly}
	capture.server, capture.tap = newNativeTappedServer(t, http.HandlerFunc(capture.serve))
	assertLoopbackURL(t, capture.server.URL)
	return capture
}
func (c *nativeCapture) serve(w http.ResponseWriter, r *http.Request) {
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
		connection, err := websocket.Accept(w, r, &websocket.AcceptOptions{CompressionMode: websocket.CompressionContextTakeover})
		if err != nil {
			return
		}
		defer connection.CloseNow()
		connection.SetReadLimit(128 * 1024 * 1024)
		c.mu.Lock()
		c.connections++
		number := c.connections
		c.mu.Unlock()
		for {
			kind, body, err := connection.Read(context.Background())
			if err != nil {
				return
			}
			if kind != websocket.MessageText {
				return
			}
			events := c.capture(r, body, body, number)
			for _, event := range events {
				body, _ := json.Marshal(event)
				if connection.Write(context.Background(), websocket.MessageText, body) != nil {
					return
				}
			}
		}
	}
	if r.Method != http.MethodPost {
		w.WriteHeader(http.StatusMethodNotAllowed)
		return
	}
	raw, err := io.ReadAll(io.LimitReader(r.Body, 128*1024*1024+1))
	if err != nil || len(raw) > 128*1024*1024 {
		w.WriteHeader(http.StatusRequestEntityTooLarge)
		return
	}
	body := raw
	if r.Header.Get("Content-Encoding") == "zstd" {
		decoder, err := zstd.NewReader(nil, zstd.WithDecoderMaxMemory(128*1024*1024), zstd.WithDecoderConcurrency(1))
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
	events := c.capture(r, body, raw, 0)
	var request map[string]any
	_ = json.Unmarshal(body, &request)
	if request["stream"] == false && len(events) > 0 {
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
func (c *nativeCapture) capture(r *http.Request, body, encoded []byte, connection int) []map[string]any {
	var value map[string]any
	if json.Unmarshal(body, &value) != nil {
		c.t.Error("native capture received invalid JSON")
		return nil
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if len(c.requests) >= 128 || len(body)+len(encoded) > nativeCaptureLimit-c.bytes {
		c.t.Error("native application capture capacity exceeded")
		return nil
	}
	c.bytes += len(body) + len(encoded)
	c.requests = append(c.requests, nativeWire{encodedBody: append([]byte(nil), encoded...), method: r.Method, headers: r.Header.Clone(), body: append([]byte(nil), body...), value: value, connection: connection})
	id := fmt.Sprintf("resp_native_%d", len(c.requests))
	prewarm := value["generate"] == false
	output := []any{}
	if !prewarm {
		c.business++
		if c.business == 1 {
			output = append(output, map[string]any{"type": "function_call", "id": "fc_native_probe", "call_id": "call_native_probe", "name": "native_probe", "arguments": "{}"})
		} else {
			output = append(output, map[string]any{"type": "message", "id": fmt.Sprintf("msg_native_%d", c.business), "role": "assistant", "phase": "final_answer", "content": []any{map[string]any{"type": "output_text", "text": "synthetic answer"}}})
		}
	}
	events := []map[string]any{{"type": "response.created", "response": map[string]any{"id": id}}, {"type": "response.metadata", "headers": map[string]any{"x-codex-turn-state": "native-routing-token"}}}
	for index, item := range output {
		events = append(events, map[string]any{"type": "response.output_item.done", "output_index": index, "item": item})
	}
	footerOutput := output
	if c.metadataOnlyFooter {
		footerOutput = []any{}
	}
	return append(events, map[string]any{"type": "response.completed", "response": map[string]any{"id": id, "output": footerOutput, "usage": map[string]any{"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}})
}
func (c *nativeCapture) snapshot() []nativeWire {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]nativeWire(nil), c.requests...)
}
func businessWires(wires []nativeWire) []nativeWire {
	var out []nativeWire
	for _, wire := range wires {
		if wire.value["generate"] != false {
			out = append(out, wire)
		}
	}
	return out
}

func uuid5(namespace []byte, payload []byte) []byte {
	hash := sha1.New()
	_, _ = hash.Write(namespace)
	_, _ = hash.Write(payload)
	id := hash.Sum(nil)[:16]
	id[6] = (id[6] & 15) | 80
	id[8] = (id[8] & 63) | 128
	return id
}
func uuidText(id []byte) string {
	s := hex.EncodeToString(id)
	return s[:8] + "-" + s[8:12] + "-" + s[12:16] + "-" + s[16:20] + "-" + s[20:]
}
func assertNativeLiteIDs(t *testing.T, wire nativeWire) {
	t.Helper()
	var raw struct {
		Input []json.RawMessage `json:"input"`
	}
	if json.Unmarshal(wire.body, &raw) != nil || len(raw.Input) < 2 {
		t.Fatal("Lite prefix missing")
	}
	metadata, _ := wire.value["client_metadata"].(map[string]any)
	thread, _ := metadata["thread_id"].(string)
	if thread == "" {
		t.Fatal("Lite thread identity missing")
	}
	oid, _ := hex.DecodeString("6ba7b8129dad11d180b400c04fd430c8")
	namespace := uuid5(oid, []byte(thread))
	var tools struct {
		Type  string          `json:"type"`
		ID    string          `json:"id"`
		Tools json.RawMessage `json:"tools"`
	}
	_ = json.Unmarshal(raw.Input[0], &tools)
	if tools.Type != "additional_tools" || tools.ID != "at_"+uuidText(uuid5(namespace, tools.Tools)) {
		t.Fatal("native Lite tools deterministic ID mismatch")
	}
	var base struct {
		ID      string `json:"id"`
		Content []struct {
			Text string `json:"text"`
		} `json:"content"`
	}
	_ = json.Unmarshal(raw.Input[1], &base)
	if len(base.Content) != 1 || base.ID != "msg_"+uuidText(uuid5(namespace, []byte(base.Content[0].Text))) {
		t.Fatal("native Lite base deterministic ID mismatch")
	}
}

func TestNativeCodexCaptureHTTPAndWS(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, model), func(t *testing.T) {
				capture := newNativeCapture(t)
				options := nativeOptions{endpoint: capture.server.URL, model: model, ws: ws, base: "  native {{literal}} base  "}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				client.turn(thread, "first synthetic turn")
				client.turn(thread, "second synthetic turn")
				wires := businessWires(capture.snapshot())
				if len(wires) != 3 {
					t.Fatalf("native request count = %d, want tool loop plus next turn", len(wires))
				}
				for _, wire := range wires {
					expected := http.MethodPost
					if ws {
						expected = http.MethodGet
					}
					if wire.method != expected {
						t.Fatal("native selected unexpected transport")
					}
					if wire.value["model"] != model {
						t.Fatal("native model selection changed")
					}
					if !strings.Contains(wire.headers.Get("User-Agent"), "0.153.4") {
						t.Fatal("native UA version mismatch")
					}
				}
				if ws {
					if wires[1].value["previous_response_id"] == nil {
						t.Fatal("native tool continuation did not use previous response")
					}
				} else {
					for _, wire := range wires {
						if wire.value["previous_response_id"] != nil {
							t.Fatal("native HTTP was incremental")
						}
						if wire.headers.Get("Content-Encoding") != "zstd" {
							t.Fatal("native Subscription HTTP did not use zstd")
						}
					}
				}
				if strings.Contains(model, "5.6") {
					prefixes := 0
					for _, wire := range capture.snapshot() {
						input, _ := wire.value["input"].([]any)
						if len(input) > 0 {
							item, _ := input[0].(map[string]any)
							if item["type"] == "additional_tools" {
								assertNativeLiteIDs(t, wire)
								prefixes++
								continue
							}
						}
						if wire.value["previous_response_id"] == nil {
							t.Fatal("native Lite omitted setup without a baseline")
						}
					}
					if prefixes == 0 {
						t.Fatal("native Lite prefix was never observed")
					}
				}
				t.Logf("native captured %d business requests; transport_ws=%t; model=%s", len(wires), ws, model)
			})
		}
	}
}
