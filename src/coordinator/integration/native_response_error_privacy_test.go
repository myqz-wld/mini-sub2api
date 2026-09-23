//go:build nativeparity

package integration

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"sync/atomic"
	"testing"
)

// A new native fatal/context/quota code must never silently turn into the CLI's generic
// retryable category because a gateway privacy allowlist forgot it. Derive this oracle from
// the independent pinned consumer, excluding its unrelated event/metadata cases and tests.
func TestNativeResponsePrivacyPreservesErrorCategories(t *testing.T) {
	source, err := os.ReadFile(filepath.Join(nativeSource(t), "codex-rs/codex-api/src/sse/responses.rs"))
	if err != nil {
		t.Fatal("read pinned native error classifications")
	}
	text := string(source)
	start := strings.Index(text, `"response.failed" =>`)
	end := strings.Index(text, "#[cfg(test)]\nmod tests {")
	if start < 0 || end <= start {
		t.Fatal("native error classification boundary moved")
	}
	groups := regexp.MustCompile(`Some\(\s*("[a-z_]+"(?:\s*\|\s*"[a-z_]+")*)\s*\)`)
	literal := regexp.MustCompile(`"([a-z_]+)"`)
	var codes []string
	for _, group := range groups.FindAllStringSubmatch(text[start:end], -1) {
		for _, code := range literal.FindAllStringSubmatch(group[1], -1) {
			if !slices.Contains(codes, code[1]) {
				codes = append(codes, code[1])
			}
		}
	}
	if len(codes) < 10 || !slices.Contains(codes, "invalid_prompt") {
		t.Fatal("native error oracle lost classifications")
	}
	slices.Sort(codes)
	var selected atomic.Value
	selected.Store("")
	fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
		id := fmt.Sprintf("resp_error_category_%d", loopbackResponseSequence.Add(1))
		event := privacyFailure(id)
		failure := event["response"].(map[string]any)["error"].(map[string]any)
		failure["code"] = selected.Load().(string)
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = fmt.Fprintf(w, "data: %s\n\n", mustRequestJSONValue(event))
		return id, "synthetic-error-category-request"
	})
	for _, code := range codes {
		t.Run(code, func(t *testing.T) {
			selected.Store(code)
			status, body, _ := publicRequestWithHeaders(t, fixture.public, fixture.subscriptionKey, `{"model":"gpt-5.4","input":[],"stream":true}`, nil)
			_ = waitForRoutingCapture(t, fixture.captures)
			if status != http.StatusOK {
				t.Fatal("native error fixture failed")
			}
			response := terminalHTTPResponse(t, body, true, "response.failed")
			failure, _ := response["error"].(map[string]any)
			if failure["code"] != code || failure["message"] != "The upstream request failed." || len(failure) != 2 {
				t.Fatal("native error category changed or provider details escaped")
			}
		})
	}
}
