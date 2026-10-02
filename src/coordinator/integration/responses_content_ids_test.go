package integration

import (
	"fmt"
	"strings"
	"testing"

	"github.com/google/uuid"
)

func contentIDRequest(session, text string) map[string]any {
	return map[string]any{
		"model": "gpt-5.4", "stream": false,
		"client_metadata": map[string]any{"session_id": session, "turn_id": "turn-" + session},
		"input": []any{
			map[string]any{"role": "user", "content": "synthetic opening"},
			map[string]any{"type": "message", "role": "assistant", "id": "msg_pi_1", "content": []any{map[string]any{"type": "output_text", "text": text}}},
			map[string]any{"type": "function_call", "id": "fc_pi_2", "call_id": "call_pi_1", "name": "probe", "arguments": fmt.Sprintf(`{"task":%q}`, text)},
			map[string]any{"type": "function_call_output", "id": "fco_pi_3", "call_id": "call_pi_1", "output": "synthetic result"},
		},
	}
}

func contentWireIDs(t *testing.T, wire map[string]any) [3]string {
	t.Helper()
	items, _ := wire["input"].([]any)
	if len(items) != 4 {
		t.Fatal("input shape changed")
	}
	message, _ := items[1].(map[string]any)
	call, _ := items[2].(map[string]any)
	result, _ := items[3].(map[string]any)
	if call["call_id"] != result["call_id"] {
		t.Fatal("tool call/result IDs differ")
	}
	return [3]string{message["id"].(string), call["id"].(string), call["call_id"].(string)}
}

func assertContentPseudonym(t *testing.T, value any, original, prefix string) {
	t.Helper()
	id, ok := value.(string)
	suffix, prefixed := strings.CutPrefix(id, prefix)
	parsed, err := uuid.Parse(suffix)
	if !ok || id == original || !prefixed || err != nil || parsed.Version() != 7 {
		t.Fatal("local pseudonym is not a distinct typed UUIDv7")
	}
}

func TestResponsesLocalIDsUseContentAndSessionPseudonymsHTTP(t *testing.T) {
	fixture := newResponsesProfileHTTPFixture(t)
	send := func(session, text string, subscription bool) [3]string {
		key := fixture.subscriptionKey
		if !subscription {
			key = fixture.apiKey
		}
		status, _, _ := publicRequestWithHeaders(t, fixture.public, key, string(mustRequestJSON(t, contentIDRequest(session, text))), nil)
		if status != 200 {
			t.Fatalf("synthetic request status=%d", status)
		}
		capture := waitForRoutingCapture(t, fixture.captures)
		return contentWireIDs(t, decodeRequestObject(t, capture.Body))
	}
	first := send("session-a", "synthetic task-a", true)
	for i, prefix := range []string{"msg_", "fc_", "call_"} {
		assertContentPseudonym(t, first[i], [3]string{"msg_pi_1", "fc_pi_2", "call_pi_1"}[i], prefix)
	}
	if replay := send("session-a", "synthetic task-a", true); replay != first {
		t.Fatal("same-session replay changed pseudonyms")
	}
	for _, changed := range [][2]string{{"session-b", "synthetic task-a"}, {"session-a", "synthetic task-b"}} {
		next := send(changed[0], changed[1], true)
		for i := range first {
			if next[i] == first[i] {
				t.Fatal("session/content change reused an upstream pseudonym")
			}
		}
	}
	if passthrough := send("session-a", "synthetic task-a", false); passthrough != [3]string{"msg_pi_1", "fc_pi_2", "call_pi_1"} {
		t.Fatal("API-key passthrough rewrote caller IDs")
	}
}
