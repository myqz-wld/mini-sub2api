package integration

import (
	"context"
	"net/http"
	"strings"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func responseIDFromPublicJSON(t *testing.T, body string) string {
	t.Helper()
	response := decodeRequestObject(t, []byte(body))
	responseID, _ := response["id"].(string)
	if responseID == "" {
		t.Fatalf("public response omitted id: %s", body)
	}
	return responseID
}

func responseIDFromPublicSSE(t *testing.T, body string) string {
	t.Helper()
	for _, line := range strings.Split(body, "\n") {
		payload, ok := strings.CutPrefix(line, "data: ")
		if !ok || payload == "[DONE]" {
			continue
		}
		event := decodeRequestObject(t, []byte(payload))
		response, _ := event["response"].(map[string]any)
		if responseID, _ := response["id"].(string); responseID != "" {
			return responseID
		}
	}
	t.Fatalf("public SSE omitted response id: %s", body)
	return ""
}

func assertProfileHTTPResponsePrivacy(
	t *testing.T,
	store *storage.Store,
	keyID string,
	headers http.Header,
	providerRequestID string,
) {
	t.Helper()
	gatewayRequestID := headers.Get("X-Mini-Sub2Api-Request-Id")
	if gatewayRequestID == "" || headers.Get("X-Request-Id") != gatewayRequestID ||
		headers.Get("Openai-Request-Id") != gatewayRequestID {
		t.Fatalf("public provider request aliases = %#v", headers)
	}
	if headers.Get("Openai-Model") != "gpt-loopback" ||
		!strings.Contains(headers.Get("Server-Timing"), "provider;dur=2") ||
		!strings.Contains(headers.Get("Server-Timing"), "upstream_ttfb;dur=") {
		t.Fatalf("safe provider response headers = %#v", headers)
	}
	for _, name := range []string{
		protocolv1.ProviderRequestIDHeader,
		"X-Codex-Installation-Id",
		"X-Unrecognized-Provider-Extension",
	} {
		if headers.Get(name) != "" {
			t.Fatalf("private response header %s crossed: %#v", name, headers)
		}
	}
	record := waitForProfileRequestRecord(t, store, keyID, gatewayRequestID)
	if record.ProviderRequestID == nil || *record.ProviderRequestID != providerRequestID {
		t.Fatalf("provider request diagnostic history = %#v", record)
	}
}

func waitForProfileRequestRecord(
	t *testing.T,
	store *storage.Store,
	keyID, requestID string,
) storage.RequestRecord {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for {
		records, err := store.History(context.Background(), keyID, nil, 100)
		if err != nil {
			t.Fatal(err)
		}
		for _, record := range records {
			if record.RequestID == requestID && record.Status != storage.RequestInProgress {
				return record
			}
		}
		if time.Now().After(deadline) {
			t.Fatalf("request history %s was not finalized: %#v", requestID, records)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

type persistedProfileIdentity struct {
	publicResponseID   string
	providerResponseID string
	sessionID          string
	threadID           string
	installationID     string
}

func TestCodexProfilesTranslateStreamingAndAggregatedResponses(t *testing.T) {
	fixture := newResponsesProfileHTTPFixture(t)
	profiles := []struct {
		name    string
		secret  string
		keyID   string
		headers http.Header
	}{
		{
			name: "codex_api_key", secret: fixture.apiKey, keyID: fixture.apiKeyID,
			headers: http.Header{"Originator": []string{"codex_exec"}},
		},
		{
			name: "codex_subscription", secret: fixture.subscriptionKey,
			keyID: fixture.subscriptionKeyID,
		},
	}
	for _, profile := range profiles {
		for _, stream := range []bool{false, true} {
			name := "aggregated"
			if stream {
				name = "streaming"
			}
			t.Run(profile.name+"_"+name, func(t *testing.T) {
				body := mustRequestJSON(t, map[string]any{
					"model": "gpt-5.4", "stream": stream,
					"input": []any{map[string]any{
						"type": "message", "id": "msg-" + profile.name + "-" + name,
						"role": "user", "content": name,
					}},
					"client_metadata": map[string]any{
						"session_id": "session-" + profile.name + "-" + name,
					},
				})
				status, publicBody, publicHeaders := publicRequestWithHeaders(
					t, fixture.public, profile.secret, string(body), profile.headers,
				)
				if status != http.StatusOK {
					t.Fatalf("public response = %d %s", status, publicBody)
				}
				capture := waitForRoutingCapture(t, fixture.captures)
				assertProfileHTTPResponsePrivacy(
					t, fixture.store, profile.keyID, publicHeaders, capture.ProviderRequestID,
				)
				var publicResponseID string
				if stream {
					publicResponseID = responseIDFromPublicSSE(t, publicBody)
				} else {
					publicResponseID = responseIDFromPublicJSON(t, publicBody)
				}
				if publicResponseID == capture.ResponseID {
					t.Fatal("provider response ID crossed the stateful profile")
				}
			})
		}
	}
}

func TestProfilesEnforceNon2xxBodyAndHeaderPrivacyBoundaries(t *testing.T) {
	fixture := newResponsesProfileHTTPFixture(t)
	profiles := []struct {
		name     string
		secret   string
		keyID    string
		headers  http.Header
		stateful bool
	}{
		{name: "bare_api_key", secret: fixture.apiKey, keyID: fixture.apiKeyID},
		{
			name: "codex_api_key", secret: fixture.apiKey, keyID: fixture.apiKeyID,
			headers: http.Header{"Originator": []string{"codex_exec"}}, stateful: true,
		},
		{
			name: "codex_subscription", secret: fixture.subscriptionKey,
			keyID: fixture.subscriptionKeyID, stateful: true,
		},
	}
	for _, profile := range profiles {
		t.Run(profile.name, func(t *testing.T) {
			body := `{"model":"profile-non2xx","input":[],"stream":true}`
			status, publicBody, publicHeaders := publicRequestWithHeaders(
				t, fixture.public, profile.secret, body, profile.headers,
			)
			if status != http.StatusTooManyRequests {
				t.Fatalf("public non-2xx = %d %s", status, publicBody)
			}
			capture := waitForRoutingCapture(t, fixture.captures)
			assertProfileHTTPResponsePrivacy(
				t, fixture.store, profile.keyID, publicHeaders, capture.ProviderRequestID,
			)
			raw := "raw provider response_id=" + capture.ResponseID +
				" conversation=conv_private request=request_private"
			if !profile.stateful {
				if publicBody != raw || !strings.HasPrefix(publicHeaders.Get("Content-Type"), "text/plain") {
					t.Fatalf("Bare non-2xx response changed = %q / %#v", publicBody, publicHeaders)
				}
				return
			}
			if strings.Contains(publicBody, capture.ResponseID) || strings.Contains(publicBody, "conv_private") ||
				strings.Contains(publicBody, "request_private") {
				t.Fatalf("raw stateful non-2xx body crossed: %q", publicBody)
			}
			errorEnvelope := decodeRequestObject(t, []byte(publicBody))
			errorObject, _ := errorEnvelope["error"].(map[string]any)
			if errorObject["code"] != "upstream_response_failed" ||
				errorObject["retryAdvice"] != "never" ||
				errorObject["phase"] != "upstream_response" ||
				errorObject["deliveryState"] != "delivered" {
				t.Fatalf("normalized non-2xx failure = %#v", errorObject)
			}
		})
	}
}

func TestCodexProfilesTranslateStreamingAndAggregatedTerminalFailures(t *testing.T) {
	fixture := newResponsesProfileHTTPFixture(t)
	profiles := []struct {
		name    string
		secret  string
		keyID   string
		headers http.Header
	}{
		{
			name: "codex_api_key", secret: fixture.apiKey, keyID: fixture.apiKeyID,
			headers: http.Header{"Originator": []string{"codex_exec"}},
		},
		{
			name: "codex_subscription", secret: fixture.subscriptionKey,
			keyID: fixture.subscriptionKeyID,
		},
	}
	for _, profile := range profiles {
		for _, eventType := range []string{"response.failed", "response.incomplete"} {
			for _, stream := range []bool{false, true} {
				mode := "aggregated"
				if stream {
					mode = "streaming"
				}
				t.Run(profile.name+"_"+eventType+"_"+mode, func(t *testing.T) {
					body := mustRequestJSON(t, map[string]any{
						"model": eventType, "input": []any{}, "stream": stream,
						"conversation": profile.name + "-" + eventType + "-" + mode,
					})
					status, publicBody, publicHeaders := publicRequestWithHeaders(
						t, fixture.public, profile.secret, string(body), profile.headers,
					)
					if status != http.StatusOK {
						t.Fatalf("terminal failure response = %d %s", status, publicBody)
					}
					capture := waitForRoutingCapture(t, fixture.captures)
					assertProfileHTTPResponsePrivacy(
						t, fixture.store, profile.keyID, publicHeaders, capture.ProviderRequestID,
					)
					if publicHeaders.Get(protocolv1.ResponseTerminalHeader) != "" {
						t.Fatalf("private terminal header crossed: %#v", publicHeaders)
					}
					response := terminalHTTPResponse(t, publicBody, stream, eventType)
					if response["id"] == capture.ResponseID || response["id"] == "" {
						t.Fatalf("terminal provider response ID crossed: %#v", response)
					}
					errorObject, _ := response["error"].(map[string]any)
					if errorObject["message"] != "opaque resp_raw conversation_raw" ||
						errorObject["id"] != "opaque_nested_id" {
						t.Fatalf("opaque HTTP terminal error changed: %#v", errorObject)
					}
					record := waitForProfileRequestRecord(
						t, fixture.store, profile.keyID,
						publicHeaders.Get("X-Mini-Sub2Api-Request-Id"),
					)
					if record.Status != storage.RequestUpstreamErr {
						t.Fatalf("terminal history = %#v", record)
					}
				})
			}
		}
	}
}

func terminalHTTPResponse(
	t *testing.T,
	body string,
	stream bool,
	eventType string,
) map[string]any {
	t.Helper()
	if !stream {
		return decodeRequestObject(t, []byte(body))
	}
	for _, line := range strings.Split(body, "\n") {
		payload, ok := strings.CutPrefix(line, "data: ")
		if !ok {
			continue
		}
		event := decodeRequestObject(t, []byte(payload))
		if event["type"] != eventType {
			continue
		}
		response, _ := event["response"].(map[string]any)
		return response
	}
	t.Fatalf("streaming terminal event %s is missing: %s", eventType, body)
	return nil
}

func TestCodexProfilesRestoreResponseOwnershipAcrossCoreRestart(t *testing.T) {
	fixture := newResponsesProfileHTTPFixture(t)
	tests := []struct {
		name    string
		secret  string
		keyID   string
		headers http.Header
	}{
		{
			name: "codex_api_key", secret: fixture.apiKey, keyID: fixture.apiKeyID,
			headers: http.Header{"Originator": []string{"codex_exec"}},
		},
		{
			name: "codex_subscription", secret: fixture.subscriptionKey,
			keyID: fixture.subscriptionKeyID,
		},
	}
	snapshots := make(map[string]persistedProfileIdentity, len(tests))
	for _, test := range tests {
		body := mustRequestJSON(t, map[string]any{
			"model": "gpt-5.4", "stream": false,
			"input": []any{map[string]any{
				"type": "message", "id": "msg-conflict-" + test.name,
				"role": "user", "content": "first",
			}},
			"conversation":     "conversation-conflict-" + test.name,
			"prompt_cache_key": "cache-conflict-" + test.name,
			"client_metadata": map[string]any{
				"session_id": "session-conflict-" + test.name,
				"thread_id":  "thread-conflict-" + test.name,
				"turn_id":    "turn-conflict-" + test.name,
			},
		})
		status, publicBody, publicHeaders := publicRequestWithHeaders(
			t, fixture.public, test.secret, string(body), test.headers,
		)
		if status != http.StatusOK {
			t.Fatalf("%s first response = %d %s", test.name, status, publicBody)
		}
		capture := waitForRoutingCapture(t, fixture.captures)
		assertProfileHTTPResponsePrivacy(
			t, fixture.store, test.keyID, publicHeaders, capture.ProviderRequestID,
		)
		identity := identityFromProfileCapture(t, capture)
		if identity.sessionID != identity.threadID {
			t.Fatalf("%s conflicting root carriers did not converge: %#v", test.name, identity)
		}
		upstream := decodeRequestObject(t, capture.Body)
		if upstream["prompt_cache_key"] != identity.sessionID {
			t.Fatalf("%s prompt cache did not converge to the canonical root", test.name)
		}
		identity.publicResponseID = responseIDFromPublicJSON(t, publicBody)
		identity.providerResponseID = capture.ResponseID
		if identity.publicResponseID == identity.providerResponseID {
			t.Fatalf("%s provider response ID crossed publicly", test.name)
		}
		snapshots[test.name] = identity
	}

	fixture.restartRuntime(t)
	for _, test := range tests {
		first := snapshots[test.name]
		body := mustRequestJSON(t, map[string]any{
			"model": "gpt-5.4", "stream": false,
			"previous_response_id": first.publicResponseID,
			"input": []any{map[string]any{
				"type": "message", "id": "msg-after-restart-" + test.name,
				"role": "user", "content": "second",
			}},
		})
		status, publicBody, publicHeaders := publicRequestWithHeaders(
			t, fixture.public, test.secret, string(body), test.headers,
		)
		if status != http.StatusOK {
			t.Fatalf("%s second response = %d %s", test.name, status, publicBody)
		}
		capture := waitForRoutingCapture(t, fixture.captures)
		assertProfileHTTPResponsePrivacy(
			t, fixture.store, test.keyID, publicHeaders, capture.ProviderRequestID,
		)
		upstream := decodeRequestObject(t, capture.Body)
		if upstream["previous_response_id"] != first.providerResponseID {
			t.Fatalf("%s previous response owner was not restored: %#v", test.name, upstream)
		}
		second := identityFromProfileCapture(t, capture)
		if second.sessionID != first.sessionID || second.threadID != first.threadID ||
			second.installationID != first.installationID {
			t.Fatalf("%s identity changed across restart: %#v -> %#v", test.name, first, second)
		}
		if responseIDFromPublicJSON(t, publicBody) == capture.ResponseID {
			t.Fatalf("%s second provider response ID crossed publicly", test.name)
		}
	}
}

func identityFromProfileCapture(t *testing.T, capture routingMatrixCapture) persistedProfileIdentity {
	t.Helper()
	body := decodeRequestObject(t, capture.Body)
	metadata, _ := body["client_metadata"].(map[string]any)
	identity := persistedProfileIdentity{
		sessionID:      stringValue(metadata["session_id"]),
		threadID:       stringValue(metadata["thread_id"]),
		installationID: stringValue(metadata["x-codex-installation-id"]),
	}
	if !isUUIDVersion(identity.sessionID, '7') || !isUUIDVersion(identity.threadID, '7') ||
		!isUUIDVersion(identity.installationID, '4') || !isUUIDVersion(metadata["turn_id"], '7') {
		t.Fatalf("invalid projected identity = %#v / %#v", identity, metadata)
	}
	return identity
}

func stringValue(value any) string {
	text, _ := value.(string)
	return text
}
