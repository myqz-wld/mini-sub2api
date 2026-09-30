package integration

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"testing"
	"time"

	"github.com/coder/websocket"
	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

const failureTailCreate = `{"type":"response.create","model":"gpt-5.5","input":[]}`

func TestWebSocketFailedFooterRecordsUsage(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		t.Run(failureTailRoute(subscription), func(t *testing.T) {
			fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(c *websocket.Conn, _ []byte, id string) {
				writeFailureTailEvent(c, "response.created", id, 0)
				writeFailureTailEvent(c, "error", "", 0)
				time.Sleep(40 * time.Millisecond)
				writeFailureTailEvent(c, "response.failed", id, 7)
			})
			c, keyID := dialFailureTail(t, fixture, subscription)
			defer c.CloseNow()
			writeE2EWebSocketText(t, c, failureTailCreate)
			created := readFailureTailEvent(t, c, "response.created")
			readFailureTailEvent(t, c, "error")
			failed := readFailureTailEvent(t, c, "response.failed")
			if failed["response"].(map[string]any)["id"] != created["response"].(map[string]any)["id"] ||
				terminalErrorObject(failed)["code"] != "invalid_prompt" {
				t.Fatal("failed footer lost response ownership or native category")
			}
			if subscription && terminalErrorObject(failed)["message"] == "private-synthetic" {
				t.Fatal("private provider error text crossed Subscription boundary")
			}
			history := waitForFailureTailHistory(t, fixture.store, keyID, 1)
			assertFailureTailRecord(t, history[0], storage.RequestUpstreamErr, 7)
			assertFailureTailStats(t, fixture.store, keyID, 1, 1, 7)
			waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)
		})
	}
}

func TestWebSocketFailureAllowsNewWorkOnlyOnNewConnection(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		t.Run(failureTailRoute(subscription), func(t *testing.T) {
			var failedID string
			fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(c *websocket.Conn, _ []byte, id string) {
				writeFailureTailEvent(c, "response.created", id, 0)
				if failedID == "" {
					failedID = id
					writeFailureTailEvent(c, "error", "", 0)
					return
				}
				writeFailureTailEvent(c, "response.completed", id, 11)
			})
			first, keyID := dialFailureTail(t, fixture, subscription)
			defer first.CloseNow()
			writeE2EWebSocketText(t, first, failureTailCreate)
			old := readFailureTailEvent(t, first, "response.created")["response"].(map[string]any)["id"]
			readFailureTailEvent(t, first, "error")
			assertFailureTailClose(t, first)
			second, _ := dialFailureTail(t, fixture, subscription)
			defer second.CloseNow()
			writeE2EWebSocketText(t, second, failureTailCreate)
			readFailureTailEvent(t, second, "response.created")
			completed := readFailureTailEvent(t, second, "response.completed")
			if completed["response"].(map[string]any)["id"] == old {
				t.Fatal("new connection reused failed response identity")
			}
			history := waitForFailureTailHistory(t, fixture.store, keyID, 2)
			for _, record := range history {
				if record.Status == storage.RequestUpstreamErr {
					if record.Usage != nil {
						t.Fatal("expired failed request acquired usage")
					}
				} else {
					assertFailureTailRecord(t, record, storage.RequestCompleted, 11)
				}
			}
			captures := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 2)
			if captures[0].ProviderRequestID == captures[1].ProviderRequestID {
				t.Fatal("failed physical connection was reused")
			}
			if decodeRequestObject(t, captures[1].Frame)["previous_response_id"] != nil {
				t.Fatal("failed response became a continuation baseline")
			}
		})
	}
}

func TestWebSocketErrorWithoutFooterNeverBecomesSafeReplay(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		t.Run(failureTailRoute(subscription), func(t *testing.T) {
			fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(c *websocket.Conn, _ []byte, id string) {
				writeFailureTailEvent(c, "response.created", id, 0)
				writeFailureTailEvent(c, "error", "", 0)
				// Expiry releases validation facts, not delivery evidence.
				time.Sleep(1200 * time.Millisecond)
				_ = c.CloseNow()
			})
			c, keyID := dialFailureTail(t, fixture, subscription)
			defer c.CloseNow()
			writeE2EWebSocketText(t, c, failureTailCreate)
			readFailureTailEvent(t, c, "response.created")
			readFailureTailEvent(t, c, "error")
			assertFailureTailClose(t, c)
			history := waitForFailureTailHistory(t, fixture.store, keyID, 1)
			if history[0].Status != storage.RequestUpstreamErr || history[0].Usage != nil {
				t.Fatal("error-only operation was not finalized once without invented usage")
			}
		})
	}
}

func TestSubscriptionWebSocketFailureTailRejectsConflictsAndSuccess(t *testing.T) {
	for _, kind := range []string{"conflicting_id", "response.completed", "response.output_item.added"} {
		t.Run(kind, func(t *testing.T) {
			fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(c *websocket.Conn, _ []byte, id string) {
				writeFailureTailEvent(c, "response.created", id, 0)
				writeFailureTailEvent(c, "error", "", 0)
				if kind == "conflicting_id" {
					writeFailureTailEvent(c, "response.failed", id+"_conflict", 99)
				} else {
					writeFailureTailEvent(c, kind, id, 99)
				}
			})
			c, keyID := dialFailureTail(t, fixture, true)
			defer c.CloseNow()
			writeE2EWebSocketText(t, c, failureTailCreate)
			readFailureTailEvent(t, c, "response.created")
			readFailureTailEvent(t, c, "error")
			assertFailureTailClose(t, c)
			history := waitForFailureTailHistory(t, fixture.store, keyID, 1)
			if history[0].Status != storage.RequestUpstreamErr || history[0].Usage != nil {
				t.Fatal("invalid footer contributed success or usage")
			}
		})
	}
}

func failureTailRoute(subscription bool) string {
	if subscription {
		return "subscription"
	}
	return "api_key"
}

func dialFailureTail(t *testing.T, fixture responsesProfileWebSocketFixture, subscription bool) (*websocket.Conn, string) {
	t.Helper()
	key, keyID := fixture.apiKey, fixture.apiKeyID
	if subscription {
		key, keyID = fixture.subscriptionKey, fixture.subscriptionKeyID
	}
	return dialResponsesProfileWebSocket(t, fixture.public, key, http.Header{"Originator": []string{"codex_exec"}}), keyID
}

func writeFailureTailEvent(c *websocket.Conn, kind, id string, tokens int) {
	event := map[string]any{"type": kind}
	if kind == "error" {
		event["error"] = map[string]any{"code": "server_error", "message": "private-synthetic"}
	} else {
		response := map[string]any{"id": id, "output": []any{}}
		if kind == "response.failed" {
			response["error"] = map[string]any{"code": "invalid_prompt", "message": "private-synthetic"}
		}
		if tokens != 0 {
			response["usage"] = map[string]any{"total_tokens": tokens}
		}
		event["response"] = response
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	_ = c.Write(ctx, websocket.MessageText, mustRequestJSONValue(event))
}

func readFailureTailEvent(t *testing.T, c *websocket.Conn, kind string) map[string]any {
	t.Helper()
	event := decodeRequestObject(t, []byte(readE2EWebSocketText(t, c)))
	if event["type"] != kind {
		t.Fatalf("event type = %v, want %s", event["type"], kind)
	}
	return event
}

func assertFailureTailClose(t *testing.T, c *websocket.Conn) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	_, _, err := c.Read(ctx)
	var closeError websocket.CloseError
	var failure protocolv1.FailureMetadata
	if !errors.As(err, &closeError) || int(closeError.Code) != protocolv1.FailureCloseCode ||
		json.Unmarshal([]byte(closeError.Reason), &failure) != nil ||
		failure.RetryAdvice != protocolv1.RetryNever || failure.DeliveryState != protocolv1.DeliveryDelivered {
		t.Fatal("failure tail lost delivered/never replay evidence")
	}
}

func waitForFailureTailHistory(t *testing.T, store *storage.Store, keyID string, count int) []storage.RequestRecord {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for {
		history, err := store.History(context.Background(), keyID, nil, 100)
		if err != nil {
			t.Fatal("history unavailable")
		}
		terminal := len(history) == count
		for _, record := range history {
			terminal = terminal && record.Status != storage.RequestInProgress
		}
		if terminal {
			return history
		}
		if time.Now().After(deadline) {
			t.Fatal("terminal history count did not converge")
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func assertFailureTailRecord(t *testing.T, record storage.RequestRecord, status string, tokens int64) {
	t.Helper()
	if record.Status != status || record.Usage == nil || record.Usage.TotalTokens != tokens || record.TTFBMilliseconds == nil {
		t.Fatal("terminal history lost its status, usage, or delivery observation")
	}
}

func assertFailureTailStats(t *testing.T, store *storage.Store, keyID string, count, failures, tokens int64) {
	t.Helper()
	stats, err := store.Stats(context.Background(), keyID, "", "")
	if err != nil || len(stats) != 1 || stats[0].RequestCount != count || stats[0].ErrorCount != failures ||
		stats[0].UsageObservationCount != count || stats[0].Usage == nil || stats[0].Usage.TotalTokens != tokens {
		t.Fatal("failed-footer daily accounting was lost or counted more than once")
	}
}
