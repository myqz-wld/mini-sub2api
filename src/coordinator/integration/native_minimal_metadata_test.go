//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

// The oracle is the independently pinned catalog, not a copy of the gateway's model switch.
// These are ordinary caller requests; the fixed upstream responder does not evaluate model answers.
func TestNativeOrdinaryModelMetadataCatalog(t *testing.T) {
	var catalog struct {
		Models []struct {
			Slug     string `json:"slug"`
			Review   bool   `json:"node_repl_auto_review_required"`
			Disabled bool   `json:"node_repl_disabled"`
		} `json:"models"`
	}
	raw, err := os.ReadFile(filepath.Join(nativeSource(t), "codex-rs/models-manager/models.json"))
	if err != nil || json.Unmarshal(raw, &catalog) != nil || len(catalog.Models) != 9 {
		t.Fatal("pinned native metadata catalog unavailable")
	}
	for _, model := range catalog.Models {
		for _, subscription := range []bool{false, true} {
			for _, delivery := range []string{"json", "sse", "ws"} {
				t.Run(fmt.Sprintf("%s/subscription=%t/%s", model.Slug, subscription, delivery), func(t *testing.T) {
					capture := newNativeCapture(t)
					gateway := newNativeGateway(t, capture.server.URL, subscription)
					client := ordinaryClient(t, gateway, delivery == "ws", delivery != "json", nil)
					format := map[string]any{"type": "json_schema", "name": "translation", "strict": true, "schema": map[string]any{"type": "object", "properties": map[string]any{"text": map[string]any{"type": "string"}}, "required": []any{"text"}, "additionalProperties": false}}
					request := map[string]any{"model": model.Slug, "input": "Translate the supplied synthetic text.", "instructions": "  Return only the requested translation. {{literal}}  ", "text": map[string]any{"format": format}, "max_output_tokens": 64, "temperature": 0.2, "top_p": 0.8}
					client.send(request)
					wires := capture.snapshot()
					if len(businessWires(wires)) != 1 {
						t.Fatal("minimal caller inference count changed")
					}
					if !subscription {
						packets := gateway.tap.packets(t)
						if len(packets) != 1 || len(wires) != 1 || !bytes.Equal(packets[0].payload, wires[0].encodedBody) {
							t.Fatal("minimal API-key request changed")
						}
						return
					}
					for _, wire := range wires {
						metadata, ok := wire.value["client_metadata"].(map[string]any)
						if !ok {
							t.Fatal("minimal client metadata absent")
						}
						if metadata["guardian_credits_requested"] != "true" {
							t.Fatal("minimal caller lacks native backend Guardian metadata")
						}
						var turn map[string]any
						if encoded, ok := metadata["x-codex-turn-metadata"].(string); !ok || json.Unmarshal([]byte(encoded), &turn) != nil {
							t.Fatal("invalid minimal turn metadata")
						}
						if turn["node_repl_auto_review_required"] != model.Review || turn["node_repl_disabled"] != model.Disabled || turn["auto_review_enabled"] != false {
							t.Fatal("generated policy differs from pinned catalog")
						}
						if turn["agent_name"] != "/root" || turn["workspaces"] != nil || turn["tool_namespaces_info"] != nil {
							t.Fatal("minimal caller acquired unrequested environment")
						}
						for _, field := range []string{"max_output_tokens", "temperature", "top_p"} {
							if _, exists := wire.value[field]; exists {
								t.Fatal("server-unsupported control was forwarded")
							}
						}
					}
					value := ordinaryResolvedFirst(t, wires).value
					text, _ := value["text"].(map[string]any)
					if !reflect.DeepEqual(text["format"], format) {
						t.Fatal("ordinary structured output format changed")
					}
					input, _ := value["input"].([]any)
					if len(input) == 0 {
						t.Fatal("ordinary input lost")
					}
					first, _ := input[0].(map[string]any)
					if first["type"] == "additional_tools" {
						tools, ok := first["tools"].([]any)
						if !ok || len(tools) != 0 || len(input) != 3 {
							t.Fatal("minimal Lite caller acquired tools or instructions")
						}
						if capturedBase(t, ordinaryResolvedFirst(t, wires)) != request["instructions"] {
							t.Fatal("minimal Lite base changed")
						}
					} else if tools, ok := value["tools"].([]any); len(input) != 1 || value["instructions"] != request["instructions"] || !ok || len(tools) != 0 {
						t.Fatal("minimal Responses caller lost the native empty-tools carrier or changed instructions")
					}
					last, _ := input[len(input)-1].(map[string]any)
					content, _ := last["content"].([]any)
					if last["role"] != "user" || len(content) != 1 || content[0].(map[string]any)["text"] != request["input"] {
						t.Fatal("minimal user content changed")
					}
				})
			}
		}
	}
}
