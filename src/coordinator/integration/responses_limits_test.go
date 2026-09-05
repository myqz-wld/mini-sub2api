package integration

import (
	"bytes"
	"context"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestResponsesAcceptsActualFramesAboveFormer16MiBLimit(t *testing.T) {
	large := strings.Repeat("a", 17*1024*1024)
	t.Run("HTTP", func(t *testing.T) {
		fixture := newResponsesProfileHTTPFixture(t)
		body := mustRequestJSON(t, map[string]any{"model": "gpt-5.4", "instructions": "base", "input": large, "stream": false})
		status, _, _ := publicRequestWithHeaders(t, fixture.public, fixture.apiKey, string(body), http.Header{"Originator": {"codex_exec"}})
		if status != http.StatusOK {
			t.Fatalf("large HTTP status = %d", status)
		}
		capture := waitForRoutingCapture(t, fixture.captures)
		if !bytes.Equal(capture.Body, body) {
			t.Fatal("large API-key HTTP request changed")
		}
	})
	t.Run("SubscriptionHTTP", func(t *testing.T) {
		fixture := newResponsesProfileHTTPFixture(t)
		body := mustRequestJSON(t, map[string]any{"model": "gpt-5.4", "instructions": "base", "input": large, "stream": false})
		status, _, _ := publicRequestWithHeaders(t, fixture.public, fixture.subscriptionKey, string(body), nil)
		if status != http.StatusOK {
			t.Fatalf("large Subscription HTTP status = %d", status)
		}
		capture := waitForRoutingCapture(t, fixture.captures)
		value := decodeRequestObject(t, capture.Body)
		items, _ := value["input"].([]any)
		if len(items) != 1 {
			t.Fatal("large Subscription input structure changed")
		}
		content, _ := items[0].(map[string]any)["content"].([]any)
		if content[0].(map[string]any)["text"] != large {
			t.Fatal("large Subscription input changed")
		}
	})
	t.Run("WS", func(t *testing.T) {
		fixture := newResponsesProfileWebSocketFixture(t)
		connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.apiKey, http.Header{"Originator": {"codex_exec"}})
		defer connection.CloseNow()
		body := mustRequestJSON(t, map[string]any{"type": "response.create", "model": "gpt-5.4", "input": large})
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		if err := connection.Write(ctx, websocket.MessageText, body); err != nil {
			t.Fatal(err)
		}
		captures := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)
		if !bytes.Equal(captures[0].Frame, body) {
			t.Fatal("large API-key WS frame changed")
		}
		readResponsesProfileTerminalEvents(t, connection)
	})
}

func TestOperatorRequestLimitAppliesToHTTPAndWebSocketAdmission(t *testing.T) {
	t.Setenv("MINI_SUB2API_LIMITS", `{"requestBytes":4096}`)
	t.Run("HTTP", func(t *testing.T) {
		fixture := newResponsesProfileHTTPFixture(t)
		body := mustRequestJSON(t, map[string]any{"model": "gpt-5.4", "input": strings.Repeat("a", 5000)})
		status, _, _ := publicRequestWithHeaders(t, fixture.public, fixture.apiKey, string(body), nil)
		if status != http.StatusRequestEntityTooLarge {
			t.Fatalf("HTTP admission status = %d", status)
		}
		select {
		case <-fixture.captures:
			t.Fatal("oversized HTTP reached upstream")
		default:
		}
	})
	t.Run("WS", func(t *testing.T) {
		fixture := newResponsesProfileWebSocketFixture(t)
		connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.apiKey, nil)
		defer connection.CloseNow()
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = connection.Write(ctx, websocket.MessageText, mustRequestJSON(t, map[string]any{
			"type": "response.create", "model": "gpt-5.4", "input": strings.Repeat("a", 5000),
		}))
		if _, _, err := connection.Read(ctx); err == nil {
			t.Fatal("oversized WS was admitted")
		}
		select {
		case <-fixture.captures:
			t.Fatal("oversized WS reached upstream")
		default:
		}
	})
}
