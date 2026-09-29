package integration

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"
)

func TestCodex1580MCPAttributionAtUpstream(t *testing.T) {
	for _, ws := range []bool{false, true} {
		t.Run(fmt.Sprintf("ws=%t", ws), func(t *testing.T) {
			peer := newFidelityPeer(t, ws)
			first := fidelityBody(fidelityUser())
			first["client_metadata"] = map[string]any{"session_id": "mcp-session", "thread_id": "mcp-session", "turn_id": "mcp-first"}
			wire, _, response := peer.send(first, nil)
			turn := wire["client_metadata"].(map[string]any)["turn_id"]
			attribution := map[string]any{"status": "complete", "sources": []any{map[string]any{"server_name": "synthetic-server", "tool_name": "lookup", "first_turn_id": "mcp-first"}}}
			body := fidelityBody(fidelityUser())
			body["previous_response_id"] = response["id"]
			body["client_metadata"] = map[string]any{"mcp_attribution": string(mustRequestJSON(t, attribution)),
				"x-codex-turn-metadata": `{"mcp_attribution":"nested-must-not-cross"}`}
			wire, headers, _ := peer.send(body, nil)
			metadata := wire["client_metadata"].(map[string]any)
			var emitted map[string]any
			if json.Unmarshal([]byte(metadata["mcp_attribution"].(string)), &emitted) != nil {
				t.Fatal("MCP attribution was not encoded as body metadata")
			}
			if emitted["sources"].([]any)[0].(map[string]any)["first_turn_id"] != turn {
				t.Fatal("MCP attribution did not preserve the first mapped turn")
			}
			if headers.Get("Mcp-Attribution") != "" || strings.Contains(headers.Get("X-Codex-Turn-Metadata"), "mcp_attribution") {
				t.Fatal("body-only MCP attribution entered a header")
			}
			attribution["sources"].([]any)[0].(map[string]any)["server_name"] = strings.Repeat("x", 16*1024)
			body = fidelityBody(fidelityUser())
			body["client_metadata"] = map[string]any{"mcp_attribution": string(mustRequestJSON(t, attribution))}
			wire, _, _ = peer.send(body, nil)
			raw := wire["client_metadata"].(map[string]any)["mcp_attribution"].(string)
			if len(raw) > 16*1024 || !strings.Contains(raw, `"error_reason":"payload_too_large"`) {
				t.Fatal("MCP attribution overflow lacked the bounded native fallback")
			}
		})
	}
}
