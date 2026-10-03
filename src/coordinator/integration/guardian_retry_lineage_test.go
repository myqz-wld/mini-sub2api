package integration

import (
	"fmt"
	"net/http"
	"testing"
)

func TestGuardianClassifierRetryKeepsTurnAcrossLeaseThreads(t *testing.T) {
	for _, ws := range []bool{false, true} {
		t.Run(fmt.Sprintf("ws=%v", ws), func(t *testing.T) {
			peer := newFidelityPeer(t, ws)
			source := fidelityBody(fidelityUser())
			source["client_metadata"] = map[string]any{"session_id": "retry-source", "thread_id": "retry-source", "turn_id": "source-turn"}
			sourceWire, _, reply := peer.send(source, nil)
			sourceFlat, _ := fidelityMetadata(t, sourceWire)
			var firstTurn, firstThread any
			for attempt := 0; attempt < 3; attempt++ {
				thread := fmt.Sprintf("classifier-lease-%d", attempt)
				turn := map[string]any{"session_id": "retry-source", "thread_id": thread, "turn_id": "one-classification", "parent_turn_id": "source-turn", "root_turn_id": "source-turn", "guardian_classifier_source_thread_id": "retry-source", "thread_source": "guardian_classifier", "turn_trigger": "guardian_classifier"}
				body := fidelityBody(fidelityUser())
				body["model"] = "gpt-5.6-luna"
				body["prompt_cache_key"] = "guardian-v2:retry-source"
				body["client_metadata"] = map[string]any{"session_id": "retry-source", "thread_id": thread, "turn_id": "one-classification", "parent_turn_id": "source-turn", "parent_response_id": reply["id"], "x-openai-subagent": "guardian", "x-codex-turn-metadata": string(mustRequestJSON(t, turn))}
				wire, _, _ := peer.send(body, http.Header{"X-Openai-Subagent": []string{"guardian"}})
				flat, out := fidelityMetadata(t, wire)
				if attempt == 0 {
					firstTurn, firstThread = flat["turn_id"], flat["thread_id"]
				} else if flat["turn_id"] != firstTurn || flat["thread_id"] == firstThread {
					t.Fatal("classifier retry changed logical turn or reused another lease thread")
				}
				if out["guardian_classifier_source_thread_id"] != sourceFlat["thread_id"] || out["parent_turn_id"] != sourceFlat["turn_id"] {
					t.Fatal("classifier retry changed source ancestry")
				}
			}
		})
	}
}
