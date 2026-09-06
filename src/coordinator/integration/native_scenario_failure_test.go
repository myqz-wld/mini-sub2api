//go:build nativeparity

package integration

import (
	"bytes"
	"fmt"
	"net/http"
	"testing"
)

func failureNativeRoute(t *testing.T, capture *nativeFailureCapture, route, model string, ws bool) (*nativeClient, nativeOptions, nativeGateway) {
	t.Helper()
	options := nativeOptions{endpoint: capture.server.URL, model: model, ws: ws, base: "synthetic failure scenario base", neutralPersonality: true}
	var gateway nativeGateway
	if route != "direct" {
		gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
		options.endpoint, options.bearer = gateway.server.URL, gateway.secret
	}
	return startNativeClient(t, options), options, gateway
}

func TestNativeScenarioFailureTerminalRecovery(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, test := range []struct {
			kind  string
			ws    bool
			model string
		}{
			{"failed", false, "gpt-5.4"}, {"failed", true, "gpt-5.4"},
			{"failed", false, "gpt-5.6-sol"}, {"failed", true, "gpt-5.6-sol"},
			{"incomplete", false, "gpt-5.6-sol"}, {"incomplete", true, "gpt-5.4"},
			{"disconnect", false, "gpt-5.6-sol"}, {"disconnect", true, "gpt-5.4"},
		} {
			t.Run(fmt.Sprintf("%s/%s/ws=%t/%s", route, test.kind, test.ws, test.model), func(t *testing.T) {
				failedAttempts := 1
				if test.ws && test.kind != "failed" {
					failedAttempts = 2 // Native itself retries a retryable WS stream failure using HTTP.
				}
				capture := newNativeFailureCapture(t, test.kind, failedAttempts, 0)
				client, options, gateway := failureNativeRoute(t, capture, route, test.model, test.ws)
				thread := client.thread(options)
				first := failureStartTurn(t, client, thread, "synthetic failed first turn")
				if status := failureWaitTurn(t, client, thread, first); status != "failed" {
					t.Fatalf("native failure terminal status=%s", status)
				}
				before := businessWires(capture.snapshot())
				if len(before) != failedAttempts {
					t.Fatalf("failed inference attempts=%d want=%d", len(before), failedAttempts)
				}
				second := failureStartTurn(t, client, thread, "synthetic healthy next turn")
				if status := failureWaitTurn(t, client, thread, second); status != "completed" {
					t.Fatalf("native recovery terminal status=%s", status)
				}
				wires := businessWires(capture.snapshot())
				if len(wires) != failedAttempts+1 {
					t.Fatal("gateway replayed or dropped failure/recovery inference")
				}
				initial := transportMetadata(t, wires[0].value)
				recovered := transportMetadata(t, wires[len(wires)-1].value)
				if first == second || initial["turn_id"] == recovered["turn_id"] || initial["session_id"] != recovered["session_id"] || initial["thread_id"] != recovered["thread_id"] {
					t.Fatal("native failure changed session identity or reused failed turn")
				}
				for i, wire := range wires {
					if i > 0 && wire.value["previous_response_id"] != nil {
						t.Fatal("failed response became a reusable native baseline")
					}
					if failedAttempts == 2 && i > 0 && wire.method != http.MethodPost {
						t.Fatal("native-selected fallback did not remain HTTP")
					}
				}
				last := wires[len(wires)-1]
				partialCount := failureItemCount(last.value, "msg_failure_1")
				if partialCount != 1 {
					t.Fatalf("native completed-item partial history count=%d want=1", partialCount)
				}
				failureAssertMediation(t, route, gateway, capture)
				t.Logf("terminal=%s native_failed_attempts=%d gateway_extra_attempts=0 next_turn_completed=true partial_done_item_retained_once=true", test.kind, failedAttempts)
			})
		}
	}
}

func TestNativeScenarioFailureInterruptRecovery(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, ws := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/ws=%t", route, ws), func(t *testing.T) {
				model := "gpt-5.4"
				if ws {
					model = "gpt-5.6-sol"
				}
				capture := newNativeFailureCapture(t, "interrupt", 0, 1)
				client, options, gateway := failureNativeRoute(t, capture, route, model, ws)
				thread := client.thread(options)
				first := failureStartTurn(t, client, thread, "synthetic held turn")
				capture.waitStarted(t)
				client.call("turn/interrupt", map[string]any{"threadId": thread, "turnId": first})
				if status := failureWaitTurn(t, client, thread, first); status != "interrupted" {
					t.Fatalf("native interrupt terminal status=%s", status)
				}
				capture.unblock()
				second := failureStartTurn(t, client, thread, "synthetic next turn after interrupt")
				if status := failureWaitTurn(t, client, thread, second); status != "completed" {
					t.Fatalf("native post-interrupt terminal status=%s", status)
				}
				wires := businessWires(capture.snapshot())
				if len(wires) != 2 || wires[1].value["previous_response_id"] != nil {
					t.Fatal("interrupted response was replayed or became continuation baseline")
				}
				before, after := transportMetadata(t, wires[0].value), transportMetadata(t, wires[1].value)
				if before["session_id"] != after["session_id"] || before["turn_id"] == after["turn_id"] {
					t.Fatal("native interrupt/recovery identity boundary changed")
				}
				failureAssertMediation(t, route, gateway, capture)
				t.Log("explicit native interrupt: next turn completed, no failed baseline, no extra upstream inference")
			})
		}
	}
}

func TestNativeScenarioFailureIndependentNativeThreads(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, ws := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/ws=%t", route, ws), func(t *testing.T) {
				model := "gpt-5.4"
				if ws {
					model = "gpt-5.6-sol"
				}
				capture := newNativeFailureCapture(t, "overlap", 0, 2)
				client, options, gateway := failureNativeRoute(t, capture, route, model, ws)
				threads := []string{client.thread(options), client.thread(options)}
				turns := []string{failureStartTurn(t, client, threads[0], "synthetic identical independent work"), ""}
				capture.waitStarted(t)
				turns[1] = failureStartTurn(t, client, threads[1], "synthetic identical independent work")
				capture.waitStarted(t) // Both business responses are incomplete at this barrier.
				if len(businessWires(capture.snapshot())) != 2 {
					t.Fatal("independent native threads were serialized or duplicate work appeared")
				}
				capture.unblock()
				for i := range threads {
					if status := failureWaitTurn(t, client, threads[i], turns[i]); status != "completed" {
						t.Fatalf("independent native thread status=%s", status)
					}
				}
				wires := businessWires(capture.snapshot())
				first, second := transportMetadata(t, wires[0].value), transportMetadata(t, wires[1].value)
				for _, key := range []string{"session_id", "thread_id", "turn_id"} {
					if first[key] == second[key] || first[key] == nil || second[key] == nil {
						t.Fatalf("independent native identity collapsed at %s", key)
					}
				}
				failureAssertMediation(t, route, gateway, capture, threads...)
				t.Log("independent native threads overlapped at upstream, completed separately, and retained separate identities")
			})
		}
	}
}

func failureItemCount(value map[string]any, id string) int {
	count := 0
	input, _ := value["input"].([]any)
	for _, raw := range input {
		item, _ := raw.(map[string]any)
		if item["id"] == id {
			count++
		}
	}
	return count
}

func failureAssertMediation(t *testing.T, route string, gateway nativeGateway, capture *nativeFailureCapture, causalThreads ...string) {
	t.Helper()
	if route == "direct" {
		return
	}
	wires := capture.snapshot()
	packets := gateway.tap.packets(t)
	if len(wires) != len(packets) {
		t.Fatalf("failure mediation attempt count in=%d out=%d", len(packets), len(wires))
	}
	if len(causalThreads) > 0 {
		packets = failureAlignCausalThreads(t, packets, wires, causalThreads)
	}
	projection := &transportProjection{identities: map[string]map[string]string{}}
	for i, wire := range wires {
		if route == "api-key" && !bytes.Equal(packets[i].payload, wire.encodedBody) {
			t.Fatal("API-key failure/recovery bytes changed")
		}
		if packets[i].method != wire.method {
			t.Fatal("gateway converted failure/recovery transport")
		}
		if route == "subscription" {
			projection.compare(t, transportPacketValue(t, packets[i]), wire.value, fmt.Sprintf("request[%d]", i))
		}
	}
	if route == "api-key" {
		assertProfileStateFileCount(t, gateway.stateDir, 0)
	} else {
		t.Logf("typed failure request comparison: known projection differences=%v", projection.known)
	}
}

// The tap enumerates physical connections; the server records cross-connection
// arrival order. Two asynchronous native startup prewarms can therefore appear in
// different positions. The caller's barriers prove business i belongs to native
// thread i. Use that independent fact, never payload similarity, to align threads;
// retain every packet and its order within that thread, including all prewarms.
func failureAlignCausalThreads(t *testing.T, packets []nativePacket, wires []nativeWire, threads []string) []nativePacket {
	t.Helper()
	business := businessWires(wires)
	if len(business) != len(threads) {
		t.Fatal("concurrent causal thread count differs from business barriers")
	}
	origins := map[string]string{}
	queues := map[string][]nativePacket{}
	for i, wire := range business {
		projected, ok := transportMetadata(t, wire.value)["thread_id"].(string)
		if !ok || projected == "" || origins[projected] != "" || threads[i] == "" {
			t.Fatal("concurrent causal ownership is missing or collapsed")
		}
		origins[projected] = threads[i]
		queues[threads[i]] = nil
	}
	for _, packet := range packets {
		value := transportPacketValue(t, packet)
		thread, _ := transportMetadata(t, value)["thread_id"].(string)
		queue, exists := queues[thread]
		if !exists {
			t.Fatal("captured native packet belongs to an unstarted thread")
		}
		queues[thread] = append(queue, packet)
	}
	aligned := make([]nativePacket, 0, len(wires))
	for _, wire := range wires {
		projected, _ := transportMetadata(t, wire.value)["thread_id"].(string)
		original := origins[projected]
		queue := queues[original]
		if original == "" || len(queue) == 0 {
			t.Fatal("upstream thread received an extra or unowned packet")
		}
		aligned = append(aligned, queue[0])
		queues[original] = queue[1:]
	}
	for _, queue := range queues {
		if len(queue) != 0 {
			t.Fatal("native thread packet was not delivered upstream")
		}
	}
	return aligned
}
