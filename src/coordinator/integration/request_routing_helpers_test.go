package integration

import (
	"encoding/json"
	"testing"
)

func canonicalExpectedTools(tools []any) []any {
	canonical := make([]any, 0, len(tools))
	for _, tool := range tools {
		object := tool.(map[string]any)
		copy := make(map[string]any, len(object)+1)
		for name, value := range object {
			copy[name] = value
		}
		if copy["type"] == "function" {
			if _, ok := copy["strict"]; !ok {
				copy["strict"] = false
			}
		}
		canonical = append(canonical, copy)
	}
	return canonical
}

func canonicalExpectedLiteTools(tools []any) []any {
	functions := make([]any, 0)
	grouped := make([]any, 0, len(tools))
	functionIndex := -1
	for _, tool := range canonicalExpectedTools(tools) {
		object := tool.(map[string]any)
		if object["type"] == "function" || object["type"] == "custom" {
			if functionIndex == -1 {
				functionIndex = len(grouped)
			}
			functions = append(functions, object)
			continue
		}
		grouped = append(grouped, object)
	}
	if functionIndex >= 0 {
		namespace := map[string]any{
			"type": "namespace", "name": "functions", "description": "", "tools": functions,
		}
		grouped = append(grouped, nil)
		copy(grouped[functionIndex+1:], grouped[functionIndex:])
		grouped[functionIndex] = namespace
	}
	return grouped
}

func mustRequestJSON(t *testing.T, value any) []byte {
	t.Helper()
	encoded, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	return encoded
}

func decodeRequestObject(t *testing.T, body []byte) map[string]any {
	t.Helper()
	var value map[string]any
	if err := json.Unmarshal(body, &value); err != nil {
		t.Fatalf("decode upstream request: %v; body=%s", err, body)
	}
	return value
}
