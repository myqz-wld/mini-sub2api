package integration

import (
	"fmt"
	"net/http"
	"testing"
)

func TestFidelityClassifierUsesSourceChildAndOptionalRoot(t *testing.T) {
	for _, ws := range []bool{false, true} {
		t.Run(fmt.Sprintf("ws=%v", ws), func(t *testing.T) {
			peer := newFidelityPeer(t, ws)
			root := fidelityBody(fidelityUser())
			root["client_metadata"] = map[string]any{"session_id": "lineage-root", "thread_id": "lineage-root", "turn_id": "lineage-root-turn"}
			rootWire, _, _ := peer.send(root, nil)
			source := fidelityBody(fidelityUser())
			source["client_metadata"] = map[string]any{"session_id": "lineage-root", "thread_id": "source-child", "turn_id": "source-child-turn", "parent_thread_id": "lineage-root", "parent_turn_id": "lineage-root-turn", "root_turn_id": "lineage-root-turn"}
			sourceWire, _, reply := peer.send(source, http.Header{"X-Openai-Subagent": []string{"collab_spawn"}})
			rootFlat, _ := fidelityMetadata(t, rootWire)
			sourceFlat, _ := fidelityMetadata(t, sourceWire)
			for _, explicitRoot := range []bool{true, false} {
				body := fidelityBody(fidelityUser())
				body["model"] = "gpt-5.6-luna"
				body["prompt_cache_key"] = "guardian-v2:source-child"
				turn := map[string]any{"session_id": "lineage-root", "thread_id": fmt.Sprintf("classifier-child-%v", explicitRoot), "turn_id": fmt.Sprintf("classifier-turn-%v", explicitRoot), "parent_turn_id": "source-child-turn", "guardian_classifier_source_thread_id": "source-child", "thread_source": "guardian_classifier", "turn_trigger": "guardian_classifier"}
				if explicitRoot {
					turn["root_turn_id"] = "lineage-root-turn"
				}
				body["client_metadata"] = map[string]any{"session_id": "lineage-root", "thread_id": turn["thread_id"], "turn_id": turn["turn_id"], "parent_turn_id": "source-child-turn", "parent_response_id": reply["id"], "x-codex-turn-metadata": string(mustRequestJSON(t, turn))}
				wire, _, _ := peer.send(body, http.Header{"X-Codex-Guardian": []string{"classifier"}, "X-Openai-Subagent": []string{"guardian"}})
				_, out := fidelityMetadata(t, wire)
				if out["guardian_classifier_source_thread_id"] != sourceFlat["thread_id"] || out["parent_turn_id"] != sourceFlat["turn_id"] {
					t.Fatal("classifier did not follow its child source")
				}
				if explicitRoot && out["root_turn_id"] != rootFlat["turn_id"] {
					t.Fatal("classifier root mapping changed")
				}
				if !explicitRoot && out["root_turn_id"] != nil {
					t.Fatal("classifier invented optional root")
				}
			}
		})
	}
}
