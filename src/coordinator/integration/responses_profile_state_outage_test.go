package integration

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/coder/websocket"

	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestCodexProfilesFailSafeBeforeUpstreamWhenRequestStateIsCorrupt(t *testing.T) {
	profiles := []struct {
		name         string
		subscription bool
	}{
		{name: "codex_api_key"},
		{name: "codex_subscription", subscription: true},
	}
	for _, profile := range profiles {
		t.Run(profile.name, func(t *testing.T) {
			fixture := newResponsesProfileHTTPFixture(t)
			secret, keyID := fixture.apiKey, fixture.apiKeyID
			headers := http.Header{"Originator": []string{"codex_exec"}}
			if profile.subscription {
				secret, keyID = fixture.subscriptionKey, fixture.subscriptionKeyID
				headers = nil
			}
			initial := `{"model":"gpt-5.4","input":[{"type":"message","id":"msg-state-initial","role":"user","content":"initial"}],"stream":false}`
			status, _, _ := publicRequestWithHeaders(t, fixture.public, secret, initial, headers)
			if status != http.StatusOK {
				t.Fatalf("initial state materialization = %d", status)
			}
			_ = waitForRoutingCapture(t, fixture.captures)
			stateFiles, err := filepath.Glob(filepath.Join(
				fixture.coreStateDir, "accounts", "rs_*.request-state.json",
			))
			if err != nil || len(stateFiles) != 1 {
				t.Fatalf("request-state files = %#v, %v", stateFiles, err)
			}
			if err := os.WriteFile(stateFiles[0], []byte("{corrupt"), 0o600); err != nil {
				t.Fatal(err)
			}

			failed := `{"model":"gpt-5.4","input":[{"type":"message","id":"msg-state-failed","role":"user","content":"failed"}],"stream":false}`
			status, publicBody, publicHeaders := publicRequestWithHeaders(
				t, fixture.public, secret, failed, headers,
			)
			if status != http.StatusServiceUnavailable {
				t.Fatalf("state outage response = %d %s", status, publicBody)
			}
			envelope := decodeRequestObject(t, []byte(publicBody))
			errorObject, _ := envelope["error"].(map[string]any)
			if errorObject["code"] != "state_unavailable" ||
				errorObject["retryAdvice"] != "safe" || errorObject["phase"] != "internal" ||
				errorObject["deliveryState"] != "not_delivered" {
				t.Fatalf("state outage failure = %#v", errorObject)
			}
			select {
			case capture := <-fixture.captures:
				t.Fatalf("state outage reached upstream: %#v", capture)
			case <-time.After(200 * time.Millisecond):
			}
			requestID := publicHeaders.Get("X-Mini-Sub2Api-Request-Id")
			record := waitForProfileRequestRecord(t, fixture.store, keyID, requestID)
			if record.ProviderRequestID != nil || record.Status != storage.RequestUpstreamErr {
				t.Fatalf("state outage history = %#v", record)
			}
		})
	}
}

func TestCodexProfilesCloseFirstWebSocketCreateSafelyWhenRequestStateIsCorrupt(t *testing.T) {
	profiles := []struct {
		name         string
		subscription bool
	}{
		{name: "codex_api_key"},
		{name: "codex_subscription", subscription: true},
	}
	for _, profile := range profiles {
		t.Run(profile.name, func(t *testing.T) {
			fixture := newResponsesProfileHTTPFixture(t)
			secret, keyID := fixture.apiKey, fixture.apiKeyID
			headers := http.Header{"Originator": []string{"codex_exec"}}
			if profile.subscription {
				secret, keyID = fixture.subscriptionKey, fixture.subscriptionKeyID
				headers = nil
			}
			status, _, _ := publicRequestWithHeaders(
				t, fixture.public, secret,
				`{"model":"gpt-5.4","input":[],"conversation":"ws-state-seed","stream":false}`,
				headers,
			)
			if status != http.StatusOK {
				t.Fatalf("state seed = %d", status)
			}
			_ = waitForRoutingCapture(t, fixture.captures)
			stateFiles, err := filepath.Glob(filepath.Join(
				fixture.coreStateDir, "accounts", "rs_*.request-state.json",
			))
			if err != nil || len(stateFiles) != 1 {
				t.Fatalf("request-state files = %#v, %v", stateFiles, err)
			}
			if err := os.WriteFile(stateFiles[0], []byte("{corrupt"), 0o600); err != nil {
				t.Fatal(err)
			}

			connection := dialResponsesProfileWebSocket(t, fixture.public, secret, headers)
			defer connection.CloseNow()
			writeE2EWebSocketText(
				t, connection,
				`{"type":"response.create","model":"gpt-5.4","conversation":"ws-state-failed","input":[]}`,
			)
			readContext, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			_, _, readErr := connection.Read(readContext)
			cancel()
			if websocket.CloseStatus(readErr) != websocket.StatusCode(protocolv1.FailureCloseCode) {
				t.Fatalf("state outage close = %v", readErr)
			}
			var closeError websocket.CloseError
			if !errors.As(readErr, &closeError) {
				t.Fatalf("state outage close metadata is unavailable: %v", readErr)
			}
			var failure protocolv1.FailureMetadata
			if json.Unmarshal([]byte(closeError.Reason), &failure) != nil ||
				failure.RetryAdvice != protocolv1.RetrySafe || failure.Phase != protocolv1.PhaseInternal ||
				failure.DeliveryState != protocolv1.DeliveryNotDelivered {
				t.Fatalf("state outage failure = %#v / %q", failure, closeError.Reason)
			}
			select {
			case capture := <-fixture.captures:
				t.Fatalf("first state-failing WebSocket create reached upstream: %#v", capture)
			case <-time.After(200 * time.Millisecond):
			}
			deadline := time.Now().Add(2 * time.Second)
			for {
				records, err := fixture.store.History(context.Background(), keyID, nil, 100)
				if err != nil {
					t.Fatal(err)
				}
				var websocketRecord *storage.RequestRecord
				for index := range records {
					record := &records[index]
					if record.Transport == storage.TransportWebSocket {
						websocketRecord = record
						break
					}
				}
				if websocketRecord != nil && websocketRecord.Status != storage.RequestInProgress {
					if websocketRecord.Status != storage.RequestUpstreamErr ||
						websocketRecord.ProviderRequestID != nil {
						t.Fatalf("state outage WebSocket history = %#v", websocketRecord)
					}
					break
				}
				if time.Now().After(deadline) {
					t.Fatalf("state outage WebSocket history did not finalize: %#v", records)
				}
				time.Sleep(10 * time.Millisecond)
			}
		})
	}
}
