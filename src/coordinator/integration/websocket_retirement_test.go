package integration

import (
	"context"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
	"mini-sub2api/src/coordinator/internal/storage"
)

func TestWebSocketFailureRetiresBeforeAnotherCreate(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, delay := range []time.Duration{0, 1500 * time.Millisecond} {
			name := failureTailRoute(subscription) + "/before_expiry"
			if delay != 0 {
				name = failureTailRoute(subscription) + "/after_expiry"
			}
			t.Run(name, func(t *testing.T) {
				var creates atomic.Int32
				var failedID string
				fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(c *websocket.Conn, _ []byte, id string) {
					if creates.Add(1) == 1 {
						failedID = id
						writeFailureTailEvent(c, "response.created", id, 0)
						writeFailureTailEvent(c, "error", "", 0)
						return
					}
					writeFailureTailEvent(c, "response.failed", failedID, 7)
					writeFailureTailEvent(c, "response.created", id, 0)
					writeFailureTailEvent(c, "response.completed", id, 11)
				})
				c, keyID := dialFailureTail(t, fixture, subscription)
				defer c.CloseNow()
				writeE2EWebSocketText(t, c, failureTailCreate)
				readFailureTailEvent(t, c, "response.created")
				readFailureTailEvent(t, c, "error")
				time.Sleep(delay)
				ctx, cancel := context.WithTimeout(context.Background(), time.Second)
				_ = c.Write(ctx, websocket.MessageText, []byte(failureTailCreate))
				cancel()
				assertFailureTailClose(t, c)
				history := waitForFailureTailHistory(t, fixture.store, keyID, 1)
				if history[0].Status != storage.RequestUpstreamErr || history[0].Usage != nil {
					t.Fatal("retired response changed status or acquired late usage")
				}
				if creates.Load() != 1 {
					t.Fatal("retired physical stream accepted another create")
				}
			})
		}
	}
}

func TestWebSocketRepeatedFailuresUseSeparateConnectionsWithoutReplay(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		t.Run(failureTailRoute(subscription), func(t *testing.T) {
			var creates atomic.Int32
			fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(c *websocket.Conn, _ []byte, id string) {
				creates.Add(1)
				writeFailureTailEvent(c, "response.created", id, 0)
				writeFailureTailEvent(c, "response.failed", id, 7)
			})
			var keyID string
			for round := 0; round < 3; round++ {
				c, key := dialFailureTail(t, fixture, subscription)
				keyID = key
				writeE2EWebSocketText(t, c, failureTailCreate)
				readFailureTailEvent(t, c, "response.created")
				readFailureTailEvent(t, c, "response.failed")
				assertFailureTailClose(t, c)
				_ = c.CloseNow()
			}
			if creates.Load() != 3 {
				t.Fatal("failed inference was replayed")
			}
			for _, record := range waitForFailureTailHistory(t, fixture.store, keyID, 3) {
				assertFailureTailRecord(t, record, storage.RequestUpstreamErr, 7)
			}
			captures := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 3)
			if captures[0].ProviderRequestID == captures[1].ProviderRequestID || captures[1].ProviderRequestID == captures[2].ProviderRequestID {
				t.Fatal("failure reused a physical stream")
			}
		})
	}
}
