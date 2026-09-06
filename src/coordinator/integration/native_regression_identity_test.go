//go:build nativeparity

package integration

import (
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

// Source-backed synthetic challenges, not actual persisted-native fork captures.
// The existing bounded mock owns payloads; no capture data is saved.
func TestNativeLateHistoryOwnership(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
			for _, ids := range []bool{false, true} {
				for _, owner := range []string{"source_child", "unrelated_root"} {
					t.Run(fmt.Sprintf("ws=%t/%s/ids=%t/%s", ws, model, ids, owner), func(t *testing.T) {
						capture := newNativeIdentityCapture(t, "disabled")
						gateway := newNativeGateway(t, capture.server.URL, true)
						forkBody := func() map[string]any {
							body := identityRequest(model, identityFork, identityFork, identityForkTurn, "", identityChild, identityChildTurn)
							if !ids {
								for _, raw := range body["input"].([]any) {
									delete(raw.(map[string]any), "id")
								}
							}
							return body
						}
						if status, _ := identitySend(t, gateway, ws, nil, forkBody()); status != 200 {
							t.Fatal("unknown history seed rejected")
						}
						seed := businessWires(capture.snapshot())[0]
						_, seedMetadata := identityMetadata(t, seed)
						sourceAlias := seedMetadata["forked_from_thread_id"]
						historical := historicalBoundaryItem(t, seed, ids)
						turnAlias := historical["internal_chat_message_metadata_passthrough"].(map[string]any)["turn_id"]
						if owner == "source_child" {
							if status, _ := identitySend(t, gateway, ws, nil, identityRequest(model, identityRoot, identityRoot, identityRootTurn, "", "", "")); status != 200 {
								t.Fatal("late source parent rejected")
							}
						}
						session, thread, parent := "regression-unrelated", "regression-unrelated", ""
						if owner == "source_child" {
							session, thread, parent = identityRoot, identityChild, identityRoot
						}
						if status, _ := identitySend(t, gateway, ws, nil, identityRequest(model, session, thread, identityChildTurn, parent, "", "")); status != 200 {
							t.Fatal("unknown historical turn could not acquire actual ownership")
						}
						wires := businessWires(capture.snapshot())
						actual, actualNested := identityMetadata(t, wires[len(wires)-1])
						if actual["turn_id"] != turnAlias || (owner == "source_child" && actual["thread_id"] != sourceAlias) {
							t.Fatal("reserved history/source alias changed at actual ownership")
						}
						if (actualNested["parent_thread_id"] != nil) != (owner == "source_child") {
							t.Fatal("late ownership branch shape changed")
						}
						before := len(wires)
						stateBefore := historicalBoundaryStateHashes(t, gateway.stateDir)
						status, _ := identitySend(t, gateway, ws, nil, forkBody())
						wires = businessWires(capture.snapshot())
						if owner == "unrelated_root" {
							if status == 200 || len(wires) != before || !reflect.DeepEqual(stateBefore, historicalBoundaryStateHashes(t, gateway.stateDir)) {
								t.Fatal("late conflicting history owner was accepted or changed durable identity state")
							}
						} else {
							if status != 200 || len(wires) != before+1 || !reflect.DeepEqual(historical, historicalBoundaryItem(t, wires[len(wires)-1], ids)) {
								t.Fatal("late source child lost eligible history or stable item metadata")
							}
						}
					})
				}
			}
		}
	}
}

func historicalBoundaryItem(t *testing.T, wire nativeWire, ids bool) map[string]any {
	t.Helper()
	for _, raw := range wire.value["input"].([]any) {
		item, _ := raw.(map[string]any)
		if item["role"] == "user" {
			if _, present := item["id"]; present != ids {
				t.Fatal("historical item ID presence changed")
			}
			return item
		}
	}
	t.Fatal("historical user item missing")
	return nil
}

func historicalBoundaryStateHashes(t *testing.T, stateDir string) map[string][32]byte {
	t.Helper()
	paths, err := filepath.Glob(filepath.Join(stateDir, "accounts", "*.request-state.json"))
	if err != nil || len(paths) != 1 {
		t.Fatal("durable identity state cardinality differs")
	}
	hashes := map[string][32]byte{}
	for _, path := range paths {
		data, err := os.ReadFile(path)
		if err != nil || !json.Valid(data) {
			t.Fatal("durable identity state unavailable")
		}
		hashes[filepath.Base(path)] = sha256.Sum256(data)
	}
	return hashes
}

func TestNativePreviousBranchHistory(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, model), func(t *testing.T) {
				capture := newNativeIdentityCapture(t, "disabled")
				gateway := newNativeGateway(t, capture.server.URL, true)
				client := ordinaryClient(t, gateway, ws, false, nil)
				seed := failureOrdinarySend(client, identityRequest(model, identityRoot, identityChild, identityChildTurn, identityRoot, "", ""))
				if seed.err != "" || seed.terminal != "response.completed" || seed.response["id"] == nil {
					t.Fatal("branch authority seed failed")
				}
				owned := identityRequest(model, identityRoot, identityChild, "regression-own-followup", identityRoot, "", "")
				owned["previous_response_id"] = seed.response["id"]
				positive := failureOrdinarySend(client, owned)
				if positive.err != "" || positive.terminal != "response.completed" {
					t.Fatal("same-branch explicit previous positive control failed")
				}
				before := len(businessWires(capture.snapshot()))
				foreign := identityRequest(model, identityRoot, "regression-sibling", "regression-sibling-turn", identityRoot, "", "")
				foreign["previous_response_id"] = seed.response["id"]
				negative := failureOrdinarySend(client, foreign)
				wires := businessWires(capture.snapshot())
				if negative.err != "" {
					t.Fatal("branch authority transport control failed")
				}
				all := capture.snapshot()
				packets := capture.tap.packets(t)
				if len(packets) != len(all) {
					t.Fatal("branch authority raw tap request count differs")
				}
				for i, packet := range packets {
					if !reflect.DeepEqual(nativeScenarioPacketValue(t, packet), all[i].value) {
						t.Fatal("branch authority raw packet differs from decoded server request")
					}
				}
				if negative.terminal == "response.completed" || len(wires) != before {
					t.Errorf("unrelated previous history admitted: ws=%t completed=%t upstream_delta=%d", ws, negative.terminal == "response.completed", len(wires)-before)
				}
			})
		}
	}
}
