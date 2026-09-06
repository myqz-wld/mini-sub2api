//go:build nativeparity

package integration

import (
	"bytes"
	"context"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/klauspost/compress/zstd"
	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/httpapi"
	"mini-sub2api/src/coordinator/internal/storage"
)

// Every entry gets an independent credential and Key in one temporary state.
// Repeated account names intentionally exercise duplicate OAuth imports.
func fingerprintBoundaryGateways(t *testing.T, upstream, mode string, accounts []string) []nativeGateway {
	t.Helper()
	assertLoopbackURL(t, upstream)
	binary := findCoreBinary(t)
	stateDir := t.TempDir()
	coreDir := filepath.Join(stateDir, "core-codex")
	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal("device probe store")
	}
	t.Cleanup(func() { _ = store.Close() })
	var gateways []nativeGateway
	for index, account := range accounts {
		var metadata coreMetadata
		if account == "" {
			metadata = createCoreCredential(t, binary, []string{"credential", "add-api-key", "--state-dir", coreDir, "--upstream-url", upstream + "/v1/responses", "--fingerprint-mode", mode, "--secret-stdin"}, upstreamAPIKey+"\n")
		} else {
			authPath := filepath.Join(stateDir, fmt.Sprintf("synthetic-auth-%d.json", index))
			auth := mustRequestJSON(t, map[string]any{"auth_mode": "chatgpt", "tokens": map[string]string{"id_token": testJWT(&account, 3600), "access_token": testJWT(nil, 3600), "refresh_token": "synthetic-not-imported", "account_id": account}})
			if os.WriteFile(authPath, auth, 0600) != nil {
				t.Fatal("device probe synthetic auth")
			}
			metadata = createCoreCredential(t, binary, []string{"credential", "import-codex-auth", "--state-dir", coreDir, "--auth-file", authPath, "--issuer", upstream, "--client-id", "native-loopback-client", "--upstream-url", upstream + "/v1/responses", "--fingerprint-mode", mode}, "")
		}
		credential := persistCredential(t, store, "Device probe", metadata)
		for keyIndex := 0; keyIndex < 2; keyIndex++ {
			key := createDownstreamKey(t, store, credential.ID, "Device probe Key")
			gateways = append(gateways, nativeGateway{secret: key.Secret, stateDir: coreDir})
		}
	}
	supervisor, err := adapter.Start(context.Background(), adapter.Config{Binary: binary, StateDir: coreDir})
	if err != nil {
		t.Fatal("device probe Core start")
	}
	t.Cleanup(func() { _ = supervisor.Close() })
	handler := httpapi.NewHandler(store, supervisor, nil)
	server, tap := newNativeTappedServer(t, handler)
	t.Cleanup(handler.ShutdownWebSockets)
	for index := range gateways {
		gateways[index].server, gateways[index].tap = server, tap
	}
	return gateways
}

func TestNativeAccountDeviceScope(t *testing.T) {
	for _, mode := range []string{"device", "off"} {
		for _, ws := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/ws=%t", mode, ws), func(t *testing.T) {
				capture := newNativeCapture(t)
				gateways := fingerprintBoundaryGateways(t, capture.server.URL, mode, []string{"synthetic-shared-account", "synthetic-shared-account", "synthetic-other-account"})
				for _, gateway := range gateways {
					client := ordinaryClient(t, gateway, ws, true, nil)
					for repeat := 0; repeat < 2; repeat++ {
						client.send(map[string]any{
							"model": "gpt-5.4", "instructions": "synthetic", "input": "synthetic device scope",
							"client_metadata": map[string]any{"session_id": "same-caller-session", "x-codex-installation-id": "same-caller-installation"},
						})
					}
				}
				allWires := capture.snapshot()
				wires := businessWires(allWires)
				expectedAll := 12
				if ws {
					// Each fresh ordinary socket emits one hidden setup prewarm
					// in addition to its two requested business operations.
					expectedAll = 18
				}
				if len(wires) != 12 || len(allWires) != expectedAll {
					t.Fatal("device scope request count changed")
				}
				devices := make([]string, 6)
				sessions := map[string]bool{}
				for index := 0; index < 6; index++ {
					flat, nested := nativeScenarioTraceMetadata(t, wires[index*2].value)
					repeated, _ := nativeScenarioTraceMetadata(t, wires[index*2+1].value)
					device, _ := flat["x-codex-installation-id"].(string)
					session, _ := flat["session_id"].(string)
					if !isUUIDVersion(device, '4') || device != nested["installation_id"] || device != repeated["x-codex-installation-id"] {
						t.Fatal("device carriers or repeat stability changed")
					}
					if !isUUIDVersion(session, '7') || sessions[session] || session != repeated["session_id"] {
						t.Fatal("Key-scoped session isolation or repeat stability changed")
					}
					sessions[session], devices[index] = true, device
				}
				// Validate every prewarm too: it must retain the selected
				// session's device. No control frame is silently discarded.
				for _, wire := range allWires {
					flat, nested := nativeScenarioTraceMetadata(t, wire.value)
					matched := false
					for index := range devices {
						business, _ := nativeScenarioTraceMetadata(t, wires[index*2].value)
						if flat["session_id"] == business["session_id"] {
							matched = true
							if flat["x-codex-installation-id"] != devices[index] || nested["installation_id"] != devices[index] {
								t.Fatal("prewarm device carrier changed")
							}
						}
					}
					if !matched {
						t.Fatal("prewarm introduced another session")
					}
				}
				if mode == "device" {
					if devices[0] != devices[1] || devices[0] != devices[2] || devices[0] != devices[3] || devices[4] != devices[5] || devices[0] == devices[4] {
						t.Fatal("account device convergence or other-account isolation changed")
					}
				} else {
					seen := map[string]bool{}
					for _, device := range devices {
						if seen[device] {
							t.Fatal("off mode merged distinct Key installation scopes")
						}
						seen[device] = true
					}
				}
			})
		}
	}
}

func TestNativeAPIKeyModeMarkers(t *testing.T) {
	for _, mode := range []string{"device", "off"} {
		for _, ws := range []bool{false, true} {
			for _, marker := range []string{"", "not-codex", "codex_exec"} {
				t.Run(fmt.Sprintf("%s/ws=%t/marker=%s", mode, ws, marker), func(t *testing.T) {
					capture := newNativeCapture(t)
					gateway := fingerprintBoundaryGateways(t, capture.server.URL, mode, []string{""})[0]
					headers := http.Header{"Originator": []string{marker}, "X-Codex-Installation-Id": []string{"synthetic-direct-installation"}}
					client := ordinaryClient(t, gateway, ws, true, headers)
					body := mustRequestJSON(t, map[string]any{
						"type": "response.create", "model": "gpt-5.4", "input": strings.Repeat("synthetic compression fixture ", 500), "stream": true,
						"client_metadata": map[string]any{"x-codex-installation-id": "synthetic-flat-installation", "x-codex-turn-metadata": `{"installation_id":"synthetic-nested-installation"}`},
					})
					encoded := append([]byte(" \n"), body...)
					if !ws {
						encoder, err := zstd.NewWriter(nil)
						if err != nil {
							t.Fatal("API probe zstd encoder")
						}
						encoded = encoder.EncodeAll(encoded, nil)
						_ = encoder.Close()
						client.headers.Set("Content-Encoding", "zstd")
					}
					client.sendEncoded(encoded)
					wires, incoming := capture.snapshot(), gateway.tap.packets(t)
					if len(wires) != 1 || len(incoming) != 1 || !bytes.Equal(encoded, wires[0].encodedBody) || !bytes.Equal(incoming[0].payload, encoded) {
						t.Fatal("API probe encoded application bytes changed")
					}
					if wires[0].headers.Get("X-Codex-Installation-Id") != "synthetic-direct-installation" || wires[0].headers.Get("Originator") != marker {
						t.Fatal("API probe installation or marker header changed")
					}
					assertProfileStateFileCount(t, gateway.stateDir, 0)
				})
			}
		}
	}
}
