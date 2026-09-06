//go:build nativeparity

package integration

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"
)

func TestNativeFinalEnvironmentAndPersonality(t *testing.T) {
	for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
		for _, ws := range []bool{false, true} {
			for _, personality := range []string{"none", "friendly", "pragmatic"} {
				t.Run(fmt.Sprintf("%s/ws=%t/%s", model, ws, personality), func(t *testing.T) {
					firstDir, secondDir := t.TempDir(), t.TempDir()
					for path, marker := range map[string]string{firstDir: "FINAL_FIRST_AGENTS", secondDir: "FINAL_SECOND_AGENTS"} {
						if os.WriteFile(filepath.Join(path, "AGENTS.md"), []byte(marker), 0600) != nil {
							t.Fatal("write final environment fixture")
						}
					}
					capture := newNativeFailureCapture(t, "completed", 0, 0)
					gateway := newNativeGateway(t, capture.server.URL, true)
					options := environmentOptions(model, firstDir)
					options.endpoint, options.bearer, options.ws = gateway.server.URL, gateway.secret, ws
					options.base = ""
					options.neutralPersonality = false
					options.configOverrides["personality"] = fmt.Sprintf("%q", personality)
					options.configOverrides["features.code_mode_host"] = "true"
					client := startNativeClient(t, options)
					thread := client.thread(options)
					client.turn(thread, "First synthetic environment observation; no tools.")
					first := environmentFirstContext(t, gateway)
					environmentAssertCount(t, first, "user", "FINAL_FIRST_AGENTS", 1)
					environmentAssertCount(t, first, "user", "<cwd>"+firstDir+"</cwd>", 1)
					date, zone, shell := "", "", ""
					for _, message := range environmentMessages(first, "user") {
						for _, text := range environmentTexts(message) {
							if strings.Contains(text, "<environment_context>") {
								date, zone, shell = finalEnvironmentTag(text, "current_date"), finalEnvironmentTag(text, "timezone"), finalEnvironmentTag(text, "shell")
								// Native only emits a network block when managed requirements contain
								// network policy. It is optional even with an explicit local workspace.
								if !strings.Contains(text, "<filesystem>") {
									t.Fatal("native filesystem context absent")
								}
							}
						}
					}
					location, err := time.LoadLocation(zone)
					if err != nil || shell == "" || date != time.Now().In(location).Format("2006-01-02") {
						t.Fatal("native date/timezone/shell context is not coherent")
					}
					client.turnWith(thread, map[string]any{"input": []any{map[string]any{"type": "text", "text": "Changed synthetic directory; no tools."}}, "environments": []any{map[string]any{"environmentId": "local", "cwd": secondDir}}})
					caller := environmentCallerValues(t, gateway)
					changed := caller[len(caller)-1]
					environmentAssertCount(t, changed, "user", "FINAL_SECOND_AGENTS", 1)
					environmentAssertCount(t, changed, "user", "<cwd>"+secondDir+"</cwd>", 1)
					client.turn(thread, "Unchanged synthetic directory; no tools.")
					environmentAssertParity(t, gateway, capture.nativeCapture, true)
					wires := capture.snapshot()
					if len(businessWires(wires)) != 3 {
						t.Fatal("environment observation changed inference count")
					}
					for _, wire := range wires {
						if len(finalTools(wire.value)) > 0 && model == "gpt-6-astra" {
							assertNativeLiteIDs(t, wire)
						}
					}
					saveFinalCapture(t, gateway.tap.packets(t), wires)
				})
			}
		}
	}
}

func finalEnvironmentTag(text, tag string) string {
	match := regexp.MustCompile("<" + regexp.QuoteMeta(tag) + ">([^<]+)</" + regexp.QuoteMeta(tag) + ">").FindStringSubmatch(text)
	if len(match) != 2 {
		return ""
	}
	return match[1]
}

func TestNativeFinalOrdinaryEnvironmentText(t *testing.T) {
	for _, model := range []string{"gpt-5.5", "gpt-6-astra"} {
		for _, delivery := range []string{"json", "sse", "ws"} {
			for _, zone := range []string{"Etc/UTC", "Asia/Shanghai"} {
				t.Run(fmt.Sprintf("%s/%s/%s", model, delivery, zone), func(t *testing.T) {
					capture := newNativeFailureCapture(t, "completed", 0, 0)
					gateway := newNativeGateway(t, capture.server.URL, true)
					client := ordinaryClient(t, gateway, delivery == "ws", delivery != "json", nil)
					text := "<environment_context><cwd>/caller/synthetic-workspace</cwd><shell>caller-shell</shell><timezone>" + zone + "</timezone></environment_context>"
					request := map[string]any{"model": model, "instructions": "Caller-owned base {{literal}}", "input": []any{map[string]any{"role": "developer", "content": "Caller-owned personality and permission text"}, map[string]any{"role": "user", "content": text}, map[string]any{"role": "user", "content": "Synthetic environment-preservation task"}}}
					client.send(request)
					wire := ordinaryResolvedFirstReference(t, capture.snapshot(), "resp_failure_1")
					if environmentCount(wire.value, "user", text) != 1 || environmentCount(wire.value, "developer", "Caller-owned personality and permission text") != 1 {
						t.Fatal("ordinary caller environment/personality text changed")
					}
					metadata, _ := wire.value["client_metadata"].(map[string]any)
					var turn map[string]any
					encoded, _ := metadata["x-codex-turn-metadata"].(string)
					if json.Unmarshal([]byte(encoded), &turn) != nil || turn["workspaces"] != nil || len(finalTools(wire.value)) != 0 {
						t.Fatal("ordinary text acquired execution context or tools")
					}
					saveFinalCapture(t, gateway.tap.packets(t), capture.snapshot())
				})
			}
		}
	}
}
