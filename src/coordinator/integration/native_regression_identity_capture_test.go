//go:build nativeparity

package integration

import (
	"bytes"
	"fmt"
	"reflect"
	"testing"
)

// Recheck the actual child producer with an independent upstream raw-to-decoded
// assertion. Per-branch order is valid here because exactly one child is sampled.
func TestNativeBranchRawCapture(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, model), func(t *testing.T) {
				capture := newNativeIdentityCapture(t, "none")
				gateway := newNativeGateway(t, capture.server.URL, true)
				options := nativeOptions{endpoint: gateway.server.URL, bearer: gateway.secret, model: model, ws: ws, configOverrides: map[string]string{"features.multi_agent_v2": "true"}}
				client := startNativeClient(t, options)
				root := client.thread(options)
				status, _ := identityNativeTurn(client, root)
				if status != "completed" {
					t.Fatal("actual native raw-oracle lifecycle failed")
				}
				incoming := identityPackets(t, gateway.tap.packets(t))
				identityNativeGraph(t, incoming)
				decoded := capture.snapshot()
				identityCompareGateway(t, incoming, decoded, true)
				rawGroups := identityWireGroups(t, identityPackets(t, capture.tap.packets(t)))
				decodedGroups := identityWireGroups(t, decoded)
				for _, branch := range []string{"root", "child"} {
					if len(rawGroups[branch]) != len(decodedGroups[branch]) {
						t.Fatal("raw identity branch capture cardinality differs")
					}
					for i, raw := range rawGroups[branch] {
						wire := decodedGroups[branch][i]
						if raw.method != wire.method || !bytes.Equal(raw.encodedBody, wire.encodedBody) || !reflect.DeepEqual(raw.value, wire.value) {
							t.Fatal("raw identity request/frame differs from independently decoded server capture")
						}
					}
				}
			})
		}
	}
}
