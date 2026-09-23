//go:build nativeparity

package integration

import (
	"fmt"
	"net/http"
	"reflect"
	"testing"
)

// Built-in OpenAI defaults matter: custom providers omit Version and backend-only metadata.
// A separate loopback metadata mock answers account discovery; the explicit inference override
// stays on the tapped gateway. Rejecting WS makes the native client select its HTTP fallback.
func TestNative1560WireShapeCapture(t *testing.T) {
	t.Setenv("TERM_PROGRAM", "native-parity")
	t.Setenv("TERM_PROGRAM_VERSION", "1")
	t.Setenv("TERM", "dumb")
	for _, ws := range []bool{false, true} {
		for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, model), func(t *testing.T) {
				var previousOrder []string
				for repeat := 0; repeat < 2; repeat++ {
					capture := newNativeCapture(t)
					capture.mu.Lock()
					capture.httpRoutingToken = repeat == 1
					capture.longRoutingToken = repeat == 1
					capture.mu.Unlock()
					gateway := newNativeGatewayForWireShape(t, capture.server.URL, true, !ws)
					options := nativeOptions{endpoint: gateway.server.URL, metadataEndpoint: capture.server.URL, bearer: gateway.secret, model: model, ws: ws, builtinOpenAI: true, base: "Synthetic ordered wire probe"}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					client.turn(thread, "First synthetic ordered request")
					client.turn(thread, "Second synthetic ordered request")
					in, out := gateway.tap.packets(t), capture.tap.packets(t)
					want, method := 3, http.MethodPost
					if ws {
						want, method = 4, http.MethodGet
					}
					if len(in) != want || len(out) != want {
						t.Fatal("wire shape capture missed a request or added inference")
					}
					wires := capture.snapshot()
					for i := range in {
						if in[i].method != method || out[i].method != method {
							t.Fatal("wire shape capture used the wrong transport")
						}
						left := readNativeJSONShape(t, nativePacketJSON(t, in[i]))
						metadata := left.fields["client_metadata"]
						if in[i].headers.Get("Version") != "0.156.0" || metadata.fields["guardian_credits_requested"] == nil {
							t.Fatal("built-in provider capture lacks its native defaults")
						}
						if i == 0 {
							if repeat > 0 {
								t.Logf("native client_metadata order varied between processes=%t", !reflect.DeepEqual(previousOrder, metadata.keys))
							}
							previousOrder = metadata.keys
						}
						assertNativeMessageParity(t, in[i], wires[i])
						assertNativeWireParity(t, in[i], out[i], wires[i], true)
						if in[i].headers.Get("X-Codex-Turn-State") != out[i].headers.Get("X-Codex-Turn-State") {
							t.Error("native routing token source/first-value ownership differs")
						}
						if !reflect.DeepEqual(in[i].headerSpellings, out[i].headerSpellings) {
							t.Errorf("native wire header order, presence or spelling differs: in=%v out=%v", in[i].headerSpellings, out[i].headerSpellings)
						}
						if metadata := in[i].headers.Get("X-Codex-Turn-Metadata"); metadata != "" {
							assertNativeJSONShape(t, []byte(metadata), []byte(out[i].headers.Get("X-Codex-Turn-Metadata")))
						}
					}
					t.Logf("strict wire shape compared repeat=%d requests=%d", repeat, want)
				}
			})
		}
	}
}
