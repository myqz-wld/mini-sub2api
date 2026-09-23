package main

import (
	"encoding/json"
	"testing"
)

func TestRenderSnapshotsPreservesLiteralTemplateAndWhitespace(t *testing.T) {
	models := fixtureModels()
	rendered, err := renderSnapshots(marshalModels(t, models), " fallback\n")
	if err != nil {
		t.Fatal(err)
	}
	if len(rendered) != 7 {
		t.Fatalf("snapshot count = %d, want 7", len(rendered))
	}
	for _, name := range modelFiles {
		if string(rendered[name]) != "  {{ personality }}\n" {
			t.Fatalf("rendered default or whitespace changed for %s", name)
		}
	}
	if string(rendered["fallback.md"]) != " fallback\n" {
		t.Fatal("fallback rendering changed")
	}
}

func TestRenderSnapshotsRejectsIncompleteOrUnrenderedAssets(t *testing.T) {
	for _, name := range []string{
		"missing_model", "duplicate_model", "missing_messages", "missing_template",
		"empty_prompt", "different_shared_prompt",
	} {
		t.Run(name, func(t *testing.T) {
			models := fixtureModels()
			fallback := "fallback"
			switch name {
			case "missing_model":
				models = models[1:]
			case "duplicate_model":
				models = append(models, models[0])
			case "missing_messages":
				models[0].Messages = nil
			case "missing_template":
				models[0].Messages.Template = nil
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
			}
			if _, err := renderSnapshots(marshalModels(t, models), fallback); err == nil {
				t.Fatal("invalid source produced snapshots")
			}
		})
	}
}

func fixtureModels() []model {
	var models []model
	for slug := range modelFiles {
		template := "  {{ personality }}\n"
		models = append(models, model{
			Slug: slug,
			Messages: &modelMessages{
				Template: &template,
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
