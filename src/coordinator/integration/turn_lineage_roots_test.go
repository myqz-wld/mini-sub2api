package integration

import (
	"fmt"
	"testing"
)

func TestTurnLineageChildOwnsItsRootAndDescendantsInheritIt(t *testing.T) {
	for _, ws := range []bool{false, true} {
		t.Run(fmt.Sprintf("ws=%v", ws), func(t *testing.T) {
			peer := newFidelityPeer(t, ws)
			child := fidelityBody(fidelityUser())
			child["client_metadata"] = map[string]any{"session_id": "root-session", "thread_id": "child-thread", "parent_thread_id": "root-session", "turn_id": "child-root-turn", "root_turn_id": "child-root-turn"}
			wire, _, _ := peer.send(child, nil)
			flat, _ := fidelityMetadata(t, wire)
			if flat["thread_id"] == flat["session_id"] || flat["root_turn_id"] != flat["turn_id"] {
				t.Fatal("self-rooted child turn lost its owning thread")
			}
			grandchild := fidelityBody(fidelityUser())
			grandchild["client_metadata"] = map[string]any{"session_id": "root-session", "thread_id": "grandchild-thread", "parent_thread_id": "child-thread", "turn_id": "grandchild-turn", "parent_turn_id": "child-root-turn", "root_turn_id": "child-root-turn"}
			wire, _, _ = peer.send(grandchild, nil)
			inherited, _ := fidelityMetadata(t, wire)
			if inherited["parent_turn_id"] != flat["turn_id"] || inherited["root_turn_id"] != flat["turn_id"] || inherited["parent_thread_id"] != flat["thread_id"] {
				t.Fatal("descendant did not preserve its child's root owner")
			}
		})
	}
}
