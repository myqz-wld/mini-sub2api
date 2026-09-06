//go:build nativeparity

package integration

import (
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"
)

// Scenario configuration must never replace the loopback provider or enable credential traffic.
func nativeScenarioConfigKeyAllowed(key string) bool {
	switch key {
	case "instructions", "model_instructions_file", "developer_instructions", "personality",
		"include_permissions_instructions", "include_collaboration_mode_instructions", "include_environment_context",
		"model_reasoning_effort", "model_reasoning_summary", "model_verbosity", "model_context_window",
		"model_auto_compact_token_limit", "experimental_compact_prompt_file", "compact_prompt",
		"plan_mode_reasoning_effort", "project_doc_max_bytes", "project_doc_fallback_filenames",
		"model_providers.native_capture.stream_max_retries":
		return true
	}
	return strings.HasPrefix(key, "skills.") || strings.HasPrefix(key, "features.")
}

func nativeConfigArguments(t *testing.T, overrides map[string]string) []string {
	t.Helper()
	keys := make([]string, 0, len(overrides))
	for key := range overrides {
		if !nativeScenarioConfigKeyAllowed(key) {
			t.Fatal("native scenario configuration key is outside the fixture allowlist")
		}
		if key == "model_providers.native_capture.stream_max_retries" {
			retries, err := strconv.Atoi(overrides[key])
			if err != nil || retries < 0 || retries > 2 {
				t.Fatal("native retry fixture must be a bounded count")
			}
		}
		keys = append(keys, key)
	}
	sort.Strings(keys)
	var args []string
	for _, key := range keys {
		args = append(args, "-c", key+"="+overrides[key])
	}
	return args
}

func writeNativeHomeFixtures(t *testing.T, directory string, fixtures map[string]string) {
	t.Helper()
	for name, content := range fixtures {
		if !filepath.IsLocal(name) || name == "config.toml" || name == "auth.json" {
			t.Fatal("native home fixture must be local and cannot replace safety configuration")
		}
		target := filepath.Join(directory, name)
		if os.MkdirAll(filepath.Dir(target), 0700) != nil || os.WriteFile(target, []byte(content), 0600) != nil {
			t.Fatal("write synthetic native home fixture")
		}
	}
}

func validateNativeThreadParams(t *testing.T, params map[string]any) {
	t.Helper()
	if params["ephemeral"] != true {
		t.Fatal("native scenario must not persist a rollout")
	}
	if provider, ok := params["modelProvider"]; ok && provider != "native_capture" {
		t.Fatal("native scenario cannot override its loopback provider")
	}
	if config, ok := params["config"].(map[string]any); ok {
		for key := range config {
			if !nativeScenarioConfigKeyAllowed(key) {
				t.Fatal("native thread configuration key is outside the fixture allowlist")
			}
		}
	}
}
