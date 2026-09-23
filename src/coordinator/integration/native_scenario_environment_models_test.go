//go:build nativeparity

package integration

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestNativeScenarioEnvironmentSkillModels(t *testing.T) {
	data, err := os.ReadFile(filepath.Join(nativeSource(t), "codex-rs", "models-manager", "models.json"))
	if err != nil {
		t.Fatal("read native skill routing catalog")
	}
	var catalog struct {
		Models []struct {
			Slug               string `json:"slug"`
			IncludeSkillsUsage bool   `json:"include_skills_usage_instructions"`
		} `json:"models"`
	}
	if json.Unmarshal(data, &catalog) != nil || len(catalog.Models) != 9 {
		t.Fatal("native skill routing catalog shape")
	}
	for _, model := range catalog.Models {
		t.Run(model.Slug, func(t *testing.T) {
			capture := newNativeCapture(t)
			gateway := newNativeGateway(t, capture.server.URL, true)
			options := environmentOptions(model.Slug, t.TempDir())
			options.base = ""
			options.endpoint, options.bearer = gateway.server.URL, gateway.secret
			options.homeFiles = map[string]string{"skills/environment-probe/SKILL.md": "---\nname: environment-probe\ndescription: ENV_SYNTHETIC_SKILL_DESCRIPTION\n---\nENV_SYNTHETIC_SKILL_BODY\n"}
			client := startNativeClient(t, options)
			thread := client.thread(options)
			client.turn(thread, "synthetic model skill routing")
			first := environmentFirstContext(t, gateway)
			environmentAssertCount(t, first, "developer", envSkillDescription, 1)
			environmentAssertSkillUsage(t, first, model.IncludeSkillsUsage)
			base := capturedBase(t, nativeWire{value: first})
			if strings.Contains(base, "SKILL.md") == model.IncludeSkillsUsage {
				t.Fatal("native embedded Skills rules disagree with separate usage flag")
			}
			environmentAssertCount(t, first, "user", envSkillBody, 0)
			environmentAssertParity(t, gateway, capture, true)
		})
	}
}

func TestNativeScenarioEnvironmentPermissionsChange(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
		t.Run(model, func(t *testing.T) {
			capture := newNativeCapture(t)
			gateway := newNativeGateway(t, capture.server.URL, true)
			options := environmentOptions(model, t.TempDir())
			options.endpoint, options.bearer = gateway.server.URL, gateway.secret
			client := startNativeClient(t, options)
			thread := client.thread(options)
			client.turn(thread, "synthetic initial permission policy")
			first := environmentFirstContext(t, gateway)
			environmentAssertCount(t, first, "developer", "Approval policy is currently never.", 1)
			environmentAssertCount(t, first, "developer", "# Escalation Requests", 0)
			// The mock still invokes only native_probe; changing prompt policy never runs a shell command.
			client.turnWith(thread, map[string]any{"input": []any{map[string]any{"type": "text", "text": "synthetic changed permission policy"}}, "approvalPolicy": "on-request"})
			packets := environmentCallerValues(t, gateway)
			changed := packets[len(packets)-1]
			environmentAssertCount(t, changed, "developer", "<permissions instructions>", 2)
			environmentAssertCount(t, changed, "developer", "# Escalation Requests", 1)
			client.turn(thread, "synthetic stable permission policy")
			packets = environmentCallerValues(t, gateway)
			environmentAssertCount(t, packets[len(packets)-1], "developer", "<permissions instructions>", 2)
			environmentAssertCount(t, packets[len(packets)-1], "developer", "# Escalation Requests", 1)
			environmentAssertParity(t, gateway, capture, true)
		})
	}
}
