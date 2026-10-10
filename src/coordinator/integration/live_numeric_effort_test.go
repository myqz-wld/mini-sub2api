//go:build nativeparity && liveparity

package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"
)

// Probe server capability against the official CLI, without requiring an optional budget to
// be enabled for this account/model. A gateway failure alone is never a passing baseline.
func TestLiveSubscription1580NumericCapability(t *testing.T) {
	options := nativeOptions{model: "gpt-6-sol", ws: false, project: liveSyntheticDirectory(t),
		base:            "Return only OK for this synthetic protocol probe. Do not use tools.",
		configOverrides: map[string]string{"model_reasoning_effort": `"8192"`}}
	// Observe the provider's response before the native error parser, which may
	// report only an incomplete stream for a rejected optional capability.
	directRelay := newLiveWireRelay(t, true)
	directResponses := observeNumericProvider(directRelay)
	options.endpoint = directRelay.endpoint
	direct := startLiveNativeDirect(t, options)
	status, rejection := numericTurnOutcome(t, direct, direct.thread(options))
	provider := waitNumericProvider(t, directResponses)
	provider.report(t)
	rejection = rejection || provider.confirmedEffortRejection()
	packets := directRelay.tap.packets(t)
	if len(packets) != 1 {
		t.Fatal("direct numeric probe exceeded its one-request bound")
	}
	assertNumericPacket(t, packets[0])
	if status != "completed" && !rejection {
		t.Fatal("native numeric capability failed without a confirmed reasoning-effort rejection")
	}
	gateway, relay := newLiveCallerWireGateway(t, 2)
	options.endpoint, options.bearer = gateway.server.URL, gateway.secret
	mediated := startNativeClient(t, options)
	actual, _ := numericTurnOutcome(t, mediated, mediated.thread(options))
	if actual != status {
		t.Fatal("gateway numeric capability differs from the official native baseline")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
	defer cancel()
	body := mustRequestJSON(t, map[string]any{"model": options.model, "input": "Return OK.", "stream": false, "reasoning": map[string]any{"effort": 8192}})
	request, _ := http.NewRequestWithContext(ctx, http.MethodPost, gateway.server.URL+"/v1/responses", bytes.NewReader(body))
	request.Header.Set("Authorization", "Bearer "+gateway.secret)
	request.Header.Set("Content-Type", "application/json")
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal("numeric raw API probe transport failed")
	}
	_, _ = io.Copy(io.Discard, io.LimitReader(response.Body, 64*1024))
	_ = response.Body.Close()
	want := http.StatusOK
	if rejection {
		switch provider.status {
		case http.StatusBadRequest:
			want = http.StatusBadRequest
		case http.StatusOK:
			// A streamed error without a completed response cannot become the
			// public caller's requested non-stream JSON success.
			want = http.StatusBadGateway
		default:
			t.Fatal("numeric type rejection used an unverified transport status")
		}
	}
	if response.StatusCode != want {
		t.Fatalf("numeric raw API status differs from native capability: status=%d", response.StatusCode)
	}
	packets = relay.tap.packets(t)
	if len(packets) != 2 {
		t.Fatal("numeric capability unexpectedly replayed a business request")
	}
	for _, packet := range packets {
		var wire map[string]any
		if json.Unmarshal(nativePacketJSON(t, packet), &wire) != nil {
			t.Fatal("numeric capability egress JSON")
		}
		reasoning, _ := wire["reasoning"].(map[string]any)
		if reasoning["effort"] != float64(8192) {
			t.Fatal("numeric capability did not emit the native JSON number")
		}
	}
	assertLiveCallerWire(t, gateway, relay)
	t.Logf("LIVE_NUMERIC_CAPABILITY native_completed=%t native_reasoning_effort_rejected=%t provider_http_status=%d raw_http_status=%d gateway_same_outcome=true numeric_wire=true", status == "completed", rejection, provider.status, response.StatusCode)
}

func numericTurnOutcome(t *testing.T, client *nativeClient, thread string) (string, bool) {
	t.Helper()
	previousObserver := client.observe
	var notifiedError any
	client.observe = func(event map[string]any) {
		if previousObserver != nil {
			previousObserver(event)
		}
		if event["method"] == "error" {
			params, _ := event["params"].(map[string]any)
			notifiedError = params["error"]
		}
	}
	defer func() { client.observe = previousObserver }()
	client.call("turn/start", map[string]any{"threadId": thread, "input": []any{map[string]any{"type": "text", "text": "Return OK."}}})
	for {
		if len(client.pending) == 0 {
			client.handle(client.read())
			continue
		}
		event := client.pending[0]
		client.pending = client.pending[1:]
		params, _ := event["params"].(map[string]any)
		turn, _ := params["turn"].(map[string]any)
		status, _ := turn["status"].(string)
		// Inspect only in memory. Retain booleans, never the provider's error message or body.
		raw, _ := json.Marshal([]any{turn["error"], notifiedError})
		message := strings.ToLower(string(raw))
		if status != "completed" {
			var terms []string
			for _, term := range []string{"400", "401", "403", "429", "500", "502", "503", "504", "reasoning", "effort", "8192", "integer", "string", "invalid", "unsupported", "model", "authentication", "limit", "range", "code_mode", "code mode", "tools", "budget", "connect", "stream", "timeout", "timed", "unexpected", "error", "retry", "credit"} {
				if strings.Contains(message, term) {
					terms = append(terms, term)
				}
			}
			t.Logf("NUMERIC_ERROR_FACTS status=%s terminal_error_present=%t notified_error_present=%t matched_static_terms=%s", status, turn["error"] != nil, notifiedError != nil, strings.Join(terms, ","))
		}
		rejected := status == "failed" && strings.Contains(message, "reasoning") && strings.Contains(message, "effort") && strings.Contains(message, "integer") && strings.Contains(message, "invalid")
		return status, rejected
	}
}
