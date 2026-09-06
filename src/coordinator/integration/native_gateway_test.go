//go:build nativeparity

package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/httpapi"
	"mini-sub2api/src/coordinator/internal/storage"
)

type nativeGateway struct {
	server   *httptest.Server
	tap      *nativeTap
	secret   string
	stateDir string
}

func newNativeGateway(t *testing.T, upstream string, subscription bool) nativeGateway {
	t.Helper()
	assertLoopbackURL(t, upstream)
	binary := findCoreBinary(t)
	stateDir := t.TempDir()
	coreDir := filepath.Join(stateDir, "core-codex")
	var metadata coreMetadata
	if subscription {
		account := "native-loopback-account"
		authPath := filepath.Join(stateDir, "synthetic-auth.json")
		auth := mustRequestJSON(t, map[string]any{"auth_mode": "chatgpt", "tokens": map[string]string{"id_token": testJWT(&account, 3600), "access_token": testJWT(nil, 3600), "refresh_token": "synthetic-not-imported", "account_id": account}})
		if os.WriteFile(authPath, auth, 0600) != nil {
			t.Fatal("write synthetic gateway auth")
		}
		metadata = createCoreCredential(t, binary, []string{"credential", "import-codex-auth", "--state-dir", coreDir, "--auth-file", authPath, "--issuer", upstream, "--client-id", "native-loopback-client", "--upstream-url", upstream + "/v1/responses"}, "")
	} else {
		metadata = createCoreCredential(t, binary, []string{"credential", "add-api-key", "--state-dir", coreDir, "--upstream-url", upstream + "/v1/responses", "--secret-stdin"}, upstreamAPIKey+"\n")
	}
	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal("open gateway fixture")
	}
	t.Cleanup(func() { _ = store.Close() })
	credential := persistCredential(t, store, "Native capture", metadata)
	key := createDownstreamKey(t, store, credential.ID, "Native capture caller")
	supervisor, err := adapter.Start(context.Background(), adapter.Config{Binary: binary, StateDir: coreDir})
	if err != nil {
		t.Fatal("start test Core")
	}
	t.Cleanup(func() { _ = supervisor.Close() })
	handler := httpapi.NewHandler(store, supervisor, nil)
	server, tap := newNativeTappedServer(t, handler)
	t.Cleanup(handler.ShutdownWebSockets)
	return nativeGateway{server: server, tap: tap, secret: key.Secret, stateDir: coreDir}
}

func TestNativeCodexThroughGateway(t *testing.T) {
	t.Setenv("TERM_PROGRAM", "native-parity")
	t.Setenv("TERM_PROGRAM_VERSION", "1")
	t.Setenv("TERM", "dumb")
	for _, subscription := range []bool{false, true} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("subscription=%t/ws=%t/%s", subscription, ws, model), func(t *testing.T) {
					capture := newNativeCapture(t)
					gateway := newNativeGateway(t, capture.server.URL, subscription)
					options := nativeOptions{endpoint: gateway.server.URL, bearer: gateway.secret, model: model, ws: ws, base: "  native {{literal}} base  "}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					client.turn(thread, "first synthetic turn")
					client.turn(thread, "second synthetic turn")
					downstream := gateway.tap.packets(t)
					upstream := capture.snapshot()
					if len(downstream) != len(upstream) {
						t.Fatalf("native request count differs across gateway: in=%d out=%d", len(downstream), len(upstream))
					}
					if len(businessWires(upstream)) != 3 {
						t.Fatal("gateway added or dropped a native inference")
					}
					for index, wire := range upstream {
						if !subscription {
							if !bytes.Equal(downstream[index].payload, wire.encodedBody) {
								t.Fatal("API-key native request payload changed")
							}
							continue
						}
						if wire.headers.Get("Originator") != "codex-tui" || wire.headers.Get("Version") != "0.153.4" {
							t.Fatal("Subscription fingerprint identity mismatch")
						}
						if !ws && wire.value["previous_response_id"] != nil {
							t.Fatal("Subscription HTTP did not send full context")
						}
						input, _ := wire.value["input"].([]any)
						if len(input) > 0 {
							item, _ := input[0].(map[string]any)
							if item["type"] == "additional_tools" {
								assertNativeLiteIDs(t, wire)
							}
						}
						if model == "gpt-5.4" && wire.value["instructions"] != options.base {
							t.Fatal("native caller base instructions changed")
						}
					}
					// Independently parse the actual upstream TCP bytes, including compressed WS text frames.
					tapped := capture.tap.packets(t)
					if len(tapped) != len(upstream) {
						t.Fatal("raw upstream tap missed a request")
					}
					for i := range upstream {
						assertNativeWireParity(t, downstream[i], tapped[i], upstream[i], subscription)
						if subscription {
							assertNativeMessageParity(t, downstream[i], upstream[i])
						}
					}
					if !subscription {
						assertProfileStateFileCount(t, gateway.stateDir, 0)
					}
					t.Logf("native gateway captured %d requests; subscription=%t ws=%t model=%s", len(upstream), subscription, ws, model)
					saveFinalCapture(t, downstream, upstream)
				})
			}
		}
	}
}

// Keep ordinary clients independent of the native driver; used by the expanded capability matrix.
func decodeNativePacket(t *testing.T, packet nativePacket) map[string]any {
	t.Helper()
	var result map[string]any
	if json.Unmarshal(packet.payload, &result) != nil {
		t.Fatal("invalid uncompressed packet JSON")
	}
	return result
}
