//go:build nativeparity

package integration

import (
	"fmt"
	"testing"
	"time"
)

func TestNativeScenarioFailureReferenceOwnership(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, delivery := range []string{"json", "sse", "ws"} {
			for _, conflict := range []string{"session", "key", "bound-ws"} {
				if conflict == "bound-ws" && delivery != "ws" {
					continue
				}
				t.Run(fmt.Sprintf("subscription=%t/%s/%s", subscription, delivery, conflict), func(t *testing.T) {
					capture := newNativeFailureCapture(t, "healthy", 0, 0)
					gateway := newNativeGateway(t, capture.server.URL, subscription)
					client := ordinaryClient(t, gateway, delivery == "ws", delivery != "json", nil)
					seed := failureOrdinarySend(client, failureRequest("gpt-5.4", "ownership-first"))
					if seed.err != "" || seed.terminal != "response.completed" || seed.response["id"] == nil {
						t.Fatal("ownership seed did not complete")
					}
					reference := seed.response["id"]
					// The very next call runs as soon as completed is delivered. It must see
					// the published response and its complete history before any later turn.
					immediate := failureRequest("gpt-5.4", "ownership-first")
					immediate["previous_response_id"] = reference
					continued := failureOrdinarySend(client, immediate)
					if continued.err != "" || continued.terminal != "response.completed" {
						t.Fatal("completed response was not immediately visible to its owner")
					}
					if conflict == "key" {
						other := failureSecondKey(t, gateway)
						client = ordinaryClient(t, other, delivery == "ws", delivery != "json", nil)
					} else if conflict == "bound-ws" {
						// Bind a separate live socket to session B, then try A's valid completed ID.
						client = ordinaryClient(t, gateway, true, true, nil)
						bound := failureOrdinarySend(client, failureRequest("gpt-5.4", "ownership-other"))
						if bound.terminal != "response.completed" {
							t.Fatal("ownership second socket failed to bind")
						}
					}
					request := failureRequest("gpt-5.4", "ownership-other")
					if conflict == "key" {
						request = failureRequest("gpt-5.4", "ownership-first")
					}
					request["previous_response_id"] = reference
					before := len(businessWires(capture.snapshot()))
					result := failureOrdinarySend(client, request)
					count := len(businessWires(capture.snapshot())) - before
					if result.err != "" || subscription && (count != 0 || result.terminal == "response.completed") || !subscription && (count != 1 || result.terminal != "response.completed") {
						t.Fatalf("reference ownership boundary changed: status=%d terminal=%s reached=%d", result.status, result.terminal, count)
					}
					t.Logf("immediate_owned_continuation=true reference_conflict=%s scoped_rejection=%t upstream_delta=%d", conflict, subscription, count)
				})
			}
		}
	}
}

func TestNativeScenarioFailureIndependentOrdinaryRequests(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, delivery := range []string{"json", "sse", "ws"} {
			for _, explicit := range []bool{false, true} {
				t.Run(fmt.Sprintf("subscription=%t/%s/explicit=%t", subscription, delivery, explicit), func(t *testing.T) {
					capture := newNativeFailureCapture(t, "overlap", 0, 2)
					gateway := newNativeGateway(t, capture.server.URL, subscription)
					clients := []nativeOrdinaryClient{ordinaryClient(t, gateway, delivery == "ws", delivery != "json", nil), ordinaryClient(t, gateway, delivery == "ws", delivery != "json", nil)}
					results := make(chan failureOrdinaryResult, 2)
					for i, client := range clients {
						session := ""
						if explicit {
							session = fmt.Sprintf("independent-ordinary-%d", i)
						}
						request := failureRequest("gpt-5.4", session)
						go func() { results <- failureOrdinarySend(client, request) }()
						capture.waitStarted(t)
					}
					if len(businessWires(capture.snapshot())) != 2 {
						t.Fatal("independent ordinary work did not overlap before completion")
					}
					capture.unblock()
					for i := 0; i < 2; i++ {
						result := failureAwaitOrdinary(t, results)
						if result.err != "" || result.terminal != "response.completed" {
							t.Fatalf("independent ordinary completion: terminal=%s control=%s", result.terminal, result.err)
						}
					}
					if subscription {
						wires := businessWires(capture.snapshot())
						first, second := transportMetadata(t, wires[0].value), transportMetadata(t, wires[1].value)
						for _, key := range []string{"session_id", "turn_id"} {
							if first[key] == second[key] || first[key] == nil || second[key] == nil {
								t.Fatalf("equivalent independent ordinary work collapsed %s", key)
							}
						}
					}
					t.Logf("overlapping equivalent ordinary inputs completed independently: explicit=%t", explicit)
				})
			}
		}
	}
}

func TestNativeScenarioFailureActiveLaneAdmission(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, ws := range []bool{false, true} {
			t.Run(fmt.Sprintf("subscription=%t/ws=%t", subscription, ws), func(t *testing.T) {
				capture := newNativeFailureCapture(t, "overlap", 0, 1)
				gateway := newNativeGateway(t, capture.server.URL, subscription)
				first := ordinaryClient(t, gateway, ws, true, nil)
				second := ordinaryClient(t, gateway, ws, true, nil)
				request := func() map[string]any {
					value := failureRequest("gpt-5.4", "same-active-session")
					value["client_metadata"].(map[string]any)["turn_id"] = "same-active-turn"
					return value
				}
				results := make(chan failureOrdinaryResult, 1)
				go func() { results <- failureOrdinarySend(first, request()) }()
				capture.waitStarted(t)
				contended := failureOrdinarySend(second, request())
				want := 2
				if subscription {
					want = 1
				}
				if contended.err != "" || len(businessWires(capture.snapshot())) != want || subscription == (contended.terminal == "response.completed") {
					t.Fatalf("active lane contention: status=%d terminal=%s", contended.status, contended.terminal)
				}
				capture.unblock()
				completed := failureAwaitOrdinary(t, results)
				if completed.terminal != "response.completed" {
					t.Fatal("rejected contender interrupted the owning active request")
				}
				t.Logf("same active lane protected=%t original_completed=true", subscription)
			})
		}
	}
}

func failureAwaitOrdinary(t *testing.T, results <-chan failureOrdinaryResult) failureOrdinaryResult {
	t.Helper()
	select {
	case result := <-results:
		return result
	case <-time.After(12 * time.Second):
		t.Fatal("ordinary concurrent result deadline")
		return failureOrdinaryResult{}
	}
}
