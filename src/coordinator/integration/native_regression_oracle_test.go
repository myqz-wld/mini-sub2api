//go:build nativeparity

package integration

import (
	"context"
	"os"
	"os/exec"
	"testing"
	"time"
)

func TestNativeTransportProjectionMutationControls(t *testing.T) {
	for _, mutation := range []string{"none", "added-request-field", "added-metadata", "changed-content", "changed-opaque-id", "reordered-input", "collapsed-items", "unstable-turn"} {
		t.Run(mutation, func(t *testing.T) {
			binary, err := os.Executable()
			if err != nil {
				t.Fatal("test executable unavailable")
			}
			ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
			defer cancel()
			command := exec.CommandContext(ctx, binary, "-test.run=^TestTransportProjectionOracleChild$", "-test.count=1")
			command.Env = []string{"MINI_SUB2API_ORACLE_MUTATION=" + mutation}
			output, err := command.CombinedOutput()
			_ = output // Bounded synthetic assertion output stays in memory and is never emitted.
			if ctx.Err() != nil {
				t.Fatal("isolated oracle probe timed out")
			}
			if mutation == "none" && err != nil {
				t.Fatal("unmodified oracle control rejected")
			}
			if mutation != "none" && err == nil {
				t.Fatal("semantic mutation escaped the strict comparator")
			}
		})
	}
}

func TestTransportProjectionOracleChild(t *testing.T) {
	mutation := os.Getenv("MINI_SUB2API_ORACLE_MUTATION")
	if mutation == "" {
		return
	}
	before := map[string]any{
		"model":            "gpt-5.4",
		"client_metadata":  map[string]any{"session_id": "source-thread", "thread_id": "source-thread", "turn_id": "source-turn"},
		"prompt_cache_key": "source-thread",
		"tools":            []any{map[string]any{"type": "function", "name": "probe", "parameters": map[string]any{"type": "object", "properties": map[string]any{"id": map[string]any{"const": "opaque-literal"}}}}},
		"input": []any{
			map[string]any{"type": "message", "role": "developer", "id": "source-item-a", "content": []any{map[string]any{"type": "input_text", "text": "synthetic developer"}}},
			map[string]any{"type": "message", "role": "user", "id": "source-item-b", "content": []any{map[string]any{"type": "input_text", "text": "synthetic user"}}},
		},
	}
	after := cloneOrdinaryContext(t, before)
	metadata := after["client_metadata"].(map[string]any)
	metadata["session_id"], metadata["thread_id"], metadata["turn_id"] = "projected-thread", "projected-thread", "projected-turn"
	after["prompt_cache_key"] = "projected-thread"
	input := after["input"].([]any)
	input[0].(map[string]any)["id"] = "projected-item-a"
	input[1].(map[string]any)["id"] = "projected-item-b"
	projection := transportProjection{identities: map[string]map[string]string{}}
	if mutation == "unstable-turn" {
		projection.compare(t, before, after, "request")
		metadata["turn_id"] = "different-projected-turn"
	}
	switch mutation {
	case "added-request-field":
		after["unexpected"] = true
	case "added-metadata":
		metadata["unexpected"] = "added"
	case "changed-content":
		input[1].(map[string]any)["content"].([]any)[0].(map[string]any)["text"] = "changed"
	case "changed-opaque-id":
		after["tools"].([]any)[0].(map[string]any)["parameters"].(map[string]any)["properties"].(map[string]any)["id"].(map[string]any)["const"] = "changed"
	case "reordered-input":
		input[0], input[1] = input[1], input[0]
	case "collapsed-items":
		input[1].(map[string]any)["id"] = "projected-item-a"
	}
	projection.compare(t, before, after, "request")
}
