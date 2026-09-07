package integration

import (
	"bytes"
	"fmt"
	"net/http"
	"testing"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

const routingHintHeader = "X-Codex-Routing-Hint"

type routingHintCase struct {
	subscription bool
	codex        bool
	model        string
	tier         string
	present      bool
	value        string
}

func TestRoutingHintHTTPPolicy(t *testing.T) {
	fixture := newResponsesProfileHTTPFixture(t)
	runRoutingHintCases(t, func(t *testing.T, test routingHintCase) {
		key := fixture.apiKey
		if test.subscription {
			key = fixture.subscriptionKey
		}
		body := test.body(t, false)
		status, _, _ := publicRequestWithHeaders(t, fixture.public, key, string(body), test.headers())
		if status != http.StatusOK {
			t.Fatalf("routing-hint HTTP request failed: status %d", status)
		}
		capture := waitForRoutingCapture(t, fixture.captures)
		assertRoutingHintHeaders(t, capture.Headers, test)
		assertHTTPProfileCredentialBoundary(t, capture, test.subscription)
		if !test.subscription && !bytes.Equal(capture.Body, body) {
			t.Fatal("routing-hint forwarding changed the API-key request body")
		}
	})
}

func TestRoutingHintWebSocketPolicy(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixture(t)
	runRoutingHintCases(t, func(t *testing.T, test routingHintCase) {
		key := fixture.apiKey
		if test.subscription {
			key = fixture.subscriptionKey
		}
		connection := dialResponsesProfileWebSocket(t, fixture.public, key, test.headers())
		defer connection.CloseNow()
		frame := test.body(t, true)
		writeE2EWebSocketText(t, connection, string(frame))
		readResponsesProfileTerminalEvents(t, connection)
		hidden := test.subscription && !test.codex
		captures := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1+boolToInt(hidden))
		for _, capture := range captures {
			assertRoutingHintHeaders(t, capture.Headers, test)
			assertWebSocketProfileCredentialBoundary(t, capture, test.subscription)
		}
		if !test.subscription && !bytes.Equal(captures[0].Frame, frame) {
			t.Fatal("routing-hint forwarding changed the API-key WS frame")
		}
	})
}

func runRoutingHintCases(t *testing.T, run func(*testing.T, routingHintCase)) {
	t.Helper()
	for _, subscription := range []bool{false, true} {
		for _, codex := range []bool{false, true} {
			for _, format := range []struct{ name, model, tier string }{
				{"ordinary", "gpt-5.4", ""},
				{"lite", "gpt-5.6-sol", "priority"},
			} {
				for _, hint := range []struct {
					name, value string
					present     bool
				}{
					{name: "absent"},
					{name: "empty", present: true},
					{name: "custom", value: "model=caller-only; tier=opaque;extra=a%2Cb", present: true},
				} {
					name := fmt.Sprintf("subscription=%t/codex=%t/%s/%s", subscription, codex, format.name, hint.name)
					t.Run(name, func(t *testing.T) {
						run(t, routingHintCase{subscription, codex, format.model, format.tier, hint.present, hint.value})
					})
				}
			}
		}
	}
}

func (test routingHintCase) headers() http.Header {
	headers := http.Header{
		"Cookie":                        {"synthetic=must-not-cross"},
		"X-Forwarded-For":               {"192.0.2.1"},
		"X-Codex-Routing-Hint-Extra":    {"must-not-cross"},
		protocolv1.AccountRefHeader:     {"untrusted-account"},
		protocolv1.PseudonymScopeHeader: {"untrusted-scope"},
	}
	if test.codex {
		headers.Set("Originator", "codex_exec")
	}
	if test.present {
		headers.Set(routingHintHeader, test.value)
	}
	return headers
}

func (test routingHintCase) body(t *testing.T, ws bool) []byte {
	t.Helper()
	request := map[string]any{
		"model": test.model, "input": "synthetic routing-hint check",
		"instructions": "Return a short answer.", "tools": []any{}, "stream": ws,
	}
	if test.tier != "" {
		request["service_tier"] = test.tier
	}
	if ws {
		request["type"] = "response.create"
	}
	return append(append([]byte(" \n"), mustRequestJSON(t, request)...), '\n')
}

func assertRoutingHintHeaders(t *testing.T, headers http.Header, test routingHintCase) {
	t.Helper()
	values := headers.Values(routingHintHeader)
	if test.subscription {
		want := "model=" + test.model
		if test.tier != "" {
			want += ";tier=" + test.tier
		}
		if len(values) != 1 || values[0] != want {
			t.Fatal("Subscription routing hint was not derived from the actual request")
		}
	} else if test.present {
		if len(values) != 1 || values[0] != test.value {
			t.Fatal("caller API-key routing hint was removed or rewritten")
		}
	} else if len(values) != 0 {
		t.Fatal("API-key routing hint was synthesized for a headerless caller")
	}
	for _, name := range []string{
		"Cookie", "X-Forwarded-For", "X-Codex-Routing-Hint-Extra",
		protocolv1.AccountRefHeader, protocolv1.PseudonymScopeHeader,
		protocolv1.RequestIDHeader, protocolv1.VersionHeader,
	} {
		if len(headers.Values(name)) != 0 {
			t.Fatalf("routing-hint forwarding exposed a private or unapproved header: %s", name)
		}
	}
}
