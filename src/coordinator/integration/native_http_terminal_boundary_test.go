//go:build nativeparity

package integration

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestNativeHTTPStreamTerminationAndLargeUsage(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
			for _, stream := range []bool{false, true} {
				for _, scenario := range []string{"empty", "delta", "done-only", "completed", "failed", "incomplete", "large-completed", "large-failed"} {
					t.Run(fmt.Sprintf("subscription=%t/%s/stream=%t/%s", subscription, model, stream, scenario), func(t *testing.T) {
						body, terminal := terminalBoundaryBody(t, scenario)
						upstream, tap := newNativeTappedServer(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
							if r.Method != http.MethodPost {
								t.Error("HTTP boundary changed transport")
								return
							}
							_, _ = io.Copy(io.Discard, io.LimitReader(r.Body, nativeCaptureLimit))
							w.Header().Set("Content-Type", "text/event-stream")
							_, _ = w.Write(body)
						}))
						gateway := newNativeGateway(t, upstream.URL, subscription)
						requestBody := mustRequestJSON(t, map[string]any{"model": model, "stream": stream, "input": "synthetic terminal probe"})
						ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
						defer cancel()
						req, _ := http.NewRequestWithContext(ctx, http.MethodPost, gateway.server.URL+"/v1/responses", bytes.NewReader(requestBody))
						req.Header.Set("Authorization", "Bearer "+gateway.secret)
						req.Header.Set("Content-Type", "application/json")
						response, err := gateway.server.Client().Do(req)
						if err != nil {
							t.Fatal("terminal boundary delivery failed")
						}
						defer response.Body.Close()
						actual, err := io.ReadAll(io.LimitReader(response.Body, 32<<20))
						if err != nil {
							t.Fatal("terminal boundary body could not be read")
						}
						status := http.StatusOK
						if subscription && !stream && !terminal {
							status = http.StatusBadGateway
						}
						if response.StatusCode != status {
							t.Fatal("terminal boundary HTTP status differs")
						}
						if !subscription && !bytes.Equal(actual, body) {
							t.Fatal("API-key response bytes changed")
						}
						if (stream || !subscription) && !terminal {
							if response.Trailer.Get(protocolv1.FailurePhaseTrailer) != "upstream_stream" ||
								response.Trailer.Get(protocolv1.DeliveryStateTrailer) != "delivered" ||
								response.Trailer.Get(protocolv1.RetryAdviceTrailer) != "never" {
								t.Fatal("premature EOF failure metadata absent")
							}
							if scenario == "delta" && !bytes.Contains(actual, []byte("synthetic delta")) {
								t.Fatal("partial stream output disappeared")
							}
						}
						packets := tap.packets(t)
						if len(packets) != 1 {
							t.Fatal("terminal failure replayed an upstream request")
						}
						if !subscription && !bytes.Equal(packets[0].payload, requestBody) {
							t.Fatal("API-key request bytes changed")
						}
						success := strings.HasSuffix(scenario, "completed")
						for deadline := time.Now().Add(3 * time.Second); ; {
							stats, err := gateway.store.Stats(context.Background(), gateway.keyID, "", "")
							if err != nil {
								t.Fatal("terminal usage query failed")
							}
							if len(stats) == 1 {
								if (stats[0].CompletedCount == 1) != success || (stats[0].ErrorCount == 1) == success {
									t.Fatal("terminal outcome was miscounted")
								}
								if terminal && (stats[0].Usage == nil || stats[0].Usage.TotalTokens != 17) {
									t.Fatal("large terminal usage was lost")
								}
								break
							}
							if time.Now().After(deadline) {
								t.Fatal("terminal accounting did not finish")
							}
							time.Sleep(10 * time.Millisecond)
						}
					})
				}
			}
		}
	}
}

func terminalBoundaryBody(t *testing.T, scenario string) ([]byte, bool) {
	t.Helper()
	switch scenario {
	case "empty":
		return nil, false
	case "delta":
		return []byte("data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic delta\"}"), false
	case "done-only":
		return []byte(": keepalive\n\ndata: [DONE]\n\n"), false
	}
	status := strings.TrimPrefix(scenario, "large-")
	text := "synthetic output"
	if strings.HasPrefix(scenario, "large-") {
		text = strings.Repeat("x", 9<<20)
	}
	value := map[string]any{"type": "response." + status, "response": map[string]any{
		"id": "resp_boundary", "status": status, "output": []any{map[string]any{"type": "message", "id": "msg_boundary", "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": text}}}}, "usage": map[string]any{"total_tokens": 17},
	}}
	return append(append([]byte("data: "), mustRequestJSON(t, value)...), []byte("\r\n\r\n")...), true
}
