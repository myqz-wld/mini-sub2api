//go:build nativeparity

package integration

import (
	"context"
	"fmt"
	"path/filepath"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
)

func TestNativeScenarioIdentityDistributionKeyScope(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("subscription=%t/ws=%t/%s", subscription, ws, model), func(t *testing.T) {
					capture := newNativeIdentityCapture(t, "disabled")
					gateway := newNativeGateway(t, capture.server.URL, subscription)
					store, err := storage.Open(context.Background(), filepath.Dir(gateway.stateDir), time.Now)
					if err != nil {
						t.Fatal("open synthetic gateway key metadata")
					}
					credentials, err := store.Credentials(context.Background())
					if err != nil || len(credentials) != 1 {
						_ = store.Close()
						t.Fatal("same-credential scope fixture shape")
					}
					key := createDownstreamKey(t, store, credentials[0].ID, "Identity second distribution key")
					if store.Close() != nil {
						t.Fatal("close synthetic key metadata")
					}
					other := gateway
					other.secret = key.Secret
					for _, caller := range []nativeGateway{gateway, other, gateway} {
						for _, child := range []bool{false, true} {
							thread, turn, parent := identityRoot, identityRootTurn, ""
							if child {
								thread, turn, parent = identityChild, identityChildTurn, identityRoot
							}
							status, _ := identitySend(t, caller, ws, nil, identityRequest(model, identityRoot, thread, turn, parent, "", ""))
							if status != 200 {
								t.Fatal("scoped root/child request rejected")
							}
						}
					}
					wires := businessWires(capture.snapshot())
					if len(wires) != 6 {
						t.Fatal("scoped branch inference count")
					}
					for i := 0; i < 2; i++ {
						first, n1 := identityMetadata(t, wires[i])
						second, n2 := identityMetadata(t, wires[i+2])
						repeat, nr := identityMetadata(t, wires[i+4])
						for _, field := range []string{"session_id", "thread_id", "turn_id", "root_turn_id"} {
							if first[field] != repeat[field] || (first[field] == second[field]) == subscription {
								t.Fatalf("distribution-key identity scope/stability failed for %s", field)
							}
						}
						if i == 1 {
							if n1["parent_thread_id"] != nr["parent_thread_id"] || (n1["parent_thread_id"] == n2["parent_thread_id"]) == subscription {
								t.Fatal("parent lineage key scope/stability failed")
							}
							if first["x-codex-parent-thread-id"] != second["x-codex-parent-thread-id"] || first["x-codex-parent-thread-id"] != identityRoot {
								t.Fatal("G-A raw parent control changed")
							}
						}
					}
					t.Logf("same_credential_two_distribution_keys scoped_identity_separation=%t repeat_key_stable=true raw_flat_parent_shared=true", subscription)
				})
			}
		}
	}
}
