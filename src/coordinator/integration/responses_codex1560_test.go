package integration

import (
	"net/http"
	"testing"
)

func TestCodex1560GuardianReferenceAndHeaders(t *testing.T) {
	fixture := newResponsesProfileHTTPFixture(t)
	status, response, _ := publicRequest(t, fixture.public, fixture.subscriptionKey, `{"model":"gpt-5.4","input":"synthetic parent"}`)
	if status != http.StatusOK {
		t.Fatal("parent request failed")
	}
	parent := waitForRoutingCapture(t, fixture.captures)
	publicID := responseIDFromPublicJSON(t, response)
	body := mustRequestJSON(t, map[string]any{"model": "codex-auto-review", "input": "synthetic review",
		"service_tier": "priority", "client_metadata": map[string]any{"parent_response_id": publicID}})
	status, _, _ = publicRequestWithHeaders(t, fixture.public, fixture.subscriptionKey, string(body), http.Header{"X-Codex-Guardian": []string{"reviewer"}})
	if status != http.StatusOK {
		t.Fatal("Guardian request failed")
	}
	review := waitForRoutingCapture(t, fixture.captures)
	request := decodeRequestObject(t, review.Body)
	if review.Headers.Get("X-Codex-Guardian") != "reviewer" {
		t.Fatal("Guardian routing header lost")
	}
	if request["service_tier"] != nil || review.Headers.Get("X-Codex-Routing-Hint") != "" {
		t.Fatal("Guardian request retained ordinary service routing")
	}
	metadata, _ := request["client_metadata"].(map[string]any)
	if metadata["parent_response_id"] != parent.ResponseID {
		t.Fatal("Guardian parent response was not restored")
	}
}
