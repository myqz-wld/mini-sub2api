//go:build nativeparity

package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"testing"

	"github.com/coder/websocket"
	"github.com/klauspost/compress/zstd"
)

// An independent source-derived boundary: native compact_remote_v2 counts only
// OutputItemDone. ResponseCompleted carries no output items to that collector.
// Only bounded in-memory captures are used; no payloads are saved.
func TestNativeCompactionRequiresItemDone(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, ws := range []bool{false, true} {
			for _, finalOnly := range []bool{false, true} {
				t.Run(fmt.Sprintf("%s/ws=%t/final_only=%t", route, ws, finalOnly), func(t *testing.T) {
					capture := boundaryCompactionCapture(t, finalOnly)
					options := nativeOptions{endpoint: capture.server.URL, model: "gpt-5.6-sol", ws: ws,
						base: "synthetic independent compaction base", neutralPersonality: true,
						configOverrides: map[string]string{"features.remote_compaction_v2": "true", "features.token_budget": "false"}}
					var gateway nativeGateway
					if route != "direct" {
						gateway = newNativeGateway(t, capture.server.URL, route == "subscription")
						options.endpoint, options.bearer = gateway.server.URL, gateway.secret
					}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					client.turn(thread, "synthetic compaction user seed")
					accepted, failed := runNativeManualCompaction(t, client, thread)
					if accepted == finalOnly || failed != finalOnly {
						t.Fatal("native compaction acceptance differs from source-derived boundary")
					}
					client.turn(thread, "synthetic postcompaction user")
					acceptedAgain, failedAgain := runNativeManualCompaction(t, client, thread)
					if acceptedAgain == finalOnly || failedAgain != finalOnly {
						t.Fatal("repeated native compaction acceptance differs")
					}
					all := capture.snapshot()
					business := businessWires(all)
					if len(business) != 4 {
						t.Fatal("independent boundary replayed or dropped a business inference")
					}
					callerBusiness := business
					if route != "direct" {
						packets := gateway.tap.packets(t)
						if len(packets) != len(all) {
							t.Fatal("independent boundary capture count changed")
						}
						callerBusiness = nil
						for i, packet := range packets {
							wire := compactionPacketWire(t, packet)
							if wire.value["generate"] != false {
								callerBusiness = append(callerBusiness, wire)
							}
							if route == "api-key" && !bytes.Equal(packet.payload, all[i].encodedBody) {
								t.Fatal("API-key independent boundary altered request bytes")
							}
						}
					}
					assertCompactionWindowLifecycle(t, callerBusiness, !finalOnly)
					callerWindow := compactionMetadata(t, callerBusiness[3])["window_number"]
					upstreamWindow := compactionMetadata(t, business[3])["window_number"]
					if finalOnly {
						_, texts := compactionItemCounts(callerBusiness[2])
						if texts["synthetic compaction user seed"] != 1 || texts["synthetic precompaction assistant"] != 1 {
							t.Fatal("native rejected compaction did not preserve original history")
						}
					}
					if upstreamWindow != callerWindow {
						t.Fatal("rejected V2 final-only response advanced the gateway window")
					}
				})
			}
		}
	}
}

func boundaryCompactionCapture(t *testing.T, finalOnly bool) *nativeCompactionCapture {
	c := &nativeCompactionCapture{nativeCapture: &nativeCapture{t: t}, mode: "v2", outputs: map[string][]any{}}
	eventsFor := func(r *http.Request, body, raw []byte, connection int) []map[string]any {
		events, _ := c.recordCompaction(r, body, raw, connection)
		if finalOnly {
			filtered := make([]map[string]any, 0, len(events))
			for _, event := range events {
				item, _ := event["item"].(map[string]any)
				if event["type"] == "response.output_item.done" && item["type"] == "compaction" {
					continue
				}
				filtered = append(filtered, event)
			}
			events = filtered
		}
		return events
	}
	c.server, c.tap = newNativeTappedServer(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/backend-api/wham/settings/user" {
			w.Header().Set("Content-Type", "application/json")
			_, _ = io.WriteString(w, `{"commit_attribution_enabled":false}`)
			return
		}
		if r.URL.Path != "/v1/responses" {
			w.WriteHeader(http.StatusNotFound)
			return
		}
		if r.Method == http.MethodGet {
			conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{CompressionMode: websocket.CompressionContextTakeover})
			if err != nil {
				return
			}
			defer conn.CloseNow()
			conn.SetReadLimit(nativeCaptureLimit)
			c.nativeCapture.mu.Lock()
			c.connections++
			number := c.connections
			c.nativeCapture.mu.Unlock()
			for {
				kind, body, err := conn.Read(context.Background())
				if err != nil || kind != websocket.MessageText {
					return
				}
				for _, event := range eventsFor(r, body, body, number) {
					encoded, _ := json.Marshal(event)
					if conn.Write(context.Background(), websocket.MessageText, encoded) != nil {
						return
					}
				}
			}
		}
		if r.Method != http.MethodPost {
			w.WriteHeader(http.StatusMethodNotAllowed)
			return
		}
		raw, err := io.ReadAll(io.LimitReader(r.Body, nativeCaptureLimit+1))
		if err != nil || len(raw) > nativeCaptureLimit {
			w.WriteHeader(413)
			return
		}
		body := raw
		if r.Header.Get("Content-Encoding") == "zstd" {
			decoder, err := zstd.NewReader(nil, zstd.WithDecoderMaxMemory(nativeCaptureLimit), zstd.WithDecoderConcurrency(1))
			if err != nil {
				w.WriteHeader(400)
				return
			}
			defer decoder.Close()
			body, err = decoder.DecodeAll(raw, nil)
			if err != nil {
				w.WriteHeader(400)
				return
			}
		}
		w.Header().Set("Content-Type", "text/event-stream")
		for _, event := range eventsFor(r, body, raw, 0) {
			encoded, _ := json.Marshal(event)
			_, _ = fmt.Fprintf(w, "data: %s\n\n", encoded)
		}
	}))
	return c
}
