//go:build nativeparity

package integration

import (
	"bytes"
	"net/http"
	"sort"
	"strings"
	"testing"
)

func identityWireGroups(t *testing.T, wires []nativeWire) map[string][]nativeWire {
	t.Helper()
	groups := map[string][]nativeWire{}
	for _, wire := range wires {
		flat, _ := identityMetadata(t, wire)
		branch := "root"
		if flat["session_id"] != flat["thread_id"] {
			branch = "child"
		}
		groups[branch] = append(groups[branch], wire)
	}
	return groups
}

func identityPackets(t *testing.T, packets []nativePacket) []nativeWire {
	t.Helper()
	var wires []nativeWire
	for _, packet := range packets {
		wires = append(wires, nativeWire{method: packet.method, headers: packet.headers, encodedBody: packet.payload, value: nativeScenarioPacketValue(t, packet), connection: packet.connection})
	}
	return wires
}

func identityNativeGraph(t *testing.T, wires []nativeWire) string {
	t.Helper()
	groups := identityWireGroups(t, wires)
	rootWires, childWires := businessWires(groups["root"]), businessWires(groups["child"])
	if len(rootWires) != 3 || len(childWires) != 1 {
		t.Fatal("native spawn/wait lifecycle did not sample both branches")
	}
	root, rootNested := identityMetadata(t, rootWires[0])
	child, childNested := identityMetadata(t, childWires[0])
	rootID, _ := root["thread_id"].(string)
	childID, _ := child["thread_id"].(string)
	rootTurn, _ := root["turn_id"].(string)
	childTurn, _ := child["turn_id"].(string)
	installation, _ := root["x-codex-installation-id"].(string)
	if !isUUIDVersion(rootID, '7') || !isUUIDVersion(childID, '7') || rootID == childID || !isUUIDVersion(rootTurn, '7') || !isUUIDVersion(childTurn, '7') || rootTurn == childTurn || !isUUIDVersion(installation, '4') {
		t.Fatal("native graph identity generators or ownership differ")
	}
	if childNested["context_window_id"] == rootNested["context_window_id"] {
		t.Fatal("fresh child reused the parent context window")
	}
	for _, wire := range wires {
		flat, nested := identityMetadata(t, wire)
		thread, _ := flat["thread_id"].(string)
		if flat["session_id"] != rootID || nested["session_id"] != rootID || nested["thread_id"] != thread || flat["x-codex-installation-id"] != installation || nested["installation_id"] != installation {
			t.Fatal("native session/thread/installation carriers conflict")
		}
		window, _ := nested["context_window_id"].(string)
		if !isUUIDVersion(window, '7') || nested["window_number"] != float64(0) || nested["window_id"] != thread+":0" || flat["x-codex-window-id"] != thread+":0" {
			t.Fatal("native context window lifecycle differs")
		}
		if nested["forked_from_thread_id"] != nil || nested["forked_from_ordinal_exclusive"] != nil {
			t.Fatal("fresh spawned branch unexpectedly exposes fork provenance")
		}
		if wire.headers.Get("Session-Id") != rootID || wire.headers.Get("Thread-Id") != thread || wire.headers.Get("X-Client-Request-Id") != thread || wire.headers.Get("X-Codex-Window-Id") != thread+":0" {
			t.Fatal("native handshake/request branch headers conflict with frames")
		}
		if thread == childID {
			if nested["parent_thread_id"] != rootID || flat["x-codex-parent-thread-id"] != rootID || wire.headers.Get("X-Codex-Parent-Thread-Id") != rootID {
				t.Fatal("native parent carriers conflict")
			}
		} else if nested["parent_thread_id"] != nil || flat["x-codex-parent-thread-id"] != nil || wire.headers.Get("X-Codex-Parent-Thread-Id") != "" {
			t.Fatal("native root unexpectedly gained a parent")
		}
		if wire.value["generate"] == false {
			continue
		}
		turn := rootTurn
		if thread == childID {
			turn = childTurn
		}
		if flat["turn_id"] != turn || nested["turn_id"] != turn || flat["root_turn_id"] != rootTurn || nested["root_turn_id"] != rootTurn {
			t.Fatal("native causal turn identity changed within sampling loop")
		}
		if thread == childID && (flat["parent_turn_id"] != rootTurn || nested["parent_turn_id"] != rootTurn) {
			t.Fatal("native child lost initiating parent turn")
		}
		if thread == rootID && (flat["parent_turn_id"] != nil || nested["parent_turn_id"] != nil) {
			t.Fatal("native root unexpectedly gained causal parent")
		}
		input, _ := wire.value["input"].([]any)
		for _, raw := range input {
			item, _ := raw.(map[string]any)
			metadata, _ := item["internal_chat_message_metadata_passthrough"].(map[string]any)
			if historical, ok := metadata["turn_id"].(string); ok && historical != "" && historical != turn {
				t.Fatal("fresh native branch attributed an item to a foreign turn")
			}
		}
	}
	return childID
}

func identityHeaderFields(header http.Header) map[string]any {
	fields := map[string]any{}
	for key, field := range map[string]string{"Session-Id": "session_id", "Thread-Id": "thread_id", "X-Codex-Parent-Thread-Id": "parent_thread_id", "X-Codex-Window-Id": "window_id", "X-Codex-Turn-Metadata": "x-codex-turn-metadata"} {
		if value := header.Get(key); value != "" {
			fields[field] = value
		}
	}
	return fields
}

func identityCompareGateway(t *testing.T, before, after []nativeWire, subscription bool) {
	t.Helper()
	in, out := identityWireGroups(t, before), identityWireGroups(t, after)
	var relations nativeMetadataRelations
	differences := map[string]bool{}
	parentMismatches := 0
	for _, branch := range []string{"root", "child"} {
		if len(in[branch]) != len(out[branch]) {
			t.Fatal("gateway added or dropped native branch frames")
		}
		for index, original := range in[branch] {
			sent := out[branch][index]
			if !subscription && !bytes.Equal(original.encodedBody, sent.encodedBody) {
				t.Fatal("API-key native branch bytes changed")
			}
			flat, _ := identityMetadata(t, original)
			projected, nested := identityMetadata(t, sent)
			left := make(map[string]any, len(flat))
			for key, value := range flat {
				left[key] = value
			}
			right := make(map[string]any, len(projected))
			for key, value := range projected {
				right[key] = value
			}
			if branch == "child" && subscription {
				// G-A is explicitly asserted as nonconformance, while every remaining
				// parent placement must still follow the same scoped mapping.
				if projected["x-codex-parent-thread-id"] != flat["x-codex-parent-thread-id"] || projected["x-codex-parent-thread-id"] == nested["parent_thread_id"] {
					t.Fatal("G-A baseline changed; reassess native parent conformance")
				}
				if projected["parent_thread_id"] != nested["parent_thread_id"] {
					t.Fatal("added parent alias conflicts with nested/header lineage")
				}
				delete(left, "x-codex-parent-thread-id")
				delete(right, "x-codex-parent-thread-id")
				parentMismatches++
			}
			for _, difference := range relations.compare(t, "client_metadata", left, right) {
				differences[difference] = true
			}
			for _, difference := range relations.compare(t, "headers", identityHeaderFields(original.headers), identityHeaderFields(sent.headers)) {
				differences[difference] = true
			}
			if original.headers.Get("X-Client-Request-Id") != original.headers.Get("Thread-Id") || sent.headers.Get("X-Client-Request-Id") != sent.headers.Get("Thread-Id") {
				t.Fatal("gateway client request thread alias conflicts")
			}
			input, _ := original.value["input"].([]any)
			emitted, _ := sent.value["input"].([]any)
			if len(input) != len(emitted) {
				t.Fatal("native branch input length changed")
			}
			for i, raw := range input {
				item, _ := raw.(map[string]any)
				target, _ := emitted[i].(map[string]any)
				b, _ := item["internal_chat_message_metadata_passthrough"].(map[string]any)
				a, _ := target["internal_chat_message_metadata_passthrough"].(map[string]any)
				for _, difference := range relations.compare(t, "input.item_metadata", b, a) {
					differences[difference] = true
				}
			}
			if len(emitted) > 0 {
				if item, ok := emitted[0].(map[string]any); ok && item["type"] == "additional_tools" {
					assertNativeLiteIDs(t, sent)
				}
			}
		}
	}
	var paths []string
	for path := range differences {
		allowed := subscription && (nativeRootMetadataDifference(path) || path == "changed:headers.x-codex-turn-metadata.turn_started_at_unix_ms" || path == "added:client_metadata.parent_thread_id")
		if !allowed {
			t.Errorf("unclassified native branch metadata difference: %s", path)
		}
		paths = append(paths, path)
	}
	sort.Strings(paths)
	if subscription && parentMismatches == 0 {
		t.Fatal("native parent carrier discrepancy was not exercised")
	}
	t.Logf("typed_parent_mismatch_frames=%d metadata_relations=%d additional_metadata_differences=%s", parentMismatches, len(relations.forward), strings.Join(paths, ","))
}
