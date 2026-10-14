//go:build nativeparity

package integration

import (
	"fmt"
	"testing"
)

// Native 0.162.1 makes overload retryable only when a usable delay is attached.
// The actual CLI is the retry oracle; the gateway must not create extra sends.
func TestNative1620NestedRetryAdvice(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, advised := range []bool{false, true} {
			for _, route := range []string{"direct", "api-key", "subscription"} {
				t.Run(fmt.Sprintf("ws=%t/advised=%t/%s", ws, advised, route), func(t *testing.T) {
					capture := newNativeCapture(t)
					attempts := 0
					capture.respond = func(_ map[string]any, id string, prewarm bool) []map[string]any {
						if !prewarm {
							attempts++
							if attempts == 1 {
								failure := map[string]any{"code": "server_is_overloaded", "message": "synthetic private rejection"}
								if advised {
									failure["headers"] = map[string]any{"Retry-After": "0"}
								}
								return []map[string]any{{"type": "response.created", "response": map[string]any{"id": id}},
									{"type": "response.failed", "response": map[string]any{"id": id, "status": "failed", "error": failure}}}
							}
						}
						output := []any{}
						if !prewarm {
							output = append(output, map[string]any{"type": "message", "id": "msg_" + id, "role": "assistant", "phase": "final_answer", "content": []any{map[string]any{"type": "output_text", "text": "synthetic retry completed"}}})
						}
						events := []map[string]any{{"type": "response.created", "response": map[string]any{"id": id}}}
						for i, item := range output {
							events = append(events, map[string]any{"type": "response.output_item.done", "output_index": i, "item": item})
						}
						return append(events, map[string]any{"type": "response.completed", "response": map[string]any{"id": id, "output": output}})
					}
					options := nativeOptions{endpoint: capture.server.URL, model: "gpt-5.5", ws: ws, base: "synthetic retry probe", neutralPersonality: true,
						configOverrides: map[string]string{"model_providers.native_capture.stream_max_retries": "1"}}
					var gateway nativeGateway
					if route != "direct" {
						gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
						options.endpoint, options.bearer = gateway.server.URL, gateway.secret
					}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					turn := failureStartTurn(t, client, thread, "Reply once.")
					status := failureWaitTurn(t, client, thread, turn)
					wantStatus, wantAttempts := "failed", 1
					if advised {
						wantStatus, wantAttempts = "completed", 2
					}
					wires := capture.snapshot()
					if status != wantStatus || len(businessWires(wires)) != wantAttempts {
						t.Fatalf("native advice behavior: status=%s business=%d want=%s/%d", status, len(businessWires(wires)), wantStatus, wantAttempts)
					}
					if route != "direct" && len(gateway.tap.packets(t)) != len(wires) {
						t.Fatal("gateway added an unrequested retry")
					}
					t.Logf("native_retry_advice=%t business_attempts=%d terminal=%s", advised, wantAttempts, status)
				})
			}
		}
	}
}
