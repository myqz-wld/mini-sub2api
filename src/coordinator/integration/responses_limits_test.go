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
	for _, subscription := range []bool{false, true} {
		name := "WS"
		if subscription {
			name = "SubscriptionWS"
		}
		t.Run(name, func(t *testing.T) {
			fixture := newResponsesProfileWebSocketFixture(t)
			key := fixture.apiKey
			if subscription {
				key = fixture.subscriptionKey
			}
			connection := dialResponsesProfileWebSocket(t, fixture.public, key, http.Header{"Originator": {"codex_exec"}})
			defer connection.CloseNow()
			body := mustRequestJSON(t, map[string]any{"type": "response.create", "model": "gpt-5.4", "input": large})
			ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
			defer cancel()
			if err := connection.Write(ctx, websocket.MessageText, body); err != nil {
				t.Fatal(err)
			}
			// This is a size-admission contract, not a two-second throughput requirement.
			// Race instrumentation can spend longer than the small-frame fixture deadline.
			var captured responsesProfileWebSocketCapture
			select {
			case captured = <-fixture.captures:
			case <-ctx.Done():
				t.Fatal("large WS capture deadline")
			}
			if !subscription && !bytes.Equal(captured.Frame, body) {
				t.Fatal("large API-key WS frame changed")
			}
			if subscription {
				value := decodeRequestObject(t, captured.Frame)
				items, _ := value["input"].([]any)
				if len(items) != 1 {
					t.Fatal("large Subscription WS input shape")
				}
				content, _ := items[0].(map[string]any)["content"].([]any)
				if len(content) != 1 || content[0].(map[string]any)["text"] != large {
					t.Fatal("large Subscription WS input changed")
				}
			}
			readResponsesProfileTerminalEvents(t, connection)
		})
	}
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
