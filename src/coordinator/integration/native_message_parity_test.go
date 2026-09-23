//go:build nativeparity

package integration

import (
	"encoding/json"
	"fmt"
	"reflect"
	"slices"
	"testing"

	"github.com/klauspost/compress/zstd"
)

func TestNativeCapturedLiteReconstructionAfterReferenceAndReconnect(t *testing.T) {
	direct := newNativeCapture(t)
	options := nativeOptions{endpoint: direct.server.URL, model: "gpt-5.6-sol", base: "  native reconstruction base  "}
	native := startNativeClient(t, options)
	thread := native.thread(options)
	native.turn(thread, "synthetic native reconstruction seed")
	sample := direct.snapshot()[0].body
	for _, ws := range []bool{false, true} {
		t.Run(fmt.Sprintf("ws=%t", ws), func(t *testing.T) {
			upstream := newNativeCapture(t)
			gateway := newNativeGateway(t, upstream.server.URL, true)
			client := ordinaryClient(t, gateway, ws, true, nil)
			// Preserve the captured tools JSON bytes: map re-serialization would invalidate
			// the native content-derived ID before the request ever reaches the gateway.
			full := sample
			if ws {
				full = append([]byte(`{"type":"response.create",`), sample[1:]...)
			}
			response := client.sendEncoded(full)
			call := response["output"].([]any)[0].(map[string]any)
			if ws {
				_ = client.ws.CloseNow()
				client = ordinaryClient(t, gateway, true, true, nil)
			}
			client.send(map[string]any{"model": options.model, "previous_response_id": response["id"], "input": []any{map[string]any{"type": "function_call_output", "call_id": call["call_id"], "output": "reconstructed tool result"}}})
			wires := upstream.snapshot()
			last := wires[len(wires)-1]
			if ws && last.value["previous_response_id"] != nil {
				if len(wires) < 2 {
					t.Fatal("reconnect baseline missing")
				}
				baseline := wires[len(wires)-2]
				if baseline.connection != last.connection || baseline.value["generate"] != false || baseline.value["previous_response_id"] != nil {
					t.Fatal("reconnect reused an old socket reference")
				}
				last = baseline // A full hidden warmup installs a fresh baseline before the delta.
			}
			if last.value["previous_response_id"] != nil {
				t.Fatal("reference reconstruction kept unusable connection baseline")
			}
			assertNativeLiteIDs(t, last)
			if capturedBase(t, last) != options.base {
				t.Fatal("native reconstructed base changed")
			}
			if len(businessWires(wires)) != 2 {
				t.Fatal("reference reconstruction replayed a business request")
			}
		})
	}
}

func assertNativeMessageParity(t *testing.T, packet nativePacket, wire nativeWire) {
	t.Helper()
	raw := packet.payload
	if packet.headers.Get("Content-Encoding") == "zstd" {
		decoder, err := zstd.NewReader(nil, zstd.WithDecoderMaxMemory(nativeCaptureLimit), zstd.WithDecoderConcurrency(1))
		if err != nil {
			t.Fatal("native comparison decoder")
		}
		defer decoder.Close()
		raw, err = decoder.DecodeAll(raw, nil)
		if err != nil {
			t.Fatal("native comparison zstd")
		}
	}
	var caller map[string]any
	assertNativeJSONShape(t, raw, wire.body)
	if json.Unmarshal(raw, &caller) != nil {
		t.Fatal("native comparison JSON")
	}
	beforeMetadata, _ := caller["client_metadata"].(map[string]any)
	afterMetadata, _ := wire.value["client_metadata"].(map[string]any)
	var beforeTurn, afterTurn map[string]any
	beforeRaw, _ := beforeMetadata["x-codex-turn-metadata"].(string)
	afterRaw, _ := afterMetadata["x-codex-turn-metadata"].(string)
	if json.Unmarshal([]byte(beforeRaw), &beforeTurn) != nil || json.Unmarshal([]byte(afterRaw), &afterTurn) != nil {
		t.Fatal("native turn metadata comparison JSON")
	}
	for _, field := range []string{"analytics_enabled", "model", "reasoning_effort"} {
		if expected, present := beforeTurn[field]; present && !reflect.DeepEqual(expected, afterTurn[field]) {
			t.Errorf("native turn metadata differs: %s", field)
		}
	}
	for field, expected := range caller {
		if slices.Contains([]string{"client_metadata", "input", "prompt_cache_key", "previous_response_id"}, field) {
			continue
		}
		if !reflect.DeepEqual(expected, wire.value[field]) {
			t.Errorf("native message setting differs: %s", field)
		}
	}
	before, _ := caller["input"].([]any)
	after, _ := wire.value["input"].([]any)
	if len(before) != len(after) {
		t.Fatal("native message sequence length changed")
	}
	for i, raw := range before {
		original, ok := raw.(map[string]any)
		if !ok {
			t.Fatal("native input item shape")
		}
		emitted, ok := after[i].(map[string]any)
		if !ok {
			t.Fatal("projected input item shape")
		}
		for field, expected := range original {
			if slices.Contains([]string{"id", "call_id", "previous_item_id", "internal_chat_message_metadata_passthrough"}, field) {
				continue
			}
			if !reflect.DeepEqual(expected, emitted[field]) {
				t.Errorf("native ordered item %d content differs: %s", i, field)
			}
		}
		for _, field := range []string{"id", "call_id", "previous_item_id"} {
			if _, present := original[field]; present {
				if id, ok := emitted[field].(string); !ok || id == "" {
					t.Errorf("native typed item reference disappeared: %s", field)
				}
			}
		}
	}
	// Identity values are validated by assertNativeWireParity; the input traversal deliberately
	// does not mask arbitrary IDs embedded in tool definitions, arguments, content or output.
	metadata, _ := wire.value["client_metadata"].(map[string]any)
	if key := wire.value["prompt_cache_key"]; key != metadata["session_id"] && key != metadata["thread_id"] {
		t.Fatal("native cache/session relationship")
	}
}

func TestNativePragmaticTemplateSurvivesGateway(t *testing.T) {
	for _, model := range []string{"gpt-5.5", "gpt-5.4", "gpt-5.4-mini"} {
		t.Run(model, func(t *testing.T) {
			direct := newNativeCapture(t)
			options := nativeOptions{endpoint: direct.server.URL, model: model}
			native := startNativeClient(t, options)
			thread := native.thread(options)
			native.turn(thread, "synthetic personality probe")
			expected := capturedBase(t, direct.snapshot()[0])
			upstream := newNativeCapture(t)
			gateway := newNativeGateway(t, upstream.server.URL, true)
			options.endpoint = gateway.server.URL
			options.bearer = gateway.secret
			mediated := startNativeClient(t, options)
			thread = mediated.thread(options)
			mediated.turn(thread, "synthetic personality probe")
			for _, wire := range upstream.snapshot() {
				if capturedBase(t, wire) != expected {
					t.Fatal("native default pragmatic template changed")
				}
			}
		})
	}
}
