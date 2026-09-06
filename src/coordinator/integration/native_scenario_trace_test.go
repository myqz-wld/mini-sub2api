//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"reflect"
	"sort"
	"strings"
	"sync/atomic"
	"testing"
)

// The public JSON-RPC trace envelope controls OTEL parentage. This does not
// enable CODEX_ROLLOUT_TRACE_ROOT, which would persist inference payloads.
func nativeScenarioTraceTurn(c *nativeClient, thread string, metadata, trace map[string]string) {
	c.t.Helper()
	params := map[string]any{
		"threadId": thread,
		"input":    []any{map[string]any{"type": "text", "text": "synthetic trace metadata turn"}},
	}
	if metadata != nil {
		params["responsesapiClientMetadata"] = metadata
	}
	c.id++
	id := c.id
	envelope := map[string]any{"id": id, "method": "turn/start", "params": params}
	if trace != nil {
		envelope["trace"] = trace
	}
	c.send(envelope)
	for {
		event := c.read()
		if event["method"] == nil && event["id"] == float64(id) {
			if _, failed := event["error"]; failed {
				c.t.Fatal("native traced turn rejected")
			}
			break
		}
		c.handle(event)
	}
	for {
		if len(c.pending) > 0 {
			event := c.pending[0]
			c.pending = c.pending[1:]
			params, _ := event["params"].(map[string]any)
			turn, _ := params["turn"].(map[string]any)
			if turn["status"] != "completed" {
				c.t.Fatal("native traced turn did not complete")
			}
			return
		}
		c.handle(c.read())
	}
}

func nativeScenarioTraceCollector(t *testing.T) string {
	t.Helper()
	var requests atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// OTEL data is discarded, never decoded, tapped, logged or persisted.
		// Bound both request count and consumption; the collector never forwards.
		if requests.Add(1) > 32 {
			t.Error("native trace collector request bound exceeded")
			w.WriteHeader(http.StatusTooManyRequests)
			return
		}
		read, err := io.Copy(io.Discard, io.LimitReader(r.Body, 1024*1024+1))
		if read > 1024*1024 {
			// Large exporter batches are deliberately rejected after bounded
			// consumption. Export completeness is not a Responses wire oracle.
			w.WriteHeader(http.StatusRequestEntityTooLarge)
			return
		}
		if err != nil {
			// The isolated native process can terminate while its best-effort
			// exporter is writing. This is not a capture or size-bound failure.
			w.WriteHeader(http.StatusBadRequest)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, "{}")
	}))
	t.Cleanup(server.Close)
	assertLoopbackURL(t, server.URL)
	return server.URL + "/v1/traces"
}

func nativeScenarioTraceParent(turn int) map[string]string {
	return map[string]string{
		"traceparent": fmt.Sprintf("00-%032x-%016x-01", turn+1, turn+101),
		"tracestate":  fmt.Sprintf("scenario=turn%d", turn+1),
	}
}

func nativeScenarioTraceParts(t *testing.T, value string) []string {
	t.Helper()
	parts := strings.Split(value, "-")
	if len(parts) != 4 || parts[0] != "00" || len(parts[1]) != 32 || len(parts[2]) != 16 || len(parts[3]) != 2 {
		t.Fatal("native traceparent shape is not W3C version zero")
	}
	for _, part := range parts[1:] {
		if _, err := hex.DecodeString(part); err != nil {
			t.Fatal("native traceparent contains non-hex data")
		}
	}
	if strings.Trim(parts[1], "0") == "" || strings.Trim(parts[2], "0") == "" {
		t.Fatal("native traceparent has a zero trace or span ID")
	}
	return parts
}

func nativeScenarioTraceMetadata(t *testing.T, value map[string]any) (map[string]any, map[string]any) {
	t.Helper()
	flat, ok := value["client_metadata"].(map[string]any)
	if !ok {
		t.Fatal("native trace scenario client metadata missing")
	}
	encoded, ok := flat["x-codex-turn-metadata"].(string)
	var nested map[string]any
	if !ok || json.Unmarshal([]byte(encoded), &nested) != nil {
		t.Fatal("native trace scenario nested metadata missing")
	}
	return flat, nested
}

func nativeScenarioTraceExtra(turn int) map[string]string {
	if turn == 2 {
		return nil
	}
	return map[string]string{
		"scenario_trace_label":   fmt.Sprintf("turn%d", turn+1),
		"scenario_trace_unicode": "synthetic \u00e9 \U0001f680",
		"session_id":             "reserved-native-sentinel",
		"thread_id":              "reserved-native-sentinel",
		"turn_id":                "reserved-native-sentinel",
		"window_number":          "reserved-native-sentinel",
		"workspaces":             "reserved-native-sentinel",
	}
}

func assertNativeScenarioTraceExtra(t *testing.T, value map[string]any, turn int, allowRemoval bool) []string {
	t.Helper()
	flat, nested := nativeScenarioTraceMetadata(t, value)
	expected := nativeScenarioTraceExtra(turn)
	var differences []string
	for _, field := range []string{"scenario_trace_label", "scenario_trace_unicode"} {
		if _, present := flat[field]; present {
			t.Fatal("native extra metadata leaked into flat client metadata")
		}
		if turn == 2 {
			if _, present := nested[field]; present {
				t.Fatal("native extra metadata persisted into a new turn without metadata")
			}
		} else if nested[field] == nil && allowRemoval {
			differences = append(differences, "removed:client_metadata.x-codex-turn-metadata."+field)
		} else if nested[field] != expected[field] {
			t.Fatalf("native custom metadata changed: %s", field)
		}
	}
	for _, field := range []string{"session_id", "thread_id", "turn_id"} {
		if nested[field] != flat[field] || nested[field] == "reserved-native-sentinel" {
			t.Fatalf("native reserved metadata override affected identity: %s", field)
		}
	}
	if nested["window_number"] == "reserved-native-sentinel" || nested["workspaces"] == "reserved-native-sentinel" {
		t.Fatal("native reserved metadata override affected context state")
	}
	return differences
}

func assertNativeScenarioTraceHeaderExtra(t *testing.T, headers http.Header, turn int, allowRemoval bool) []string {
	t.Helper()
	var nested map[string]any
	if json.Unmarshal([]byte(headers.Get("X-Codex-Turn-Metadata")), &nested) != nil {
		t.Fatal("native trace scenario header metadata missing")
	}
	expected := nativeScenarioTraceExtra(turn)
	var differences []string
	for _, field := range []string{"scenario_trace_label", "scenario_trace_unicode"} {
		if turn == 2 {
			if _, present := nested[field]; present {
				t.Fatal("native metadata header inherited prior turn custom fields")
			}
		} else if nested[field] == nil && allowRemoval {
			differences = append(differences, "removed:http.headers.x-codex-turn-metadata."+field)
		} else if nested[field] != expected[field] {
			t.Fatalf("native custom header metadata changed: %s", field)
		}
	}
	return differences
}

func TestNativeScenarioTraceMetadata(t *testing.T) {
	for _, tracing := range []bool{false, true} {
		for _, route := range []string{"direct", "api-key", "subscription"} {
			for _, ws := range []bool{false, true} {
				t.Run(fmt.Sprintf("tracing=%t/%s/ws=%t", tracing, route, ws), func(t *testing.T) {
					capture := newNativeCapture(t)
					options := nativeOptions{endpoint: capture.server.URL, model: "gpt-5.4", ws: ws}
					var gateway nativeGateway
					if route != "direct" {
						gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
						options.endpoint, options.bearer = gateway.server.URL, gateway.secret
					}
					if tracing {
						options.traceEndpoint = nativeScenarioTraceCollector(t)
					}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					for turn := 0; turn < 3; turn++ {
						var trace map[string]string
						if turn < 2 {
							trace = nativeScenarioTraceParent(turn)
						}
						nativeScenarioTraceTurn(client, thread, nativeScenarioTraceExtra(turn), trace)
					}
					packets := capture.tap.packets(t)
					if route != "direct" {
						packets = gateway.tap.packets(t)
					}
					wires := capture.snapshot()
					if len(wires) != len(packets) || len(businessWires(wires)) != 4 {
						t.Fatal("native trace metadata sampling sequence changed")
					}
					business := 0
					differences := map[string]bool{}
					for index, wire := range wires {
						before := nativeScenarioPacketValue(t, packets[index])
						if route == "api-key" && !bytes.Equal(packets[index].payload, wire.encodedBody) {
							t.Fatal("API-key trace scenario application payload changed")
						}
						assertNativeScenarioInferenceTraceAbsent(t, packets[index].headers, before)
						assertNativeScenarioInferenceTraceAbsent(t, wire.headers, wire.value)
						if ws && (packets[index].headers.Get("Traceparent") != "" || packets[index].headers.Get("Tracestate") != "" || wire.headers.Get("Traceparent") != "" || wire.headers.Get("Tracestate") != "") {
							t.Fatal("native trace leaked into WS handshake headers")
						}
						if before["generate"] == false {
							continue
						}
						turn := 0
						if business >= 2 {
							turn = business - 1
						}
						business++
						assertNativeScenarioTraceExtra(t, before, turn, false)
						for _, difference := range assertNativeScenarioTraceExtra(t, wire.value, turn, route == "subscription") {
							differences[difference] = true
						}
						headerTurn := turn
						if ws {
							// The handshake is the prewarm snapshot; later per-turn
							// extras must stay in the frame rather than mutate it.
							headerTurn = 2
						}
						assertNativeScenarioTraceHeaderExtra(t, packets[index].headers, headerTurn, false)
						for _, difference := range assertNativeScenarioTraceHeaderExtra(t, wire.headers, headerTurn, route == "subscription") {
							differences[difference] = true
						}
						if ws && (before["previous_response_id"] == nil || wire.value["previous_response_id"] == nil) {
							t.Fatal("metadata-only changes unexpectedly invalidated native WS baseline")
						}
						if !ws && (before["previous_response_id"] != nil || wire.value["previous_response_id"] != nil) {
							t.Fatal("native HTTP metadata scenario was not full context")
						}
						for _, difference := range assertNativeScenarioTraceCarrier(t, tracing, ws, route, turn, packets[index], before, wire) {
							differences[difference] = true
						}
					}
					var fields []string
					for field := range differences {
						fields = append(fields, field)
					}
					sort.Strings(fields)
					t.Logf("trace conformance=%t; differing fields=%v; business requests=%d", len(fields) == 0, fields, business)
				})
			}
		}
	}
}

func assertNativeScenarioInferenceTraceAbsent(t *testing.T, headers http.Header, value map[string]any) {
	t.Helper()
	if headers.Get("X-Codex-Inference-Call-Id") != "" {
		t.Fatal("inference-call header present with rollout tracing disabled")
	}
	flat, nested := nativeScenarioTraceMetadata(t, value)
	for _, carrier := range []map[string]any{value, flat, nested} {
		for _, field := range []string{"inference_call_id", "x-codex-inference-call-id"} {
			if _, present := carrier[field]; present {
				t.Fatal("inference-call ID appeared in an unexpected body carrier")
			}
		}
	}
}

func assertNativeScenarioTraceCarrier(t *testing.T, tracing, ws bool, route string, turn int, packet nativePacket, before map[string]any, wire nativeWire) []string {
	t.Helper()
	b, _ := nativeScenarioTraceMetadata(t, before)
	a, _ := nativeScenarioTraceMetadata(t, wire.value)
	var differences []string
	for _, field := range []string{"traceparent", "tracestate"} {
		frameKey := "ws_request_header_" + field
		for _, flat := range []map[string]any{b, a} {
			if _, present := flat[field]; present {
				t.Fatalf("trace appeared in unprefixed metadata: %s", field)
			}
		}
		original, projected := packet.headers.Get(field), wire.headers.Get(field)
		if ws {
			original, _ = b[frameKey].(string)
			projected, _ = a[frameKey].(string)
		} else if b[frameKey] != nil || a[frameKey] != nil {
			t.Fatalf("WS trace field appeared in HTTP body: %s", frameKey)
		}
		if !tracing {
			if original != "" || projected != "" {
				t.Fatalf("trace carrier appeared without active native tracer: %s", field)
			}
			continue
		}
		if field == "traceparent" {
			parts := nativeScenarioTraceParts(t, original)
			if turn < 2 {
				parent := nativeScenarioTraceParts(t, nativeScenarioTraceParent(turn)[field])
				if parts[1] != parent[1] || parts[2] == parent[2] || parts[3] != parent[3] {
					t.Fatal("native trace did not preserve parent trace and derive its own span")
				}
			} else {
				for prior := 0; prior < 2; prior++ {
					parent := nativeScenarioTraceParts(t, nativeScenarioTraceParent(prior)[field])
					if parts[1] == parent[1] {
						t.Fatal("native new turn inherited prior request trace without a carrier")
					}
				}
			}
		} else if turn < 2 && original != nativeScenarioTraceParent(turn)[field] {
			t.Fatal("native tracestate did not preserve incoming request context")
		} else if turn == 2 && original != "" {
			t.Fatal("native new turn inherited prior request tracestate")
		}
		if reflect.DeepEqual(original, projected) {
			continue
		}
		// Existing coordinator allowlist omits both W3C HTTP headers. Record
		// this exact observed conformance gap, including the API-key control.
		if !ws && route != "direct" && original != "" && projected == "" {
			differences = append(differences, "removed:http.headers."+field)
			continue
		}
		t.Fatalf("unclassified native trace projection difference: %s", field)
	}
	return differences
}
