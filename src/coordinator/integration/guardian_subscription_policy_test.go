package integration

import (
	"fmt"
	"net/http"
	"testing"
)

func TestGuardianRoleModelAndEffortPolicy(t *testing.T) {
	for _, ws := range []bool{false, true} {
		t.Run(fmt.Sprintf("ws=%t", ws), func(t *testing.T) {
			peer := newFidelityPeer(t, ws)
			parent := fidelityBody(fidelityUser())
			parent["client_metadata"] = map[string]any{"session_id": "policy-root", "thread_id": "policy-root", "turn_id": "policy-parent"}
			peer.send(parent, nil)
			for index, tc := range []struct {
				role, model, header string
				effort, wantEffort  any
				wantHeader          string
			}{
				{"guardian_classifier", "gpt-5.6-luna", "", nil, "low", "classifier"},
				{"guardian_classifier", "gpt-5.6-luna", "", false, "low", "classifier"},
				{"guardian_classifier", "codex-auto-review", "", "high", "high", "classifier"},
				{"guardian_classifier", "gpt-5.6-luna", "", "ultra", "ultra", "classifier"},
				{"guardian_classifier", "gpt-5.6-luna", "", "persistent", "persistent", "classifier"},
				{"guardian_classifier", "gpt-5.6-luna", "", "3072", float64(3072), "classifier"},
				{"guardian_classifier", "gpt-5.6-luna", "", "custom-effort", "custom-effort", "classifier"},
				{"guardian_review", "gpt-5.6-luna", "", nil, "low", ""},
				{"guardian_review", "codex-auto-review", "", nil, "low", "reviewer"},
				{"guardian_review", "codex-auto-review", "", "ultra", "max", "reviewer"},
				{"guardian_review", "gpt-5.6-luna", "reviewer", "high", "high", ""},
				{"guardian_review", "codex-auto-review", "reviewer", "high", "high", "reviewer"},
				{"guardian_review", "custom/codex-auto-review", "reviewer", "high", "high", ""},
				{"guardian_review", "codex-auto-review-preview", "reviewer", "high", "high", ""},
				{"", "gpt-5.6-luna", "reviewer", nil, "low", ""},
				{"", "codex-auto-review", "reviewer", nil, "low", "reviewer"},
				{"", "gpt-5.6-luna", "", nil, "medium", ""},
				{"", "codex-auto-review", "", nil, "medium", ""},
			} {
				body := guardianSchemaRequest()
				body["model"] = tc.model
				body["reasoning"] = map[string]any{"effort": tc.effort}
				headers := http.Header{}
				if tc.header != "" {
					headers.Set("X-Codex-Guardian", tc.header)
				}
				if tc.role != "" {
					turn := map[string]any{"thread_source": tc.role, "turn_trigger": tc.role}
					metadata := map[string]any{"x-openai-subagent": "guardian"}
					if tc.role == "guardian_classifier" {
						turn["session_id"], turn["thread_id"], turn["turn_id"] = "policy-root", fmt.Sprintf("classifier-%d", index), fmt.Sprintf("classification-%d", index)
						turn["guardian_classifier_source_thread_id"], turn["parent_turn_id"] = "policy-root", "policy-parent"
						metadata["session_id"], metadata["thread_id"], metadata["turn_id"] = turn["session_id"], turn["thread_id"], turn["turn_id"]
						metadata["parent_turn_id"] = turn["parent_turn_id"]
					}
					metadata["x-codex-turn-metadata"] = string(mustRequestJSON(t, turn))
					body["client_metadata"] = metadata
				}
				wire, emitted, _ := peer.send(body, headers)
				if emitted.Get("X-Codex-Guardian") != tc.wantHeader || wire["reasoning"].(map[string]any)["effort"] != tc.wantEffort {
					t.Fatalf("case %d: Guardian model/effort routing changed", index)
				}
				flat, _ := fidelityMetadata(t, wire)
				isReview := tc.role == "guardian_review" || tc.header == "reviewer"
				if (tc.role != "" || isReview) && flat["guardian_credits_requested"] != nil {
					t.Fatalf("case %d: review/classification acquired ordinary credit metadata", index)
				}
				if tc.role == "guardian_classifier" {
					if wire["model"] != "gpt-5.6-luna" || wire["tool_choice"] != "none" || wire["text"] != nil {
						t.Fatalf("case %d: classifier producer policy lost", index)
					}
				} else {
					if wire["model"] != tc.model {
						t.Fatalf("case %d: chosen model was replaced", index)
					}
					strict := wire["text"].(map[string]any)["format"].(map[string]any)["strict"]
					if strict != !isReview {
						t.Fatalf("case %d: review schema policy conflated with model identity", index)
					}
				}
			}
		})
	}
}

func TestGuardianRoleConflictRejectedBeforeUpstream(t *testing.T) {
	fixture := newResponsesProfileHTTPFixture(t)
	for _, markers := range [][2]string{{"guardian_classifier", "guardian_review"}, {"guardian_review", "guardian_classifier"}} {
		body := guardianSchemaRequest()
		body["client_metadata"] = map[string]any{"x-codex-turn-metadata": string(mustRequestJSON(t, map[string]any{"thread_source": markers[0], "turn_trigger": markers[1]}))}
		status, _, _ := publicRequest(t, fixture.public, fixture.subscriptionKey, string(mustRequestJSON(t, body)))
		if status != http.StatusBadRequest {
			t.Fatalf("conflicting roles status=%d", status)
		}
	}
	select {
	case <-fixture.captures:
		t.Fatal("conflicting roles reached upstream")
	default:
	}
	// API-key requests remain provider-owned, including metadata the Subscription parser rejects.
	body := guardianSchemaRequest()
	body["client_metadata"] = map[string]any{"x-codex-turn-metadata": `{"thread_source":"guardian_classifier","turn_trigger":"guardian_review"}`}
	status, _, _ := publicRequest(t, fixture.public, fixture.apiKey, string(mustRequestJSON(t, body)))
	if status != http.StatusOK {
		t.Fatalf("API-key passthrough status=%d", status)
	}
	capture := waitForRoutingCapture(t, fixture.captures)
	wire := decodeRequestObject(t, capture.Body)
	if capture.Headers.Get("X-Codex-Guardian") != "" || wire["service_tier"] != "priority" {
		t.Fatal("Subscription role policy affected API-key passthrough")
	}
}

func TestGuardianHeaderlessParentResponseRemainsKeyScoped(t *testing.T) {
	fixture := newResponsesProfileHTTPFixture(t)
	parent := fidelityBody(fidelityUser())
	parent["client_metadata"] = map[string]any{"session_id": "scope-root", "thread_id": "scope-root", "turn_id": "scope-parent"}
	status, reply, _ := publicRequest(t, fixture.public, fixture.subscriptionKey, string(mustRequestJSON(t, parent)))
	if status != http.StatusOK {
		t.Fatalf("parent setup status=%d", status)
	}
	capture := waitForRoutingCapture(t, fixture.captures)
	publicID := responseIDFromPublicJSON(t, reply)
	other := createDownstreamKey(t, fixture.store, fixture.subscriptionCredentialID, "Guardian scope control")
	for _, role := range []string{"guardian_classifier", "guardian_review"} {
		body := guardianSchemaRequest()
		body["model"] = "codex-auto-review"
		turn := map[string]any{"thread_source": role, "turn_trigger": role}
		metadata := map[string]any{"x-openai-subagent": "guardian", "parent_response_id": publicID}
		if role == "guardian_classifier" {
			body["model"] = "gpt-5.6-luna"
			turn["session_id"], turn["thread_id"], turn["turn_id"] = "scope-root", "scope-classifier", "scope-classification"
			turn["guardian_classifier_source_thread_id"], turn["parent_turn_id"] = "scope-root", "scope-parent"
			metadata["session_id"], metadata["thread_id"], metadata["turn_id"] = turn["session_id"], turn["thread_id"], turn["turn_id"]
			metadata["parent_turn_id"] = turn["parent_turn_id"]
		}
		metadata["x-codex-turn-metadata"] = string(mustRequestJSON(t, turn))
		body["client_metadata"] = metadata
		encoded := string(mustRequestJSON(t, body))
		status, _, _ = publicRequest(t, fixture.public, other.Secret, encoded)
		if status != http.StatusServiceUnavailable {
			t.Fatalf("cross-Key %s parent reference status=%d", role, status)
		}
		select {
		case <-fixture.captures:
			t.Fatal("cross-Key parent reference reached upstream")
		default:
		}
		status, _, _ = publicRequest(t, fixture.public, fixture.subscriptionKey, encoded)
		if status != http.StatusOK {
			t.Fatalf("own-Key %s parent reference status=%d", role, status)
		}
		wire := decodeRequestObject(t, waitForRoutingCapture(t, fixture.captures).Body)
		if wire["client_metadata"].(map[string]any)["parent_response_id"] != capture.ResponseID {
			t.Fatal("scoped parent response mapping lost")
		}
	}
}
