package integration

import (
	"fmt"
	"net/http"
	"reflect"
	"testing"
)

// Codex 0.159.2 ext/guardian-reviewer/src/assessment.rs intentionally requires
// only outcome. Its basic review session always sends this schema as non-strict.
func guardianAssessmentSchema() map[string]any {
	return map[string]any{
		"type": "object", "additionalProperties": false,
		"properties": map[string]any{
			"risk_level":         map[string]any{"type": "string", "enum": []any{"low", "medium", "high", "critical"}},
			"user_authorization": map[string]any{"type": "string", "enum": []any{"unknown", "low", "medium", "high"}},
			"outcome":            map[string]any{"type": "string", "enum": []any{"allow", "deny"}},
			"rationale":          map[string]any{"type": "string"},
		},
		"required": []any{"outcome"},
	}
}

func guardianSchemaRequest() map[string]any {
	body := fidelityBody(fidelityUser())
	body["model"] = "gpt-5.6-luna"
	body["service_tier"] = "priority"
	body["text"] = map[string]any{"format": map[string]any{
		"type": "json_schema", "name": "codex_output_schema", "strict": false,
		"schema": guardianAssessmentSchema(),
	}}
	return body
}

func TestGuardianOutputSchemaWithoutBackendHeader(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, model := range []string{"gpt-5.6-luna", "gpt-5.5", "codex-auto-review"} {
			for _, carrier := range []string{"body", "header"} {
				for _, marker := range []string{"thread_source", "turn_trigger"} {
					t.Run(fmt.Sprintf("ws=%t/%s/%s/%s", ws, model, carrier, marker), func(t *testing.T) {
						peer := newFidelityPeer(t, ws)
						body := guardianSchemaRequest()
						body["model"] = model
						metadata := string(mustRequestJSON(t, map[string]any{marker: "guardian_review", "request_kind": "turn"}))
						headers := http.Header{"X-Openai-Subagent": []string{"guardian"}}
						if carrier == "body" {
							body["client_metadata"] = map[string]any{"x-codex-turn-metadata": metadata}
						} else {
							headers.Set("X-Codex-Turn-Metadata", metadata)
						}
						for attempt := 0; attempt < 2; attempt++ {
							wire, emitted, _ := peer.send(body, headers)
							format := wire["text"].(map[string]any)["format"].(map[string]any)
							if format["strict"] != false {
								t.Fatal("Guardian strict=false was overridden; optional risk_level would fail upstream schema validation")
							}
							if format["name"] != "codex_output_schema" || !reflect.DeepEqual(format["schema"], guardianAssessmentSchema()) {
								t.Fatal("Guardian output schema changed")
							}
							if wire["model"] != model {
								t.Fatal("reviewer selected model changed")
							}
							if model == "codex-auto-review" {
								if emitted.Get("X-Codex-Guardian") != "reviewer" || wire["service_tier"] != nil || emitted.Get("X-Codex-Routing-Hint") != "" {
									t.Fatal("Subscription reviewer did not select its backend route")
								}
							} else if emitted.Get("X-Codex-Guardian") != "" || wire["service_tier"] != "priority" {
								t.Fatal("reviewer model override acquired backend reviewer routing policy")
							}
						}
					})
				}
			}
		}
	}
}

func TestGuardianOutputSchemaRoleBoundaries(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, tc := range []struct {
			name       string
			bodyTurn   string
			headerTurn string
			guardian   string
			wantStrict bool
		}{
			{"ordinary", "", "", "", true},
			{"unrelated_source", `{"thread_source":"agent"}`, "", "", true},
			{"similar_source", `{"thread_source":"guardian_review_other"}`, "", "", true},
			{"similar_classifier", `{"thread_source":"guardian_classifier_other"}`, "", "", true},
			{"body_precedence", `{"thread_source":"agent"}`, `{"thread_source":"guardian_review"}`, "", true},
			{"backend_reviewer", "", "", "reviewer", false},
			{"classifier_header", `{"thread_source":"guardian_review"}`, "", "classifier", false},
		} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, tc.name), func(t *testing.T) {
				peer := newFidelityPeer(t, ws)
				body := guardianSchemaRequest()
				if tc.name == "backend_reviewer" {
					body["model"] = "codex-auto-review"
				}
				if tc.bodyTurn != "" {
					body["client_metadata"] = map[string]any{"x-codex-turn-metadata": tc.bodyTurn}
				}
				headers := http.Header{"X-Openai-Subagent": []string{"guardian"}}
				if tc.headerTurn != "" {
					headers.Set("X-Codex-Turn-Metadata", tc.headerTurn)
				}
				if tc.guardian != "" {
					headers.Set("X-Codex-Guardian", tc.guardian)
				}
				wire, _, _ := peer.send(body, headers)
				if tc.guardian == "classifier" {
					if wire["text"] != nil || wire["tool_choice"] != "none" {
						t.Fatal("review metadata overrode explicit classifier policy")
					}
					return
				}
				format := wire["text"].(map[string]any)["format"].(map[string]any)
				if format["strict"] != tc.wantStrict {
					t.Fatal("unrelated or conflicting metadata changed output strictness")
				}
				if tc.guardian == "reviewer" && wire["service_tier"] != nil {
					t.Fatal("backend reviewer retained service tier")
				}
			})
		}
	}
}
