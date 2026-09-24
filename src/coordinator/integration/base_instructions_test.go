package integration

import (
	"bytes"
	"fmt"
	"net/http"
	"strings"
	"testing"
)

type baseInstructionCase struct {
	subscription, codex, lite bool
	supplied                  bool
	value                     any
	expected                  string
}

func runBaseInstructionCases(t *testing.T, run func(*testing.T, baseInstructionCase)) {
	t.Helper()
	for _, subscription := range []bool{false, true} {
		for _, codex := range []bool{false, true} {
			for _, lite := range []bool{false, true} {
				for _, base := range []struct {
					name     string
					supplied bool
					value    any
					expected string
				}{
					{name: "missing"},
					{name: "null", supplied: true},
					{name: "empty", supplied: true, value: ""},
					{name: "whitespace", supplied: true, value: " \t\r\n\u00a0\u3000", expected: " \t\r\n\u00a0\u3000"},
					{name: "number", supplied: true, value: 42},
					{name: "boolean", supplied: true, value: false},
					{name: "array", supplied: true, value: []any{"not instructions"}},
					{name: "object", supplied: true, value: map[string]any{"text": "not instructions"}},
					{name: "explicit", supplied: true, value: "  Caller {{literal}} 基础\n", expected: "  Caller {{literal}} 基础\n"},
				} {
					t.Run(fmt.Sprintf("subscription=%t/codex=%t/lite=%t/%s", subscription, codex, lite, base.name), func(t *testing.T) {
						run(t, baseInstructionCase{subscription, codex, lite, base.supplied, base.value, base.expected})
					})
				}
			}
		}
	}
}

func (test baseInstructionCase) headers() http.Header {
	headers := make(http.Header)
	if test.codex {
		headers.Set("Originator", "codex_exec")
	}
	return headers
}

func (test baseInstructionCase) body(t *testing.T, ws bool, previous string) []byte {
	t.Helper()
	model := "gpt-5.4"
	if test.lite {
		model = "gpt-5.6-sol"
	}
	input := []any{
		map[string]any{"type": "message", "id": "msg_caller_rule", "role": "developer", "content": "caller rule"},
		map[string]any{"type": "message", "role": "system", "content": "caller system"},
		map[string]any{"type": "message", "role": "user", "content": "synthetic question " + t.Name()},
	}
	if previous != "" {
		input = []any{map[string]any{"type": "message", "role": "user", "content": "synthetic continuation"}}
	}
	request := map[string]any{"model": model, "input": input, "tools": []any{}, "stream": ws}
	if previous != "" {
		request["previous_response_id"] = previous
	}
	if test.supplied {
		request["instructions"] = test.value
	}
	if ws {
		request["type"] = "response.create"
	}
	return append(append([]byte(" \n"), mustRequestJSON(t, request)...), '\n')
}

func TestBaseInstructionsHTTPCallerOnly(t *testing.T) {
	fixture := newResponsesProfileHTTPFixture(t)
	runBaseInstructionCases(t, func(t *testing.T, test baseInstructionCase) {
		key := fixture.apiKey
		if test.subscription {
			key = fixture.subscriptionKey
		}
		previous, toolID := "", ""
		for turn := 0; turn < 2; turn++ {
			body := test.body(t, false, previous)
			status, response, _ := publicRequestWithHeaders(t, fixture.public, key, string(body), test.headers())
			if status != http.StatusOK {
				t.Fatalf("base-policy HTTP status %d", status)
			}
			capture := waitForRoutingCapture(t, fixture.captures)
			if !test.subscription {
				if !bytes.Equal(capture.Body, body) {
					t.Fatal("API-key base request was rewritten")
				}
			} else {
				value := decodeRequestObject(t, capture.Body)
				if value["previous_response_id"] != nil {
					t.Fatal("HTTP continuation was not reconstructed")
				}
				current := assertCallerOnlyBase(t, value, test, true)
				if turn == 1 && current != toolID {
					t.Fatal("unchanged tools changed prefix ID")
				}
				toolID = current
			}
			previous = responseIDFromPublicJSON(t, response)
		}
	})
}

func TestBaseInstructionsWebSocketCallerOnly(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixture(t)
	runBaseInstructionCases(t, func(t *testing.T, test baseInstructionCase) {
		key := fixture.apiKey
		if test.subscription {
			key = fixture.subscriptionKey
		}
		connection := dialResponsesProfileWebSocket(t, fixture.public, key, test.headers())
		defer connection.CloseNow()
		previous := ""
		for turn := 0; turn < 2; turn++ {
			body := test.body(t, true, previous)
			writeE2EWebSocketText(t, connection, string(body))
			events := readResponsesProfileTerminalEvents(t, connection)
			hidden := turn == 0 && test.subscription && !test.codex
			captures := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1+boolToInt(hidden))
			for _, capture := range captures {
				if !test.subscription {
					if !bytes.Equal(capture.Frame, body) {
						t.Fatal("API-key base frame was rewritten")
					}
					continue
				}
				value := decodeRequestObject(t, capture.Frame)
				full := value["previous_response_id"] == nil
				assertCallerOnlyBase(t, value, test, full)
				if turn == 1 && full {
					t.Fatal("unchanged explicit continuation lost live WS reuse")
				}
			}
			previous = responseIDFromWebSocketEvents(t, events)
		}
	})
}

func assertCallerOnlyBase(t *testing.T, value map[string]any, test baseInstructionCase, full bool) string {
	t.Helper()
	input, ok := value["input"].([]any)
	if !ok {
		t.Fatal("base-policy input was not an array")
	}
	toolID := ""
	if test.lite {
		if _, present := value["instructions"]; present {
			t.Fatal("Lite retained top-level instructions")
		}
		if _, present := value["tools"]; present {
			t.Fatal("Lite retained top-level tools")
		}
		if full {
			if len(input) < 1 {
				t.Fatal("Lite tool prefix missing")
			}
			tools, _ := input[0].(map[string]any)
			definitions, ok := tools["tools"].([]any)
			toolID, _ = tools["id"].(string)
			if tools["type"] != "additional_tools" || tools["role"] != "developer" || !ok || len(definitions) != 0 || !isUUIDVersion(strings.TrimPrefix(toolID, "at_"), '5') {
				t.Fatal("empty Lite tools or deterministic ID changed")
			}
			input = input[1:]
			if test.expected != "" {
				if len(input) == 0 {
					t.Fatal("explicit Lite base missing")
				}
				assertDeveloperMessageText(t, input[0], test.expected)
				base, _ := input[0].(map[string]any)
				id, _ := base["id"].(string)
				if !isUUIDVersion(strings.TrimPrefix(id, "msg_"), '5') {
					t.Fatal("explicit Lite base ID is not deterministic")
				}
				input = input[1:]
			}
		}
	} else {
		base, present := value["instructions"]
		if (test.expected == "" && present) || (test.expected != "" && base != test.expected) {
			t.Fatal("ordinary base was invented or caller text changed")
		}
	}
	prewarm := value["generate"] == false
	if prewarm && !test.lite {
		if len(input) != 0 {
			t.Fatal("ordinary hidden setup unexpectedly included business history")
		}
		return toolID
	}
	if full {
		minimum := 3
		if prewarm {
			minimum = 2 // Lite setup warms the leading caller developer messages.
		}
		if len(input) < minimum || (prewarm && len(input) != minimum) {
			t.Fatal("original caller history missing")
		}
		assertDeveloperMessageText(t, input[0], "caller rule")
		assertDeveloperMessageText(t, input[1], "caller system")
		rule, _ := input[0].(map[string]any)
		id, _ := rule["id"].(string)
		if !isUUIDVersion(strings.TrimPrefix(id, "msg_"), '7') {
			t.Fatal("caller rule was treated as a synthesized base")
		}
	}
	for _, raw := range input {
		item, _ := raw.(map[string]any)
		if item["type"] == "additional_tools" {
			t.Fatal("duplicate Lite prefix")
		}
		if item["role"] == "developer" {
			text, ok := developerMessageText(item)
			if !ok || (text != "caller rule" && text != "caller system") {
				t.Fatal("unrequested developer instructions inserted")
			}
		}
	}
	return toolID
}
