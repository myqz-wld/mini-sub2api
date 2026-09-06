//go:build nativeparity

package integration

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

type scenarioBaseModel struct {
	Slug     string `json:"slug"`
	Messages struct {
		Template  string            `json:"instructions_template"`
		Variables map[string]string `json:"instructions_variables"`
	} `json:"model_messages"`
}

func scenarioBaseCatalog(t *testing.T, model string) scenarioBaseModel {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(nativeSource(t), "codex-rs", "models-manager", "models.json"))
	if err != nil {
		t.Fatal("read pinned personality catalog")
	}
	var catalog struct{ Models []scenarioBaseModel }
	if json.Unmarshal(data, &catalog) != nil {
		t.Fatal("parse pinned personality catalog")
	}
	for _, entry := range catalog.Models {
		if entry.Slug == model {
			return entry
		}
	}
	t.Fatal("personality scenario model missing in pinned catalog")
	return scenarioBaseModel{}
}

func scenarioBaseExpectedPersonality(model scenarioBaseModel, personality string) string {
	template := model.Messages.Template
	if personality == "none" {
		// Native none removes this exact H1 section; feature-off only renders the
		// default variable. The embedded Lite templates exercise that distinction.
		lines := strings.SplitAfter(template, "\n")
		var kept strings.Builder
		inside, removed := false, false
		for _, line := range lines {
			heading := strings.TrimSuffix(strings.TrimSuffix(line, "\n"), "\r")
			if !removed && heading == "# Personality" {
				inside, removed = true, true
				continue
			}
			if inside && (heading == "#" || strings.HasPrefix(heading, "# ") || strings.HasPrefix(heading, "#\t")) {
				inside = false
			}
			if !inside {
				kept.WriteString(line)
			}
		}
		template = kept.String()
	}
	if model.Messages.Variables == nil {
		return template
	}
	variable := ""
	if personality == "feature-off" {
		variable = model.Messages.Variables["personality_default"]
	} else if personality != "none" {
		variable = model.Messages.Variables["personality_"+personality]
	}
	return strings.ReplaceAll(template, "{{ personality }}", variable)
}

func scenarioBasePersonalityFragments(t *testing.T, wire nativeWire, expectedText string) int {
	t.Helper()
	items, _ := wire.value["input"].([]any)
	count := 0
	for _, raw := range items {
		item, _ := raw.(map[string]any)
		content, _ := item["content"].([]any)
		for _, rawPart := range content {
			part, _ := rawPart.(map[string]any)
			text, _ := part["text"].(string)
			if strings.Contains(text, "<personality_spec>") {
				count++
				if item["role"] != "developer" || len(content) != 1 || !strings.Contains(text, expectedText) {
					t.Fatal("native personality update text or standalone content boundary differs")
				}
			}
		}
	}
	return count
}

func TestNativeScenarioBasePersonalityModes(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, model := range []string{"gpt-5.4", "gpt-5.6-sol", "gpt-5.2"} {
			for _, personality := range []string{"pragmatic", "friendly", "none", "feature-off"} {
				t.Run(route+"/"+model+"/"+personality, func(t *testing.T) {
					catalog := scenarioBaseCatalog(t, model)
					expected := scenarioBaseExpectedPersonality(catalog, personality)
					options := nativeOptions{model: model, configOverrides: map[string]string{}}
					if personality == "feature-off" {
						options.neutralPersonality = true
					} else {
						options.configOverrides["personality"] = strconv.Quote(personality)
					}
					in, out := scenarioBaseRun(t, route, options, func(client *nativeClient, thread string) { client.turn(thread, "synthetic personality mode") })
					for i := range in {
						assertScenarioBaseValue(t, in[i], model == "gpt-5.6-sol", expected)
						assertScenarioBaseValue(t, out[i], model == "gpt-5.6-sol", expected)
						if scenarioBasePersonalityFragments(t, in[i], "") != 0 {
							t.Fatal("initial personality incorrectly emitted a fallback fragment")
						}
					}
				})
			}
		}
	}
}

func TestNativeScenarioBasePersonalityUpdates(t *testing.T) {
	for _, route := range []string{"direct", "api-key", "subscription"} {
		for _, ws := range []bool{false, true} {
			for _, profile := range []string{"template", "custom", "lite-no-variables"} {
				t.Run(fmt.Sprintf("%s/ws=%t/%s", route, ws, profile), func(t *testing.T) {
					options := nativeOptions{model: "gpt-5.4", ws: ws, threadParams: map[string]any{"personality": "pragmatic"}}
					if profile == "custom" {
						options.base = "literal {{ personality }} custom base"
					}
					if profile == "lite-no-variables" {
						options.model = "gpt-5.6-sol"
					}
					catalog := scenarioBaseCatalog(t, options.model)
					expected := scenarioBaseExpectedPersonality(catalog, "pragmatic")
					if options.base != "" {
						expected = options.base
					}
					in, out := scenarioBaseRun(t, route, options, func(client *nativeClient, thread string) {
						client.turn(thread, "synthetic initial personality")
						for i := 0; i < 2; i++ {
							client.turnWith(thread, map[string]any{"input": []any{map[string]any{"type": "text", "text": "synthetic personality update"}}, "personality": "friendly"})
						}
					})
					for _, pair := range [][]nativeWire{in, out} {
						business := businessWires(pair)
						if len(business) != 4 {
							t.Fatal("personality lifecycle expected tool loop plus two later turns")
						}
						for i, wire := range business {
							assertScenarioBaseValue(t, wire, options.model == "gpt-5.6-sol", expected)
							expectedCount := 0
							if profile == "template" && i >= 2 && (!ws || i == 2) {
								expectedCount = 1
							}
							if scenarioBasePersonalityFragments(t, wire, catalog.Messages.Variables["personality_friendly"]) != expectedCount {
								t.Fatal("personality lifecycle appended, lost or repeated an update incorrectly")
							}
							if ws && i >= 2 && wire.value["previous_response_id"] == nil {
								t.Fatal("personality input update unexpectedly invalidated a reusable WS configuration")
							}
						}
					}
				})
			}
		}
	}
}
