package integration

import (
	"fmt"
	"reflect"
	"testing"
)

func TestCodex158TypedCatalogSchemasAtEgress(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, layout := range []string{"ordinary", "converted-lite", "formed-lite"} {
			for _, namespace := range []bool{false, true} {
				t.Run(fmt.Sprintf("ws=%t/%s/namespace=%t", ws, layout, namespace), func(t *testing.T) {
					peer := newFidelityPeer(t, ws)
					parameters := map[string]any{
						"type": "object",
						"properties": map[string]any{
							"cell_id":    map[string]any{"type": "string"},
							"max_tokens": map[string]any{"enum": []any{float64(128), float64(256)}},
							"hint":       map[string]any{"description": "synthetic untyped hint"},
						},
						"$defs":    map[string]any{"unused": map[string]any{"type": "number"}},
						"required": []any{"cell_id"}, "additionalProperties": false,
					}
					tools := []any{map[string]any{"type": "function", "name": "wait", "description": "synthetic", "strict": false, "parameters": parameters}}
					if namespace {
						tools = []any{map[string]any{"type": "namespace", "name": "synthetic", "description": "synthetic", "tools": tools}}
					}
					body := fidelityBody(fidelityUser())
					body["tools"] = tools
					if layout != "ordinary" {
						body["model"] = "gpt-6-sol"
					}
					if layout == "formed-lite" {
						delete(body, "tools")
						body["input"] = []any{
							map[string]any{"type": "additional_tools", "role": "developer", "tools": tools}, fidelityUser(),
						}
					}
					wire, _, _ := peer.send(body, nil)
					if !reflect.DeepEqual(find158WaitParameters(wire), parameters) {
						t.Fatal("native typed schema changed at loopback egress")
					}
				})
			}
		}
	}
}

func find158WaitParameters(value any) map[string]any {
	switch value := value.(type) {
	case map[string]any:
		if value["type"] == "function" && value["name"] == "wait" {
			parameters, _ := value["parameters"].(map[string]any)
			return parameters
		}
		for _, key := range []string{"tools", "input"} {
			if found := find158WaitParameters(value[key]); found != nil {
				return found
			}
		}
	case []any:
		for _, child := range value {
			if found := find158WaitParameters(child); found != nil {
				return found
			}
		}
	}
	return nil
}
