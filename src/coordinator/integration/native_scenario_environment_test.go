//go:build nativeparity

package integration

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const (
	envDeveloper        = "ENV_SYNTHETIC_DEVELOPER"
	envHome             = "ENV_SYNTHETIC_HOME_OVERRIDE"
	envRoot             = "ENV_SYNTHETIC_ROOT_OVERRIDE"
	envLeaf             = "ENV_SYNTHETIC_LEAF_AGENTS"
	envSkillDescription = "ENV_SYNTHETIC_SKILL_DESCRIPTION"
	envSkillBody        = "ENV_SYNTHETIC_SKILL_BODY"
)

func TestNativeScenarioEnvironmentContext(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("subscription=%t/ws=%t/%s", subscription, ws, model), func(t *testing.T) {
					root, cwd := environmentProject(t)
					upstream := newNativeCapture(t)
					gateway := newNativeGateway(t, upstream.server.URL, subscription)
					options := environmentOptions(model, cwd)
					options.endpoint, options.bearer, options.ws = gateway.server.URL, gateway.secret, ws
					options.homeFiles = map[string]string{"AGENTS.md": "ENV_IGNORED_HOME", "AGENTS.override.md": envHome}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					skill := environmentWriteSkill(t, root, "environment-probe", envSkillDescription, envSkillBody)
					client.call("skills/extraRoots/set", map[string]any{"extraRoots": []any{filepath.Dir(filepath.Dir(skill))}})
					skill = environmentResolvedSkill(t, client, cwd, "environment-probe")
					client.turnWith(thread, map[string]any{"input": []any{map[string]any{"type": "text", "text": "synthetic environment context"}}, "collaborationMode": environmentMode(model, "default", "ENV_CUSTOM_DEFAULT_MODE")})
					first := environmentFirstContext(t, gateway)
					for _, marker := range []string{envHome, envRoot, envLeaf} {
						environmentAssertCount(t, first, "user", marker, 1)
					}
					for _, marker := range []string{"ENV_IGNORED_HOME", "ENV_IGNORED_ROOT", "ENV_IGNORED_MIDDLE", "ENV_OUTSIDE_PROJECT"} {
						environmentAssertCount(t, first, "user", marker, 0)
					}
					environmentAssertCount(t, first, "developer", envDeveloper, 1)
					environmentAssertCount(t, first, "developer", envSkillDescription, 1)
					environmentAssertCount(t, first, "user", envSkillBody, 0)
					environmentAssertCount(t, first, "developer", "<permissions instructions>", 1)
					environmentAssertCount(t, first, "developer", "<collaboration_mode>", 1)
					environmentAssertCount(t, first, "user", "<environment_context>", 1)
					environmentAssertCount(t, first, "user", "<cwd>"+cwd+"</cwd>", 1)
					environmentAssertGroupedKinds(t, first, "user", "agents_md.instructions", "environments.environment_context")
					environmentAssertGroupedKinds(t, first, "developer", "generic.developer_instructions", "permissions.instructions", "collaboration_mode.instructions")
					environmentAssertSkillUsage(t, first, model == "gpt-5.4")
					client.turnWith(thread, map[string]any{"input": []any{map[string]any{"type": "text", "text": "synthetic selected skill"}, map[string]any{"type": "skill", "name": "environment-probe", "path": skill}}})
					packets := environmentCallerValues(t, gateway)
					last := packets[len(packets)-1]
					environmentAssertCount(t, last, "user", envSkillBody, 1)
					environmentAssertCount(t, last, "user", "<skill>", 1)
					environmentAssertCount(t, last, "developer", envSkillBody, 0)
					environmentAssertParity(t, gateway, upstream, subscription)
					t.Log("actual native context: AGENTS precedence, fragment boundaries, skill listing/selection and gateway preservation verified")
				})
			}
		}
	}
}

func TestNativeScenarioEnvironmentToggles(t *testing.T) {
	for _, scenario := range []string{"empty", "all-presentation-off", "skills-hidden-selected", "skill-disabled"} {
		for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
			t.Run(scenario+"/"+model, func(t *testing.T) {
				cwd := t.TempDir()
				capture := newNativeCapture(t)
				gateway := newNativeGateway(t, capture.server.URL, true)
				options := environmentOptions(model, cwd)
				options.endpoint, options.bearer = gateway.server.URL, gateway.secret
				if scenario == "all-presentation-off" {
					for _, key := range []string{"include_environment_context", "include_permissions_instructions", "include_collaboration_mode_instructions", "skills.include_instructions"} {
						options.configOverrides[key] = "false"
					}
				}
				if scenario == "skills-hidden-selected" {
					options.configOverrides["skills.include_instructions"] = "false"
				}
				if scenario == "skill-disabled" {
					options.configOverrides["skills.config"] = `[{name="environment-probe",enabled=false}]`
				}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				var skill string
				if scenario != "empty" {
					skill = environmentWriteSkill(t, cwd, "environment-probe", envSkillDescription, envSkillBody)
					client.call("skills/extraRoots/set", map[string]any{"extraRoots": []any{filepath.Dir(filepath.Dir(skill))}})
					skill = environmentResolvedSkill(t, client, cwd, "environment-probe")
				}
				client.turnWith(thread, map[string]any{"input": []any{map[string]any{"type": "text", "text": "synthetic presentation toggles"}}, "collaborationMode": environmentMode(model, "default", "ENV_TOGGLE_MODE_TEXT")})
				first := environmentFirstContext(t, gateway)
				environmentAssertCount(t, first, "developer", "<skills_instructions>", 0)
				environmentAssertCount(t, first, "developer", envSkillDescription, 0)
				environmentAssertCount(t, first, "user", "<skill>", 0)
				if scenario == "all-presentation-off" {
					environmentAssertCount(t, first, "user", "<environment_context>", 0)
					environmentAssertCount(t, first, "developer", "<permissions instructions>", 0)
					environmentAssertCount(t, first, "developer", "<collaboration_mode>", 0)
				}
				if scenario == "skills-hidden-selected" || scenario == "skill-disabled" {
					client.turnWith(thread, map[string]any{"input": []any{map[string]any{"type": "text", "text": "synthetic explicit selection"}, map[string]any{"type": "skill", "name": "environment-probe", "path": skill}}})
					packets := environmentCallerValues(t, gateway)
					want := 0
					if scenario == "skills-hidden-selected" {
						want = 1
					}
					environmentAssertCount(t, packets[len(packets)-1], "user", envSkillBody, want)
				}
				environmentAssertParity(t, gateway, capture, true)
			})
		}
	}
}

func environmentOptions(model, cwd string) nativeOptions {
	return nativeOptions{model: model, project: cwd, base: "Synthetic environment base", neutralPersonality: true,
		configOverrides: map[string]string{"skills.bundled.enabled": "false", "skills.include_instructions": "true"},
		threadParams:    map[string]any{"developerInstructions": envDeveloper, "environments": []any{map[string]any{"environmentId": "local", "cwd": cwd, "runtimeWorkspaceRoots": []any{cwd}}}},
	}
}

func environmentProject(t *testing.T) (string, string) {
	t.Helper()
	outer := t.TempDir()
	root := filepath.Join(outer, "project")
	cwd := filepath.Join(root, "middle", "leaf")
	for _, directory := range []string{cwd, filepath.Join(root, ".git")} {
		if os.MkdirAll(directory, 0700) != nil {
			t.Fatal("create synthetic project")
		}
	}
	for name, value := range map[string]string{
		"AGENTS.md": "ENV_OUTSIDE_PROJECT", "project/AGENTS.md": "ENV_IGNORED_ROOT", "project/AGENTS.override.md": envRoot,
		"project/middle/AGENTS.md": "ENV_IGNORED_MIDDLE", "project/middle/AGENTS.override.md": "", "project/middle/leaf/AGENTS.md": envLeaf,
	} {
		if os.WriteFile(filepath.Join(outer, name), []byte(value), 0600) != nil {
			t.Fatal("write synthetic project instructions")
		}
	}
	return root, cwd
}

func environmentWriteSkill(t *testing.T, root, name, description, body string) string {
	t.Helper()
	directory := filepath.Join(root, "synthetic-skills", name)
	if os.MkdirAll(directory, 0700) != nil {
		t.Fatal("create synthetic skill")
	}
	path := filepath.Join(directory, "SKILL.md")
	content := fmt.Sprintf("---\nname: %s\ndescription: %s\n---\n\n%s\n", name, description, body)
	if os.WriteFile(path, []byte(content), 0600) != nil {
		t.Fatal("write synthetic skill")
	}
	return path
}

func environmentAssertSkillUsage(t *testing.T, value map[string]any, expected bool) {
	t.Helper()
	for _, item := range environmentMessages(value, "developer") {
		for _, text := range environmentTexts(item) {
			if strings.Contains(text, "<skills_instructions>") {
				if strings.Contains(text, "### How to use skills") != expected {
					t.Fatal("native model Skills usage routing differs from catalog")
				}
				return
			}
		}
	}
	t.Fatal("native populated skills catalog missing")
}
