//go:build nativeparity

package integration

import "testing"

// Public app-server turn/start is start-or-steer, not an unconditional task replace.
// These cases characterize the native public API before comparing gateway admission.
func TestNativeScenarioFailureSameThreadScheduling(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		t.Run("native/"+route, func(t *testing.T) {
			capture := newNativeFailureCapture(t, "overlap", 0, 1)
			client, options, gateway := failureNativeRoute(t, capture, route, "gpt-5.4", false)
			thread := client.thread(options)
			first := failureStartTurn(t, client, thread, "synthetic first same-thread submission")
			capture.waitStarted(t)
			second := failureStartTurn(t, client, thread, "synthetic second same-thread submission")
			if second != first || len(businessWires(capture.snapshot())) != 1 {
				t.Fatal("native public turn/start no longer steers the active thread turn")
			}
			capture.unblock()
			if status := failureWaitTurn(t, client, thread, first); status != "completed" {
				t.Fatalf("steered native turn status=%s", status)
			}
			wires := businessWires(capture.snapshot())
			if len(wires) != 2 {
				t.Fatalf("steered native sampling request count=%d", len(wires))
			}
			before, after := transportMetadata(t, wires[0].value), transportMetadata(t, wires[1].value)
			for _, key := range []string{"session_id", "thread_id", "turn_id"} {
				if before[key] == nil || before[key] != after[key] {
					t.Fatalf("steered native work changed its %s", key)
				}
			}
			if wires[1].value["previous_response_id"] != nil || failureItemCount(wires[1].value, "msg_failure_1") != 1 {
				t.Fatal("steered native HTTP request lost prior completed output history")
			}
			failureAssertMediation(t, route, gateway, capture)
			t.Log("public native same-thread turn/start steers existing turn; two serial inferences share one turn ID")
		})
	}
	for _, subscription := range []bool{false, true} {
		route := "api-key"
		if subscription {
			route = "subscription"
		}
		t.Run("ordinary/"+route, func(t *testing.T) {
			capture := newNativeFailureCapture(t, "overlap", 0, 2)
			gateway := newNativeGateway(t, capture.server.URL, subscription)
			results := []chan failureOrdinaryResult{make(chan failureOrdinaryResult, 1), make(chan failureOrdinaryResult, 1)}
			for i, turn := range []string{"same-thread-turn-a", "same-thread-turn-b"} {
				client := ordinaryClient(t, gateway, false, false, nil)
				request := failureRequest("gpt-5.4", "shared-explicit-session")
				metadata := request["client_metadata"].(map[string]any)
				metadata["thread_id"], metadata["turn_id"] = "shared-explicit-thread", turn
				go func() { results[i] <- failureOrdinarySend(client, request) }()
				capture.waitStarted(t)
			}
			wires := businessWires(capture.snapshot())
			if len(wires) != 2 {
				t.Fatal("ordinary same-thread distinct turns did not overlap before completion")
			}
			before, after := transportMetadata(t, wires[0].value), transportMetadata(t, wires[1].value)
			if before["session_id"] != after["session_id"] || before["thread_id"] != after["thread_id"] || before["turn_id"] == after["turn_id"] {
				t.Fatal("ordinary same-thread distinct-turn identity relationship changed")
			}
			capture.unblock()
			completed := []failureOrdinaryResult{failureAwaitOrdinary(t, results[0]), failureAwaitOrdinary(t, results[1])}
			for _, result := range completed {
				if result.err != "" || result.terminal != "response.completed" || result.response["id"] == nil {
					t.Fatal("ordinary same-thread concurrent turn did not complete independently")
				}
			}
			if completed[0].response["id"] == completed[1].response["id"] {
				t.Fatal("ordinary concurrent distinct turns received one response identity")
			}
			t.Log("MEASURED scheduling distinction: ordinary same-thread explicit distinct turns overlap; native public turn/start steers one active turn; no response ownership failure demonstrated")
		})
	}
}
