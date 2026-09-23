//go:build nativeparity

package integration

import (
	"fmt"
	"net/http"
	"reflect"
	"slices"
	"sync"
	"testing"
)

type callerWireBaseline struct {
	shape                 *nativeJSONShape
	headerSpellings       []string
	headers               http.Header
	routedHeaderSpellings []string
}

var callerWireBaselines = struct {
	sync.Mutex
	values map[string]callerWireBaseline
}{values: map[string]callerWireBaseline{}}

func callerWireBaselineFor(t *testing.T, model string, ws bool) callerWireBaseline {
	t.Helper()
	key := fmt.Sprintf("%s/%t", model, ws)
	callerWireBaselines.Lock()
	defer callerWireBaselines.Unlock()
	if baseline, ok := callerWireBaselines.values[key]; ok {
		return baseline
	}
	capture := &nativeCapture{t: t, httpRoutingToken: true, longRoutingToken: true}
	metadata := newNativeCapture(t)
	capture.server, capture.tap = newNativeTappedServer(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !ws && r.Method == http.MethodGet && r.URL.Path == "/backend-api/codex/responses" {
			w.WriteHeader(http.StatusMethodNotAllowed)
			return
		}
		capture.serve(w, r)
	}))
	options := nativeOptions{endpoint: capture.server.URL, metadataEndpoint: metadata.server.URL, bearer: "synthetic-caller-wire-bearer", model: model, ws: ws, builtinOpenAI: true, base: "Synthetic caller wire baseline"}
	client := startNativeClient(t, options)
	thread := client.thread(options)
	client.turn(thread, "First synthetic baseline turn")
	client.turn(thread, "Second synthetic baseline turn")
	packets, wires := capture.tap.packets(t), capture.snapshot()
	if len(packets) != len(wires) || len(wires) < 3 {
		t.Fatal("native caller baseline missed a tool or user turn")
	}
	var first *nativePacket
	var shape *nativeJSONShape
	var routedHeaders []string
	for i, wire := range wires {
		if wire.value["generate"] == false {
			continue
		}
		if first == nil {
			first = &packets[i]
			shape = readNativeJSONShape(t, wire.body)
		}
		if len(packets[i].headers.Values("X-Codex-Turn-State")) > 0 {
			routedHeaders = append([]string(nil), packets[i].headerSpellings...)
		}
		marker := "pub struct ResponsesApiRequest"
		if ws {
			marker = "pub struct ResponseCreateWsRequest"
		}
		order := callerNativeFieldOrder(t, "codex-api/src/common.rs", marker)
		if ws {
			order = append([]string{"type"}, order...)
		}
		assertCallerObject(t, readNativeJSONShape(t, wire.body), order, []string{"model", "input", "client_metadata"}, "native.$")
		input, _ := wire.value["input"].([]any)
		for j, node := range readNativeJSONShape(t, wire.body).fields["input"].items {
			value, _ := input[j].(map[string]any)
			assertCallerItemWire(t, node, value, fmt.Sprintf("native.$.input[%d]", j))
		}
	}
	if first == nil {
		t.Fatal("native caller baseline lacks a business request")
	}
	// Retain shape/header metadata only. Raw request bodies remain owned by this test fixture.
	headers := make(http.Header)
	for _, name := range []string{"Originator", "Version", "Accept", "Content-Type", "Content-Encoding", "Openai-Beta", "Upgrade", "Connection", "Sec-Websocket-Version", "Sec-Websocket-Extensions", "X-OpenAI-Internal-Codex-Responses-Lite"} {
		headers[name] = first.headers.Values(name)
	}
	baseline := callerWireBaseline{shape: shape, headerSpellings: append([]string(nil), first.headerSpellings...), headers: headers, routedHeaderSpellings: routedHeaders}
	callerWireBaselines.values[key] = baseline
	return baseline
}

func assertCallerHeaderContract(t *testing.T, baseline callerWireBaseline, sent nativePacket) {
	t.Helper()
	expected := baseline.headerSpellings
	if len(sent.headers.Values("X-Codex-Turn-State")) > 0 {
		if len(baseline.routedHeaderSpellings) == 0 {
			t.Fatal("native routing-header variant missing")
		}
		expected = baseline.routedHeaderSpellings
	}
	if !reflect.DeepEqual(expected, sent.headerSpellings) {
		t.Errorf("caller complete header order/presence/casing differs: native=%v caller=%v", expected, sent.headerSpellings)
	}
	for name, expected := range baseline.headers {
		if !slices.Equal(expected, sent.headers.Values(name)) {
			t.Errorf("caller native header value differs: %s", name)
		}
	}
}

func TestNativeOrdinaryWireShapeCapture(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
		for _, ws := range []bool{false, true} {
			for _, form := range []string{"minimal", "explicit", "null"} {
				t.Run(fmt.Sprintf("%s/ws=%t/%s", model, ws, form), func(t *testing.T) {
					capture := newNativeCapture(t)
					capture.business = 1
					gateway := newNativeGateway(t, capture.server.URL, true)
					client := ordinaryClient(t, gateway, ws, true, nil)
					request := map[string]any{"model": model, "input": []any{map[string]any{"role": "user", "content": "Synthetic bare wire probe"}}}
					if form == "explicit" {
						request["instructions"] = "  Synthetic caller base  "
						request["tools"] = []any{map[string]any{"type": "function", "name": "probe", "description": "", "parameters": map[string]any{"required": []any{}, "type": "object", "properties": map[string]any{"z": map[string]any{"description": "", "type": "string"}, "a": map[string]any{"type": "boolean"}}}}}
						request["metadata"] = map[string]any{"z": nil, "a": []any{}}
						request["stream_options"] = map[string]any{"reasoning_summary_delivery": "sequential_cutoff", "include_obfuscation": false}
						request["wire_unknown_probe"] = true
					}
					if form == "null" {
						for _, name := range []string{"instructions", "reasoning", "text", "stream_options"} {
							request[name] = nil
						}
						request["tools"] = []any{}
					}
					client.send(request)
					packets, wires, sent := gateway.tap.packets(t), capture.snapshot(), capture.tap.packets(t)
					if len(packets) != 1 || len(wires) != len(sent) {
						t.Fatal("bare wire capture count")
					}
					business := 0
					for i, wire := range wires {
						if wire.value["generate"] == false {
							continue
						}
						assertCallerWireContract(t, packets[0], wire, sent[i])
						business++
					}
					if business != 1 {
						t.Fatal("bare wire capture added business inference")
					}
				})
			}
		}
	}
}
