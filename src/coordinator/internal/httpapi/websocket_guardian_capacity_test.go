package httpapi

import (
	"encoding/json"
	"net/http"
	"sync"
	"testing"
)

func TestWebSocketCapacityRejectionProvidesNativeRetryDelay(t *testing.T) {
	for _, guardian := range []bool{false, true} {
		store, _, key := setupHTTPTest(t)
		handler, public := startPublicWebSocketServer(t, store, &rejectingWebSocketCore{})
		limit := maxWebSocketsPerKey
		headers := make(http.Header)
		if guardian {
			limit--
			headers.Set("X-Openai-Subagent", "guardian")
		}
		for i := 0; i < limit; i++ {
			if !handler.websockets.acquireRole(key.ID, guardian) {
				t.Fatal("fixture capacity unavailable")
			}
		}
		connection, response, err := dialPublicWebSocket(t, public.URL, key.Secret, headers)
		if err == nil || connection != nil || response == nil || response.StatusCode != http.StatusTooManyRequests {
			t.Fatal("capacity rejection did not remain HTTP 429")
		}
		if response.Header.Get("Retry-After") != "1" {
			t.Fatal("capacity rejection lost native retry advice")
		}
		var body struct {
			Error struct {
				Code          string `json:"code"`
				RetryAdvice   string `json:"retryAdvice"`
				DeliveryState string `json:"deliveryState"`
			} `json:"error"`
		}
		if json.Unmarshal([]byte(responseBody(t, response)), &body) != nil || body.Error.Code != "websocket_connection_limit" || body.Error.RetryAdvice != "safe" || body.Error.DeliveryState != "not_delivered" {
			t.Fatal("capacity rejection category changed")
		}
		for i := 0; i < limit; i++ {
			handler.websockets.releaseRole(key.ID, guardian)
		}
		_, next, _ := dialPublicWebSocket(t, public.URL, key.Secret, headers)
		if next == nil || next.StatusCode != http.StatusUpgradeRequired {
			t.Fatal("released capacity did not allow the next upstream handshake")
		}
		_ = responseBody(t, next)
	}
}

func TestGuardianFailedUpgradeReleasesBothAdmissionCounters(t *testing.T) {
	store, _, key := setupHTTPTest(t)
	handler, public := startPublicWebSocketServer(t, store, &rejectingWebSocketCore{})
	for attempt := 0; attempt < 16; attempt++ {
		connection, response, err := dialPublicWebSocket(t, public.URL, key.Secret, http.Header{"X-Openai-Subagent": {"guardian"}})
		if err == nil || connection != nil || response == nil || response.StatusCode != http.StatusUpgradeRequired {
			t.Fatal("failed Guardian upgrade consumed admission capacity")
		}
		_ = responseBody(t, response)
	}
	handler.ShutdownWebSockets()
	handler.websockets.mu.Lock()
	defer handler.websockets.mu.Unlock()
	if len(handler.websockets.guardian) != 0 || len(handler.websockets.perKey) != 0 {
		t.Fatal("failed Guardian upgrade leaked an admission counter")
	}
}

func TestGuardianSocketReservationKeepsTotalLimitAndOtherKeysIndependent(t *testing.T) {
	manager := newWebSocketManager(maxWebSocketsPerKey)
	for i := 0; i < 7; i++ {
		if !manager.acquireRole("one", true) {
			t.Fatal("bounded Guardian capacity unavailable")
		}
	}
	if manager.acquireRole("one", true) {
		t.Fatal("Guardian consumed sampling reservation")
	}
	if !manager.acquire("one") {
		t.Fatal("sampling reservation unavailable")
	}
	if manager.acquire("one") || manager.acquireRole("one", true) {
		t.Fatal("total per-Key cap bypassed")
	}
	for i := 0; i < 8; i++ {
		if !manager.acquire("two") {
			t.Fatal("ordinary-only or other-Key capacity changed")
		}
	}
	manager.releaseRole("one", true)
	if !manager.acquireRole("one", true) {
		t.Fatal("closed or failed Guardian upgrade leaked capacity")
	}
	manager.release("one")
	if manager.acquireRole("one", true) {
		t.Fatal("released sampling slot became Guardian capacity")
	}
	for i := 0; i < 7; i++ {
		manager.releaseRole("one", true)
	}
	if len(manager.guardian) != 0 || manager.perKey["one"] != 0 {
		t.Fatal("released counters retained ownership")
	}
}

func TestGuardianReservationConcurrentAdmissionAndRelease(t *testing.T) {
	manager := newWebSocketManager(maxWebSocketsPerKey)
	var group sync.WaitGroup
	acquired := make(chan bool, 32)
	release := make(chan struct{})
	for i := 0; i < 32; i++ {
		group.Add(1)
		go func() {
			defer group.Done()
			ok := manager.acquireRole("shared", true)
			acquired <- ok
			if ok {
				<-release
				manager.releaseRole("shared", true)
			}
		}()
	}
	successes := 0
	for i := 0; i < 32; i++ {
		if <-acquired {
			successes++
		}
	}
	if successes != 7 {
		t.Fatalf("concurrent Guardian acquisitions=%d", successes)
	}
	if !manager.acquire("shared") {
		t.Fatal("concurrent prewarm consumed sampling slot")
	}
	close(release)
	group.Wait()
	manager.release("shared")
	if len(manager.perKey) != 0 || len(manager.guardian) != 0 {
		t.Fatal("concurrent release leaked capacity")
	}
	manager.shutdown()
	if manager.acquireRole("shared", true) || manager.acquire("shared") {
		t.Fatal("shutdown admitted a socket")
	}
}
