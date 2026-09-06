//go:build nativeparity && scaffoldparity

package integration

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"
)

// A complete Responses event stream exercises third-party SDK parsers as well as Codex's parser.
func newScaffoldCapture(t *testing.T) *nativeCapture {
	t.Helper()
	capture := &nativeCapture{t: t}
	capture.server, capture.tap = newNativeTappedServer(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/v1/responses" {
			w.WriteHeader(404)
			return
		}
		raw, err := io.ReadAll(io.LimitReader(r.Body, nativeCaptureLimit+1))
		if err != nil || len(raw) > nativeCaptureLimit {
			w.WriteHeader(413)
			return
		}
		body := raw
		if r.Header.Get("Content-Encoding") == "zstd" {
			decoder, err := zstd.NewReader(nil, zstd.WithDecoderMaxMemory(nativeCaptureLimit))
			if err != nil {
				t.Error("scaffold decoder unavailable")
				return
			}
			defer decoder.Close()
			body, err = decoder.DecodeAll(raw, nil)
			if err != nil {
				t.Error("scaffold request decompression failed")
				return
			}
		}
		observed := capture.capture(r, body, raw, 0)
		if len(observed) == 0 {
			return
		}
		response := observed[len(observed)-1]["response"].(map[string]any)
		var request map[string]any
		if json.Unmarshal(body, &request) != nil {
			t.Error("scaffold request invalid")
			return
		}
		response["object"] = "response"
		response["created_at"] = 1720000000
		response["model"] = request["model"]
		response["status"] = "in_progress"
		response["output"] = []any{}
		w.Header().Set("Content-Type", "text/event-stream")
		sequence := 0
		send := func(event map[string]any) {
			event["sequence_number"] = sequence
			sequence++
			encoded, _ := json.Marshal(event)
			_, _ = fmt.Fprintf(w, "event: %s\ndata: %s\n\n", event["type"], encoded)
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
		}
		send(map[string]any{"type": "response.created", "response": response})
		if path := scaffoldReadPath(request); path != "" {
			arguments, _ := json.Marshal(map[string]string{"filePath": path})
			item := map[string]any{"type": "function_call", "id": "fc_scaffold_read", "call_id": "call_scaffold_read", "name": "read", "arguments": "", "status": "in_progress"}
			send(map[string]any{"type": "response.output_item.added", "output_index": 0, "item": item})
			send(map[string]any{"type": "response.function_call_arguments.delta", "output_index": 0, "item_id": item["id"], "delta": string(arguments)})
			item["arguments"] = string(arguments)
			item["status"] = "completed"
			send(map[string]any{"type": "response.function_call_arguments.done", "output_index": 0, "item_id": item["id"], "arguments": string(arguments), "name": "read"})
			send(map[string]any{"type": "response.output_item.done", "output_index": 0, "item": item})
			response["status"] = "completed"
			response["output"] = []any{item}
			response["usage"] = map[string]any{"input_tokens": 4, "output_tokens": 2, "total_tokens": 6, "input_tokens_details": map[string]any{"cached_tokens": 0}, "output_tokens_details": map[string]any{"reasoning_tokens": 0}}
			send(map[string]any{"type": "response.completed", "response": response})
			return
		}
		item := map[string]any{"type": "message", "id": "msg_" + response["id"].(string), "role": "assistant", "status": "in_progress", "content": []any{}}
		send(map[string]any{"type": "response.output_item.added", "output_index": 0, "item": item})
		part := map[string]any{"type": "output_text", "text": "", "annotations": []any{}, "logprobs": []any{}}
		send(map[string]any{"type": "response.content_part.added", "output_index": 0, "item_id": item["id"], "content_index": 0, "part": part})
		send(map[string]any{"type": "response.output_text.delta", "output_index": 0, "item_id": item["id"], "content_index": 0, "delta": "synthetic answer", "logprobs": []any{}})
		part["text"] = "synthetic answer"
		send(map[string]any{"type": "response.output_text.done", "output_index": 0, "item_id": item["id"], "content_index": 0, "text": part["text"], "logprobs": []any{}})
		send(map[string]any{"type": "response.content_part.done", "output_index": 0, "item_id": item["id"], "content_index": 0, "part": part})
		item["content"] = []any{part}
		item["status"] = "completed"
		send(map[string]any{"type": "response.output_item.done", "output_index": 0, "item": item})
		response["output"] = []any{item}
		response["status"] = "completed"
		response["usage"] = map[string]any{"input_tokens": 4, "output_tokens": 2, "total_tokens": 6, "input_tokens_details": map[string]any{"cached_tokens": 0}, "output_tokens_details": map[string]any{"reasoning_tokens": 0}}
		send(map[string]any{"type": "response.completed", "response": response})
	}))
	return capture
}

func scaffoldReadPath(request map[string]any) string {
	input, _ := request["input"].([]any)
	path := ""
	for _, raw := range input {
		item, _ := raw.(map[string]any)
		if item["type"] == "function_call_output" {
			return ""
		}
		if item["role"] != "user" {
			continue
		}
		content, _ := item["content"].([]any)
		for _, raw := range content {
			part, _ := raw.(map[string]any)
			text, _ := part["text"].(string)
			if strings.HasPrefix(text, "Read fixture: ") {
				path = strings.SplitN(strings.TrimPrefix(text, "Read fixture: "), "\n", 2)[0]
			}
		}
	}
	return path
}
