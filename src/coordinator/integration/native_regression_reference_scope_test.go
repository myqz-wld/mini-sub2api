//go:build nativeparity

package integration

import (
	"fmt"
	"testing"
)

func TestNativeReferenceHistoryScope(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
			for _, mode := range []string{"ancestor_full", "ancestor_reference", "fork_full", "fork_reference", "cross_session", "cross_key", "sibling_full"} {
				t.Run(fmt.Sprintf("ws=%t/%s/%s", ws, model, mode), func(t *testing.T) {
					capture := newNativeIdentityCapture(t, "disabled")
					gateway := newNativeGateway(t, capture.server.URL, true)
					client := ordinaryClient(t, gateway, ws, false, nil)
					seedBody := identityRequest(model, identityRoot, identityRoot, identityRootTurn, "", "", "")
					if mode == "sibling_full" {
						seedBody = identityRequest(model, identityRoot, identityChild, identityChildTurn, identityRoot, "", "")
					}
					seed := failureOrdinarySend(client, seedBody)
					if seed.err != "" || seed.terminal != "response.completed" || seed.response["id"] == nil {
						t.Fatal("reference scope control seed failed")
					}
					var request map[string]any
					allow := false
					switch mode {
					case "ancestor_full", "ancestor_reference":
						request = identityRequest(model, identityRoot, identityChild, identityChildTurn, identityRoot, "", "")
						if mode == "ancestor_full" {
							request = identityRequest(model, identityRoot, identityChild, identityChildTurn, identityRoot, "", identityRootTurn)
						} else {
							request["previous_response_id"] = seed.response["id"]
						}
						allow = true
					case "fork_full", "fork_reference":
						// Independent root forks use a distinct socket as well as session.
						client = ordinaryClient(t, gateway, ws, false, nil)
						request = identityRequest(model, identityFork, identityFork, identityForkTurn, "", identityRoot, "")
						if mode == "fork_full" {
							request = identityRequest(model, identityFork, identityFork, identityForkTurn, "", identityRoot, identityRootTurn)
							allow = true
						} else {
							request["previous_response_id"] = seed.response["id"]
						}
					case "cross_session":
						client = ordinaryClient(t, gateway, ws, false, nil)
						request = identityRequest(model, "regression-other-session", "regression-other-session", "regression-other-turn", "", "", "")
						request["previous_response_id"] = seed.response["id"]
					case "cross_key":
						client = ordinaryClient(t, failureSecondKey(t, gateway), ws, false, nil)
						request = identityRequest(model, identityRoot, identityRoot, "regression-other-turn", "", "", "")
						request["previous_response_id"] = seed.response["id"]
					case "sibling_full":
						request = identityRequest(model, identityRoot, "regression-sibling", "regression-sibling-turn", identityRoot, "", identityChildTurn)
					}
					before := len(businessWires(capture.snapshot()))
					result := failureOrdinarySend(client, request)
					count := len(businessWires(capture.snapshot())) - before
					if result.err != "" || (result.terminal == "response.completed") != allow || count != map[bool]int{true: 1, false: 0}[allow] {
						t.Fatalf("reference scope control differs: mode=%s expected_completed=%t observed_completed=%t upstream_delta=%d", mode, allow, result.terminal == "response.completed", count)
					}
				})
			}
		}
	}
}
