package integration

import (
	"fmt"
	"net/http"
	"reflect"
	"testing"
)

func TestOutputSchemaStrictPreservesExplicitValues(t *testing.T) {
	for _, ws := range []bool{false, true} {
		for _, role := range []struct {
			name, model, guardian, turn string
			defaultStrict               bool
		}{
			{"ordinary", "gpt-5.5", "", "", true},
			{"lite", "gpt-6-astra", "", "", true},
			{"reviewer", "gpt-5.5", "reviewer", "", false},
			{"lite_reviewer", "gpt-6-astra", "", `{"thread_source":"guardian_review"}`, false},
			{"backend_reviewer", "codex-auto-review", "reviewer", "", false},
		} {
			t.Run(fmt.Sprintf("ws=%t/%s", ws, role.name), func(t *testing.T) {
				peer := newFidelityPeer(t, ws)
				headers := http.Header{}
				if role.guardian != "" {
					headers.Set("X-Codex-Guardian", role.guardian)
				}
				for _, strict := range []struct {
					name    string
					present bool
					value   any
				}{
					{"missing", false, nil},
					{"false", true, false},
					{"true", true, true},
					{"null", true, nil},
					{"string", true, "false"},
					{"number", true, 0},
					{"array", true, []any{}},
					{"object", true, map[string]any{}},
				} {
					t.Run(strict.name, func(t *testing.T) {
						want := role.defaultStrict
						explicit, isBoolean := strict.value.(bool)
						if isBoolean {
							want = explicit
						}
						schema := guardianAssessmentSchema()
						if want {
							schema["required"] = []any{"risk_level", "user_authorization", "outcome", "rationale"}
						}
						format := map[string]any{"type": "json_schema", "name": "codex_output_schema", "schema": schema}
						if strict.present {
							format["strict"] = strict.value
						}
						body := fidelityBody(fidelityUser())
						body["model"] = role.model
						body["text"] = map[string]any{"format": format}
						if role.turn != "" {
							body["client_metadata"] = map[string]any{"x-codex-turn-metadata": role.turn}
						}
						wire, _, response := peer.send(body, headers)
						assertOutputSchemaStrict(t, wire, want, schema)
						if isBoolean {
							// Current explicit controls must also survive HTTP/reconnected-WS reconstruction.
							body["previous_response_id"] = response["id"]
							wire, _, _ = peer.send(body, headers)
							assertOutputSchemaStrict(t, wire, want, schema)
						}
					})
				}
			})
		}
	}
}

func assertOutputSchemaStrict(t *testing.T, wire map[string]any, want bool, schema map[string]any) {
	t.Helper()
	text, _ := wire["text"].(map[string]any)
	format, _ := text["format"].(map[string]any)
	if format["strict"] != want {
		t.Fatalf("output strict changed: got %v, want %t", format["strict"], want)
	}
	if !reflect.DeepEqual(format["schema"], schema) {
		t.Fatal("output schema changed with strictness policy")
	}
}
