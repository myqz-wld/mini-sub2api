package main

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestRenderSnapshotsUsesEffectiveDefaultAndPreservesWhitespace(t *testing.T) {
	models := fixtureModels()
	rendered, err := renderSnapshots(marshalModels(t, models), " fallback\n", "experiment header")
	if err != nil {
		t.Fatal(err)
	}
	if len(rendered) != 7 {
		t.Fatalf("snapshot count = %d, want 7", len(rendered))
	}
	for _, name := range modelFiles {
		if string(rendered[name]) != "  default personality\n" {
			t.Fatalf("rendered default or whitespace changed for %s", name)
		}
	}
	if string(rendered["fallback.md"]) != " fallback\n" ||
		string(rendered["exp-codex-personality.md"]) != "experiment header\n\n\n\n fallback\n" {
		t.Fatal("fallback rendering changed")
	}
}

func TestRenderSnapshotsRejectsIncompleteOrUnrenderedAssets(t *testing.T) {
	for _, name := range []string{
		"missing_model", "duplicate_model", "missing_messages", "missing_template",
		"unknown_placeholder", "literal_placeholder", "empty_prompt", "different_shared_prompt",
		"fallback_placeholder", "experimental_placeholder",
	} {
		t.Run(name, func(t *testing.T) {
			models := fixtureModels()
			fallback, header := "fallback", "header"
			switch name {
			case "missing_model":
				models = models[1:]
			case "duplicate_model":
				models = append(models, models[0])
			case "missing_messages":
				models[0].Messages = nil
			case "missing_template":
				models[0].Messages.Template = nil
			case "unknown_placeholder":
				*models[0].Messages.Template = "{{ unresolved }}"
			case "literal_placeholder":
				models[0].Messages.Variables = nil
			case "empty_prompt":
				for i := range models {
					*models[i].Messages.Template = " \n"
				}
			case "different_shared_prompt":
				for i := range models {
					if models[i].Slug == "gpt-5.6-sol" {
						*models[i].Messages.Template = "different"
					}
				}
			case "fallback_placeholder":
				fallback = "{{ unresolved }}"
			case "experimental_placeholder":
				header = "{{ unresolved }}"
			}
			if _, err := renderSnapshots(marshalModels(t, models), fallback, header); err == nil {
				t.Fatal("invalid source produced snapshots")
			}
		})
	}
}

func TestRenderSnapshotsHandlesAbsentAndEmptyDefaultVariables(t *testing.T) {
	for _, variables := range []*instructionVariables{nil, {}} {
		models := fixtureModels()
		for i := range models {
			models[i].Messages.Variables = variables
			if variables == nil {
				*models[i].Messages.Template = `literal base {"authority":{"kind":"orchestrator"}}`
			} else {
				*models[i].Messages.Template = "base {{ personality }}\n"
			}
		}
		result, err := renderSnapshots(marshalModels(t, models), "fallback", "header")
		if err != nil {
			t.Fatal(err)
		}
		for _, name := range modelFiles {
			want := "base \n"
			if variables == nil {
				want = `literal base {"authority":{"kind":"orchestrator"}}`
			}
			if string(result[name]) != want {
				t.Fatalf("literal template changed for %s", name)
			}
		}
	}
}

func fixtureModels() []model {
	var models []model
	for slug := range modelFiles {
		template := "  {{ personality }}\n"
		models = append(models, model{
			Slug: slug,
			Messages: &modelMessages{
				Template:  &template,
				Variables: &instructionVariables{PersonalityDefault: "default personality"},
			},
		})
	}
	return models
}

func marshalModels(t *testing.T, models []model) []byte {
	t.Helper()
	data, err := json.Marshal(map[string]any{"models": models})
	if err != nil {
		t.Fatal(err)
	}
	return data
}

func TestRenderSnapshotsRejectsPlaceholdersWithoutExposingPromptContent(t *testing.T) {
	_, err := renderSnapshots(marshalModels(t, fixtureModels()), "private {{ unresolved }} text", "header")
	if err == nil || !strings.Contains(err.Error(), "placeholder") || strings.Contains(err.Error(), "private") {
		t.Fatal("placeholder diagnostics must name the asset without printing its contents")
	}
}
