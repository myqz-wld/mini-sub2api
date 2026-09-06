//go:build nativeparity

package integration

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestNativeScenarioEnvironmentDirectoryChanges(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, model), func(t *testing.T) {
				firstDir, secondDir := t.TempDir(), t.TempDir()
				for path, marker := range map[string]string{firstDir: "ENV_FIRST_DIRECTORY", secondDir: "ENV_SECOND_DIRECTORY"} {
					if os.WriteFile(filepath.Join(path, "AGENTS.md"), []byte(marker), 0600) != nil {
						t.Fatal("write context transition fixture")
					}
				}
				capture := newNativeCapture(t)
				gateway := newNativeGateway(t, capture.server.URL, true)
				options := environmentOptions(model, firstDir)
				options.endpoint, options.bearer, options.ws = gateway.server.URL, gateway.secret, ws
				client := startNativeClient(t, options)
				thread := client.thread(options)
				client.turn(thread, "synthetic first directory")
				environmentAssertCount(t, environmentFirstContext(t, gateway), "user", "ENV_FIRST_DIRECTORY", 1)
				client.turnWith(thread, map[string]any{"input": []any{map[string]any{"type": "text", "text": "synthetic changed directory"}}, "environments": []any{map[string]any{"environmentId": "local", "cwd": secondDir}}})
				packets := environmentCallerValues(t, gateway)
				changed := packets[len(packets)-1]
				environmentAssertCount(t, changed, "user", "ENV_SECOND_DIRECTORY", 1)
				environmentAssertCount(t, changed, "user", "These AGENTS.md instructions replace all previously provided AGENTS.md instructions.", 1)
				environmentAssertCount(t, changed, "user", "<cwd>"+secondDir+"</cwd>", 1)
				client.turn(thread, "synthetic unchanged directory")
				packets = environmentCallerValues(t, gateway)
				unchanged := packets[len(packets)-1]
				want := 1
				if ws {
					if unchanged["previous_response_id"] == nil {
						t.Fatal("unchanged native environment did not reuse WS baseline")
					}
					want = 0
				}
				environmentAssertCount(t, unchanged, "user", "ENV_SECOND_DIRECTORY", want)
				environmentAssertCount(t, unchanged, "user", "These AGENTS.md instructions replace all previously provided AGENTS.md instructions.", want)
				environmentAssertParity(t, gateway, capture, true)
			})
		}
	}
}

func TestNativeScenarioEnvironmentSkillCatalogRefresh(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
		t.Run(model, func(t *testing.T) {
			cwd := t.TempDir()
			capture := newNativeCapture(t)
			gateway := newNativeGateway(t, capture.server.URL, true)
			options := environmentOptions(model, cwd)
			options.endpoint, options.bearer = gateway.server.URL, gateway.secret
			client := startNativeClient(t, options)
			thread := client.thread(options)
			firstSkill := environmentWriteSkill(t, cwd, "first-probe", "ENV_FIRST_SKILL_DESCRIPTION", "ENV_FIRST_SKILL_BODY")
			root := filepath.Dir(filepath.Dir(firstSkill))
			refresh := func() { client.call("skills/extraRoots/set", map[string]any{"extraRoots": []any{root}}) }
			refresh()
			client.turn(thread, "synthetic first catalog")
			environmentAssertCount(t, environmentFirstContext(t, gateway), "developer", "ENV_FIRST_SKILL_DESCRIPTION", 1)
			environmentWriteSkill(t, cwd, "second-probe", "ENV_SECOND_SKILL_DESCRIPTION", "ENV_SECOND_SKILL_BODY")
			refresh()
			client.turn(thread, "synthetic changed catalog")
			packets := environmentCallerValues(t, gateway)
			changed := packets[len(packets)-1]
			environmentAssertCount(t, changed, "developer", "ENV_FIRST_SKILL_DESCRIPTION", 2)
			environmentAssertCount(t, changed, "developer", "ENV_SECOND_SKILL_DESCRIPTION", 1)
			client.turn(thread, "synthetic unchanged catalog")
			packets = environmentCallerValues(t, gateway)
			unchanged := packets[len(packets)-1]
			environmentAssertCount(t, unchanged, "developer", "ENV_FIRST_SKILL_DESCRIPTION", 2)
			environmentAssertCount(t, unchanged, "developer", "ENV_SECOND_SKILL_DESCRIPTION", 1)
			environmentAssertCount(t, unchanged, "user", "ENV_SECOND_SKILL_BODY", 0)
			environmentAssertParity(t, gateway, capture, true)
		})
	}
}

func TestNativeScenarioEnvironmentModes(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
		t.Run(model, func(t *testing.T) {
			capture := newNativeCapture(t)
			gateway := newNativeGateway(t, capture.server.URL, true)
			options := environmentOptions(model, t.TempDir())
			options.endpoint, options.bearer = gateway.server.URL, gateway.secret
			client := startNativeClient(t, options)
			thread := client.thread(options)
			client.turn(thread, "synthetic default mode")
			first := environmentFirstContext(t, gateway)
			environmentAssertCount(t, first, "developer", "<collaboration_mode>", 0)
			client.turnWith(thread, map[string]any{"input": []any{map[string]any{"type": "text", "text": "synthetic plan mode"}}, "collaborationMode": environmentMode(model, "plan", nil)})
			packets := environmentCallerValues(t, gateway)
			changed := packets[len(packets)-1]
			environmentAssertCount(t, changed, "developer", "<collaboration_mode>", 1)
			environmentAssertCount(t, changed, "developer", "# Plan Mode", 1)
			client.turn(thread, "synthetic same plan mode")
			packets = environmentCallerValues(t, gateway)
			environmentAssertCount(t, packets[len(packets)-1], "developer", "<collaboration_mode>", 1)
			client.turnWith(thread, map[string]any{"input": []any{map[string]any{"type": "text", "text": "synthetic changed mode text"}}, "collaborationMode": environmentMode(model, "plan", "ENV_CUSTOM_PLAN_TEXT")})
			packets = environmentCallerValues(t, gateway)
			environmentAssertCount(t, packets[len(packets)-1], "developer", "<collaboration_mode>", 2)
			environmentAssertCount(t, packets[len(packets)-1], "developer", "ENV_CUSTOM_PLAN_TEXT", 1)
			environmentAssertParity(t, gateway, capture, true)
		})
	}
}

func environmentMode(model, mode string, instructions any) map[string]any {
	return map[string]any{"mode": mode, "settings": map[string]any{"model": model, "reasoning_effort": "medium", "developer_instructions": instructions}}
}

func TestNativeScenarioEnvironmentAgentsBoundaries(t *testing.T) {
	for _, scenario := range []string{"without-project-root", "empty-leaf-override", "fallback-name", "project-budget", "default-project-budget"} {
		t.Run(scenario, func(t *testing.T) {
			root, cwd := environmentProject(t)
			capture := newNativeCapture(t)
			gateway := newNativeGateway(t, capture.server.URL, true)
			options := environmentOptions("gpt-5.4", cwd)
			options.endpoint, options.bearer = gateway.server.URL, gateway.secret
			rootCount, leafCount := 1, 1
			switch scenario {
			case "without-project-root":
				if os.Remove(filepath.Join(root, ".git")) != nil {
					t.Fatal("remove synthetic project marker")
				}
				rootCount = 0
			case "empty-leaf-override":
				if os.WriteFile(filepath.Join(cwd, "AGENTS.override.md"), nil, 0600) != nil {
					t.Fatal("write empty synthetic override")
				}
				leafCount = 0
			case "fallback-name":
				if os.Rename(filepath.Join(cwd, "AGENTS.md"), filepath.Join(cwd, "CONTEXT.md")) != nil {
					t.Fatal("rename synthetic fallback")
				}
				options.configOverrides["project_doc_fallback_filenames"] = `["CONTEXT.md"]`
			case "project-budget":
				options.configOverrides["project_doc_max_bytes"] = fmt.Sprint(len(envRoot))
				leafCount = 0
			case "default-project-budget":
				if os.WriteFile(filepath.Join(root, "AGENTS.override.md"), []byte(envRoot+strings.Repeat("A", 32*1024-len(envRoot))), 0600) != nil {
					t.Fatal("write synthetic default budget fixture")
				}
				leafCount = 0
			}
			client := startNativeClient(t, options)
			thread := client.thread(options)
			client.turn(thread, "synthetic AGENTS boundary")
			first := environmentFirstContext(t, gateway)
			environmentAssertCount(t, first, "user", envRoot, rootCount)
			environmentAssertCount(t, first, "user", envLeaf, leafCount)
			environmentAssertCount(t, first, "user", "ENV_OUTSIDE_PROJECT", 0)
			environmentAssertParity(t, gateway, capture, true)
		})
	}
}
