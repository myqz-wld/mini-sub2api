//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"
)

// The caller side of a mediated scenario is captured from the actual native child,
// before the gateway changes any fields. Raw bodies remain bounded, in memory only.
func scenarioBaseRun(t *testing.T, route string, options nativeOptions, turns func(*nativeClient, string)) ([]nativeWire, []nativeWire) {
	t.Helper()
	capture := newNativeCapture(t)
	options.endpoint = capture.server.URL
	var gateway nativeGateway
	if route != "direct" {
		gateway = newNativeGateway(t, options.endpoint, route == "subscription")
		options.endpoint, options.bearer = gateway.server.URL, gateway.secret
	}
	client := startNativeClient(t, options)
	thread := client.thread(options)
	turns(client, thread)
	out := capture.snapshot()
	if len(businessWires(out)) < 1 {
		t.Fatal("base scenario produced no native inference")
	}
	if route == "direct" {
		return out, out
	}
	packets := gateway.tap.packets(t)
	if len(packets) != len(out) {
		t.Fatal("base scenario inference/prewarm count changed across gateway")
	}
	in := make([]nativeWire, len(packets))
	for i, packet := range packets {
		body := packet.payload
		if packet.headers.Get("Content-Encoding") == "zstd" {
			decoder, err := zstd.NewReader(nil, zstd.WithDecoderMaxMemory(nativeCaptureLimit), zstd.WithDecoderConcurrency(1))
			if err != nil {
				t.Fatal("base scenario decoder initialization")
			}
			body, err = decoder.DecodeAll(body, nil)
			decoder.Close()
			if err != nil {
				t.Fatal("base scenario native compressed capture")
			}
		}
		var value map[string]any
		if json.Unmarshal(body, &value) != nil {
			t.Fatal("base scenario native JSON capture")
		}
		in[i] = nativeWire{body: body, encodedBody: packet.payload, headers: packet.headers, value: value}
		if route == "api-key" && !bytes.Equal(packet.payload, out[i].encodedBody) {
			t.Fatal("API-key changed actual native base scenario bytes")
		}
		assertScenarioBaseContent(t, in[i], out[i])
	}
	return in, out
}

func assertScenarioBaseContent(t *testing.T, before, after nativeWire) {
	t.Helper()
	input, _ := before.value["input"].([]any)
	output, _ := after.value["input"].([]any)
	if len(input) != len(output) {
		t.Fatal("native instruction scenario input boundaries changed")
	}
	for i := range input {
		a, _ := input[i].(map[string]any)
		b, _ := output[i].(map[string]any)
		for _, field := range []string{"type", "role", "content", "tools"} {
			av, ap := a[field]
			bv, bp := b[field]
			if ap != bp || !reflect.DeepEqual(av, bv) {
				t.Fatalf("instruction scenario item %d field %s changed", i, field)
			}
		}
	}
}

func scenarioBasePrefixNamespace(t *testing.T, wire nativeWire) []byte {
	t.Helper()
	metadata, _ := wire.value["client_metadata"].(map[string]any)
	thread, _ := metadata["thread_id"].(string)
	if thread == "" {
		t.Fatal("base scenario thread identity missing")
	}
	oid, _ := hex.DecodeString("6ba7b8129dad11d180b400c04fd430c8")
	return uuid5(oid, []byte(thread))
}

func assertScenarioBaseValue(t *testing.T, wire nativeWire, lite bool, expected string) {
	t.Helper()
	if !lite {
		actual, present := wire.value["instructions"]
		if (expected == "" && present) || (expected != "" && actual != expected) {
			t.Fatal("ordinary native resolved base value or omission differs")
		}
		return
	}
	if _, ok := wire.value["instructions"]; ok {
		t.Fatal("native Lite serialized a top-level instructions carrier")
	}
	if _, ok := wire.value["tools"]; ok {
		t.Fatal("native Lite serialized a top-level tools carrier")
	}
	var raw struct{ Input []json.RawMessage }
	if json.Unmarshal(wire.body, &raw) != nil {
		t.Fatal("native Lite scenario prefix parse")
	}
	if len(raw.Input) == 0 {
		return // A validated empty WS suffix has no prompt prefix.
	}
	var prefix struct {
		Type, ID, Role string
		Tools          json.RawMessage
	}
	_ = json.Unmarshal(raw.Input[0], &prefix)
	if prefix.Type != "additional_tools" {
		return // Native WS reused the preceding full request's prefix.
	}
	namespace := scenarioBasePrefixNamespace(t, wire)
	if prefix.Role != "developer" || prefix.ID != "at_"+uuidText(uuid5(namespace, prefix.Tools)) {
		t.Fatal("native Lite tools prefix content-derived identity differs")
	}
	if expected == "" {
		for _, item := range raw.Input[1:] {
			var header struct{ ID string }
			_ = json.Unmarshal(item, &header)
			if header.ID == "msg_"+uuidText(uuid5(namespace, nil)) {
				t.Fatal("empty native Lite base emitted an instruction prefix")
			}
		}
		return
	}
	if len(raw.Input) < 2 {
		t.Fatal("native Lite base prefix missing")
	}
	var base struct {
		Type, ID, Role string
		Content        []struct{ Type, Text string }
	}
	_ = json.Unmarshal(raw.Input[1], &base)
	if base.Type != "message" || base.Role != "developer" || len(base.Content) != 1 || base.Content[0].Type != "input_text" || base.Content[0].Text != expected {
		t.Fatal("native Lite base text or content boundary differs")
	}
	if base.ID != "msg_"+uuidText(uuid5(namespace, []byte(expected))) {
		t.Fatal("native Lite base deterministic ID differs")
	}
}

func TestNativeScenarioBasePrecedence(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				for _, kind := range []string{"config", "file-over-config", "typed-over-file-config", "typed-empty", "typed-whitespace"} {
					t.Run(fmt.Sprintf("%s/ws=%t/%s/%s", route, ws, model, kind), func(t *testing.T) {
						inline, typed := "\n  inline {{literal}} base  \n", "\n  typed {{ personality }} base  \n"
						options := nativeOptions{model: model, ws: ws, configOverrides: map[string]string{"instructions": strconv.Quote(inline)}}
						expected := inline
						if kind == "file-over-config" || kind == "typed-over-file-config" {
							file := filepath.Join(t.TempDir(), "synthetic-base.txt")
							if os.WriteFile(file, []byte("\n  file {{literal}} base  \n"), 0600) != nil {
								t.Fatal("write synthetic instruction file")
							}
							options.configOverrides["model_instructions_file"] = strconv.Quote(file)
							expected = "file {{literal}} base"
						}
						switch kind {
						case "typed-over-file-config":
							expected = typed
							options.threadParams = map[string]any{"baseInstructions": expected}
						case "typed-empty":
							expected = ""
							options.threadParams = map[string]any{"baseInstructions": expected}
						case "typed-whitespace":
							expected = " \t\n "
							options.threadParams = map[string]any{"baseInstructions": expected}
						}
						in, out := scenarioBaseRun(t, route, options, func(client *nativeClient, thread string) {
							client.turn(thread, "synthetic base precedence")
						})
						lite := model == "gpt-5.6-sol"
						for i := range in {
							assertScenarioBaseValue(t, in[i], lite, expected)
							if route == "subscription" && !lite && strings.TrimSpace(expected) == "" {
								// Blank caller bases carry no gateway-owned prompt text.
								assertScenarioBaseValue(t, out[i], false, "")
								continue
							}
							assertScenarioBaseValue(t, out[i], lite, expected)
							if lite && route == "subscription" {
								a, b := in[i].value["input"].([]any), out[i].value["input"].([]any)
								if len(a) > 0 && a[0].(map[string]any)["type"] == "additional_tools" && a[0].(map[string]any)["id"] == b[0].(map[string]any)["id"] {
									t.Fatal("Subscription did not namespace native deterministic tools ID")
								}
							}
						}
					})
				}
			}
		}
	}
}

func TestNativeScenarioBaseDeveloperPrecedenceAndGrouping(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
			t.Run(route+"/"+model, func(t *testing.T) {
				inline, typed := "discarded synthetic developer configuration", "  selected {{literal}} developer \n"
				options := nativeOptions{model: model, base: "separate synthetic base", configOverrides: map[string]string{"developer_instructions": strconv.Quote(inline)}, threadParams: map[string]any{"developerInstructions": typed}}
				in, _ := scenarioBaseRun(t, route, options, func(client *nativeClient, thread string) { client.turn(thread, "synthetic grouping") })
				items := in[0].value["input"].([]any)
				found := 0
				for _, raw := range items {
					item, _ := raw.(map[string]any)
					content, _ := item["content"].([]any)
					for i, rawPart := range content {
						part, _ := rawPart.(map[string]any)
						text, _ := part["text"].(string)
						if strings.Contains(text, inline) {
							t.Fatal("overridden developer configuration leaked into effective context")
						}
						if text == typed {
							found++
							if item["role"] != "developer" || i != 0 || len(content) < 2 {
								t.Fatal("native initial developer fragments were not grouped in ordered content elements")
							}
							if part["type"] != "input_text" {
								t.Fatal("native developer fragment type changed")
							}
						}
					}
				}
				if found != 1 {
					t.Fatal("typed developer fragment count differs")
				}
			})
		}
	}
}

func TestNativeScenarioBaseConfigurationErrors(t *testing.T) {
	for _, kind := range []string{"missing-file", "blank-file"} {
		t.Run(kind, func(t *testing.T) {
			capture := newNativeCapture(t)
			client := startNativeClient(t, nativeOptions{endpoint: capture.server.URL, model: "gpt-5.4"})
			file := filepath.Join(t.TempDir(), "synthetic-base.txt")
			if kind == "blank-file" && os.WriteFile(file, []byte(" \t\n"), 0600) != nil {
				t.Fatal("write synthetic blank base file")
			}
			// File loading occurs before typed precedence. A valid typed base cannot
			// rescue an invalid configured file, and no Responses request is emitted.
			client.callExpect("thread/start", map[string]any{"model": "gpt-5.4", "ephemeral": true, "approvalPolicy": "never", "sandbox": "read-only", "environments": []any{}, "baseInstructions": "typed base cannot suppress a file error", "config": map[string]any{"model_instructions_file": file}}, -32600)
			if len(capture.snapshot()) != 0 {
				t.Fatal("invalid instruction configuration reached inference")
			}
		})
	}
}
