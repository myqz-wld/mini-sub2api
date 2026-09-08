//go:build nativeparity

package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func sendReasoningRequest(t *testing.T, client nativeOrdinaryClient, request map[string]any, visible bool) map[string]any {
	t.Helper()
	request["stream"] = client.stream
	if client.ws != nil {
		request["type"] = "response.create"
	}
	body := mustRequestJSON(t, request)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	items := make(map[int]any)
	consume := func(raw []byte) map[string]any {
		var event map[string]any
		if json.Unmarshal(raw, &event) != nil {
			t.Fatal("reasoning public event is invalid JSON")
		}
		assertReasoningVisibility(t, event, visible)
		if event["type"] == "error" || event["type"] == "response.failed" {
			t.Fatal("reasoning public inference failed")
		}
		return collectOrdinaryOutput(t, event, items)
	}
	if client.ws != nil {
		if client.ws.Write(ctx, websocket.MessageText, body) != nil {
			t.Fatal("reasoning public WS send failed")
		}
		for {
			kind, raw, err := client.ws.Read(ctx)
			if err != nil || kind != websocket.MessageText {
				t.Fatal("reasoning public WS read failed")
			}
			if response := consume(raw); response != nil {
				return response
			}
		}
	}
	req, _ := http.NewRequestWithContext(ctx, http.MethodPost, client.gateway.server.URL+"/v1/responses", bytes.NewReader(body))
	req.Header = client.headers.Clone()
	req.Header.Set("Content-Type", "application/json")
	response, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal("reasoning public HTTP send failed")
	}
	defer response.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(response.Body, nativeCaptureLimit+1))
	if err != nil || len(raw) > nativeCaptureLimit || response.StatusCode != 200 {
		t.Fatal("reasoning public HTTP delivery failed")
	}
	if !client.stream {
		var value map[string]any
		if json.Unmarshal(raw, &value) != nil || value["id"] == nil {
			t.Fatal("reasoning public JSON response missing")
		}
		assertReasoningVisibility(t, value, visible)
		return value
	}
	for _, line := range bytes.Split(raw, []byte("\n")) {
		if bytes.HasPrefix(line, []byte("data: ")) {
			if response := consume(line[6:]); response != nil {
				return response
			}
		}
	}
	t.Fatal("reasoning public SSE completion missing")
	return nil
}

func assertReasoningVisibility(t *testing.T, value map[string]any, visible bool) {
	t.Helper()
	check := func(item map[string]any) {
		if item["type"] != "reasoning" {
			return
		}
		_, exists := item["encrypted_content"]
		if exists != visible {
			t.Fatal("reasoning public ciphertext visibility differs from caller preference")
		}
		if item["id"] == nil || item["summary"] == nil {
			t.Fatal("reasoning public identity or summary disappeared")
		}
	}
	check(value)
	if item, ok := value["item"].(map[string]any); ok {
		check(item)
	}
	if output, ok := value["output"].([]any); ok {
		for _, item := range output {
			if object, ok := item.(map[string]any); ok {
				check(object)
			}
		}
	}
	if response, ok := value["response"].(map[string]any); ok {
		assertReasoningVisibility(t, response, visible)
	}
}
