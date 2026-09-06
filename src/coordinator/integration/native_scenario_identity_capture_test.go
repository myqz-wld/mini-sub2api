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

// Synthetic responses exercise the native built-in spawn tool. No shell or dynamic
// tool executes, and all application bytes remain inside the bounded capture.
type nativeIdentityCapture struct {
	*nativeCapture
	fork           string
	rootCalls      int
	spawnNamespace string
	spawnFound     bool
	followup       bool
}

func newNativeIdentityCapture(t *testing.T, fork string) *nativeIdentityCapture {
	c := &nativeIdentityCapture{nativeCapture: &nativeCapture{t: t}, fork: fork}
	c.server, c.tap = newNativeTappedServer(t, http.HandlerFunc(c.serveIdentity))
	return c
}

func (c *nativeIdentityCapture) serveIdentity(w http.ResponseWriter, r *http.Request) {
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
			for _, event := range c.captureIdentity(r, body, body, number) {
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
	events := c.captureIdentity(r, body, raw, 0)
	var value map[string]any
	_ = json.Unmarshal(body, &value)
	if value["stream"] == false && len(events) > 0 {
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

func (c *nativeIdentityCapture) captureIdentity(r *http.Request, body, encoded []byte, connection int) []map[string]any {
	var value map[string]any
	if json.Unmarshal(body, &value) != nil {
		c.t.Error("identity capture invalid JSON")
		return nil
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if len(c.requests) >= 128 || len(body)+len(encoded) > nativeCaptureLimit-c.bytes {
		c.t.Error("identity capture capacity exceeded")
		return nil
	}
	c.bytes += len(body) + len(encoded)
	c.requests = append(c.requests, nativeWire{method: r.Method, headers: r.Header.Clone(), body: append([]byte(nil), body...), encodedBody: append([]byte(nil), encoded...), value: value, connection: connection})
	if namespace, found := identitySpawnNamespace(value); found {
		c.spawnNamespace, c.spawnFound = namespace, true
	}
	id := fmt.Sprintf("resp_identity_%d", len(c.requests))
	output := []any{}
	if value["generate"] != false {
		c.business++
		metadata, _ := value["client_metadata"].(map[string]any)
		if metadata["session_id"] == metadata["thread_id"] {
			c.rootCalls++
		}
		if metadata["session_id"] == metadata["thread_id"] && c.rootCalls == 1 && c.fork != "disabled" {
			if !c.spawnFound {
				c.t.Error("real native spawn tool definition absent")
				return nil
			}
			args, _ := json.Marshal(map[string]any{"task_name": "identity_child", "message": "synthetic child identity task", "fork_turns": c.fork})
			item := map[string]any{"type": "function_call", "id": "fc_identity_spawn", "call_id": "call_identity_spawn", "name": "spawn_agent", "arguments": string(args)}
			if c.spawnNamespace != "" {
				item["namespace"] = c.spawnNamespace
			}
			output = append(output, item)
		} else if metadata["session_id"] == metadata["thread_id"] && c.followup && c.rootCalls == 4 {
			item := map[string]any{"type": "function_call", "id": "fc_identity_followup", "call_id": "call_identity_followup", "name": "followup_task", "arguments": `{"target":"identity_child","message":"synthetic child next task"}`}
			if c.spawnNamespace != "" {
				item["namespace"] = c.spawnNamespace
			}
			output = append(output, item)
		} else if metadata["session_id"] == metadata["thread_id"] && (c.rootCalls == 2 || c.followup && c.rootCalls == 5) && c.fork == "none" {
			item := map[string]any{"type": "function_call", "id": fmt.Sprintf("fc_identity_wait_%d", c.rootCalls), "call_id": fmt.Sprintf("call_identity_wait_%d", c.rootCalls), "name": "wait_agent", "arguments": `{"timeout_ms":3000}`}
			if c.spawnNamespace != "" {
				item["namespace"] = c.spawnNamespace
			}
			output = append(output, item)
		} else {
			output = append(output, map[string]any{"type": "message", "id": fmt.Sprintf("msg_identity_%d", c.business), "role": "assistant", "phase": "final_answer", "content": []any{map[string]any{"type": "output_text", "text": "synthetic identity complete"}}})
		}
	}
	events := []map[string]any{{"type": "response.created", "response": map[string]any{"id": id}}}
	for index, item := range output {
		events = append(events, map[string]any{"type": "response.output_item.done", "output_index": index, "item": item})
	}
	return append(events, map[string]any{"type": "response.completed", "response": map[string]any{"id": id, "output": output, "usage": map[string]any{"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}})
}

func identitySpawnNamespace(value map[string]any) (string, bool) {
	tools, _ := value["tools"].([]any)
	if tools == nil {
		input, _ := value["input"].([]any)
		if len(input) > 0 {
			if item, ok := input[0].(map[string]any); ok {
				tools, _ = item["tools"].([]any)
			}
		}
	}
	for _, raw := range tools {
		tool, _ := raw.(map[string]any)
		if tool["name"] == "spawn_agent" && tool["type"] == "function" {
			return "", true
		}
		children, _ := tool["tools"].([]any)
		for _, raw := range children {
			child, _ := raw.(map[string]any)
			if child["name"] == "spawn_agent" && child["type"] == "function" {
				namespace, _ := tool["name"].(string)
				return namespace, namespace != ""
			}
		}
	}
	return "", false
}
