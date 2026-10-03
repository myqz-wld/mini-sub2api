package integration

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestSubscriptionWebSocketUpgradeMetadata(t *testing.T) {
	for _, flag := range []string{"absent", "", "false", "true"} {
		for _, modelPresent := range []bool{false, true} {
			t.Run(fmt.Sprintf("reasoning_%s/model=%t", flag, modelPresent), func(t *testing.T) {
				handshakes := make(chan http.Header, 4)
				fixture := newResponsesProfileWebSocketFixtureWithHandshake(t,
					func(connection *websocket.Conn, payload []byte, id string) {
						writeResponsesProfileEvents(connection, id, isSyntheticResponsesProfilePrewarm(payload))
					}, func(w http.ResponseWriter, r *http.Request, _ int) bool {
						handshakes <- r.Header.Clone()
						if flag != "absent" {
							w.Header().Set("X-Reasoning-Included", flag)
						}
						w.Header().Del("OpenAI-Model")
						if modelPresent {
							w.Header().Set("OpenAI-Model", "synthetic-actual-model")
						}
						return true
					})
				headers := codexScenarioHeaders("synthetic-metadata-session", "codex-loopback/0.159.2")
				connection, response := dialResponsesProfileWebSocketWithResponse(t, fixture.public, fixture.subscriptionKey, headers)
				defer connection.CloseNow()
				for _, name := range []string{"X-Reasoning-Included", "OpenAI-Model"} {
					if len(response.Header.Values(name)) != 0 {
						t.Fatalf("deferred upgrade advertised unavailable %s", name)
					}
				}
				assertNoUpgradeHandshake(t, handshakes)
				assertProfileStateFileCount(t, fixture.coreStateDir, 0)
				assertNoUpgradeFrames(t, fixture)
				first, second := responsesProfileWebSocketFrames(t, "gpt-5.5")
				for _, frame := range [][]byte{first, second} {
					value := decodeResponsesProfileWebSocketFrame(t, frame)
					value["client_metadata"].(map[string]any)["session_id"] = headers.Get("Session-Id")
					writeE2EWebSocketText(t, connection, string(mustRequestJSON(t, value)))
					events := readResponsesProfileTerminalEvents(t, connection)
					var firstEvent struct {
						Type    string            `json:"type"`
						Headers map[string]string `json:"headers"`
					}
					if json.Unmarshal([]byte(events[0]), &firstEvent) != nil || firstEvent.Type != "response.created" {
						t.Fatal("model projection displaced actual response output")
					}
					wantModel := ""
					if modelPresent {
						wantModel = "synthetic-actual-model"
					}
					if firstEvent.Headers["openai-model"] != wantModel {
						t.Fatal("actual handshake model presence/value changed")
					}
				}
				actual := <-handshakes
				if actual.Get("Session-Id") == "" || actual.Get("Session-Id") == headers.Get("Session-Id") ||
					actual.Get("Authorization") == "" || actual.Get("OpenAI-Beta") == "" {
					t.Fatal("actual handshake lost authenticated scoped first-frame admission")
				}
				assertNoUpgradeHandshake(t, handshakes)
				waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 2)
				assertNoUpgradeFrames(t, fixture)
			})
		}
	}
}

func TestSubscriptionWebSocketReplacementPreservesModelAcrossReasoningChanges(t *testing.T) {
	for _, initiallyPresent := range []bool{false, true} {
		t.Run(fmt.Sprintf("initially_present=%t", initiallyPresent), func(t *testing.T) {
			var handshakes atomic.Int32
			fixture := newResponsesProfileWebSocketFixtureWithHandshake(t,
				func(connection *websocket.Conn, payload []byte, id string) {
					if isSyntheticResponsesProfilePrewarm(payload) {
						connection.CloseNow()
						return
					}
					writeResponsesProfileEvents(connection, id, false)
				}, func(w http.ResponseWriter, _ *http.Request, number int) bool {
					handshakes.Add(1)
					present := initiallyPresent
					model := "synthetic-setup-model"
					if number == 2 {
						present = !present
						model = "synthetic-replacement-model"
					}
					if present {
						w.Header().Set("X-Reasoning-Included", "false")
					}
					w.Header().Set("OpenAI-Model", model)
					return true
				})
			connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, nil)
			defer connection.CloseNow()
			if handshakes.Load() != 0 {
				t.Fatal("upstream connected before first-frame admission")
			}
			writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
			events := readResponsesProfileTerminalEvents(t, connection)
			if !strings.Contains(events[0], `"openai-model":"synthetic-replacement-model"`) {
				t.Fatal("replacement lost its model or rejected changed reasoning presence")
			}
			frames := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 2)
			if !isSyntheticResponsesProfilePrewarm(frames[0].Frame) || isSyntheticResponsesProfilePrewarm(frames[1].Frame) {
				t.Fatal("hidden failure replayed public inference")
			}
			if handshakes.Load() != 2 {
				t.Fatal("hidden replacement created an extra upstream connection")
			}
			assertProfileWebSocketDiagnosticHistory(t, fixture.store, fixture.subscriptionKeyID,
				responsesProfileProviderRequestID(2), 1)
			assertNoUpgradeFrames(t, fixture)
		})
	}
}

func TestSubscriptionWebSocketHandshakeFailureAfterUpgrade(t *testing.T) {
	for _, test := range []struct {
		name        string
		status      int
		usageWindow bool
	}{
		{"upgrade_required", http.StatusUpgradeRequired, false},
		{"rate_limit", http.StatusTooManyRequests, false},
		{"usage_limit", http.StatusTooManyRequests, true},
		{"authentication", http.StatusUnauthorized, false},
		{"unavailable", http.StatusServiceUnavailable, false},
	} {
		t.Run(test.name, func(t *testing.T) {
			var handshakes atomic.Int32
			fixture := newResponsesProfileWebSocketFixtureWithHandshake(t, nil,
				func(w http.ResponseWriter, _ *http.Request, _ int) bool {
					handshakes.Add(1)
					w.Header().Set("Content-Type", "application/json")
					w.Header().Set("X-Retry-Metadata", "NO_MORE_RETRY")
					w.WriteHeader(test.status)
					if test.usageWindow {
						_, _ = io.WriteString(w, `{"error":{"type":"usage_limit_reached","limit_window_minutes":5,"message":"synthetic-private"}}`)
					} else {
						_, _ = io.WriteString(w, `{"error":{"message":"synthetic-private"}}`)
					}
					return false
				})
			connection, response := dialResponsesProfileWebSocketWithResponse(t, fixture.public, fixture.subscriptionKey,
				http.Header{"Originator": {"codex_exec"}})
			defer connection.CloseNow()
			if response.StatusCode != http.StatusSwitchingProtocols || handshakes.Load() != 0 {
				t.Fatal("upstream failure affected the deferred public upgrade")
			}
			assertProfileStateFileCount(t, fixture.coreStateDir, 0)
			writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
			if test.usageWindow {
				event := readE2EWebSocketText(t, connection)
				var value struct {
					Type   string `json:"type"`
					Status int    `json:"status"`
					Error  struct {
						Type               string            `json:"type"`
						LimitWindowMinutes int               `json:"limit_window_minutes"`
						Headers            map[string]string `json:"headers"`
					} `json:"error"`
				}
				if json.Unmarshal([]byte(event), &value) != nil || value.Type != "error" ||
					value.Status != 429 || value.Error.Type != "usage_limit_reached" ||
					value.Error.LimitWindowMinutes != 5 || value.Error.Headers["x-retry-metadata"] != "NO_MORE_RETRY" ||
					strings.Contains(event, "synthetic-private") {
					t.Fatal("deferred rejection lost typed retry metadata or exposed provider text")
				}
			}
			wantDelivery := protocolv1.DeliveryNotDelivered
			if test.usageWindow {
				// The public session has observed a terminal native error event.
				wantDelivery = protocolv1.DeliveryDelivered
			}
			assertUpgradeFailureClose(t, connection, wantDelivery)
			attempts := handshakes.Load()
			if attempts < 1 || (test.status != http.StatusUnauthorized && attempts != 1) || attempts > 3 {
				t.Fatal("deferred handshake rejection retried outside OAuth recovery")
			}
			assertNoUpgradeFrames(t, fixture)
		})
	}
}

func TestSubscriptionWebSocketIdleCloseDoesNotConnectUpstream(t *testing.T) {
	handshakes := make(chan http.Header, 4)
	fixture := newResponsesProfileWebSocketFixtureWithHandshake(t, nil,
		func(_ http.ResponseWriter, r *http.Request, _ int) bool {
			handshakes <- r.Header.Clone()
			return false
		})
	connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, nil)
	defer connection.CloseNow()
	select {
	case <-handshakes:
		t.Fatal("idle downstream socket connected to upstream")
	case <-time.After(50 * time.Millisecond):
	}
	if err := connection.Close(websocket.StatusNormalClosure, ""); err != nil {
		t.Fatal("idle downstream socket did not close cleanly")
	}
	assertNoUpgradeHandshake(t, handshakes)
	assertProfileStateFileCount(t, fixture.coreStateDir, 0)
	assertNoUpgradeFrames(t, fixture)
}

func TestSubscriptionWebSocketCancelsDeferredHandshake(t *testing.T) {
	release := make(chan struct{})
	defer close(release)
	seen := make(chan struct{}, 1)
	cancelled := make(chan struct{}, 1)
	fixture := newResponsesProfileWebSocketFixtureWithHandshake(t, nil,
		func(_ http.ResponseWriter, r *http.Request, _ int) bool {
			seen <- struct{}{}
			select {
			case <-r.Context().Done():
				cancelled <- struct{}{}
			case <-release:
			}
			return false
		})
	connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey,
		http.Header{"Originator": {"codex_exec"}})
	defer connection.CloseNow()
	select {
	case <-seen:
		t.Fatal("stalled upstream handshake started before first create")
	default:
	}
	writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
	select {
	case <-seen:
	case <-time.After(3 * time.Second):
		t.Fatal("first create did not start the deferred handshake")
	}
	connection.CloseNow()
	select {
	case <-cancelled:
	case <-time.After(3 * time.Second):
		t.Fatal("downstream cancellation left the upstream handshake running")
	}
	assertNoUpgradeFrames(t, fixture)
}

func assertNoUpgradeHandshake(t *testing.T, handshakes <-chan http.Header) {
	t.Helper()
	select {
	case <-handshakes:
		t.Fatal("unexpected upstream WebSocket handshake")
	default:
	}
}

func assertNoUpgradeFrames(t *testing.T, fixture responsesProfileWebSocketFixture) {
	t.Helper()
	select {
	case <-fixture.captures:
		t.Fatal("unexpected inference frame crossed the upgrade boundary")
	default:
	}
}

func TestSubscriptionWebSocketModelNoticeWaitsForActualError(t *testing.T) {
	release := make(chan struct{})
	released := false
	defer func() {
		if !released {
			close(release)
		}
	}()
	fixture := newResponsesProfileWebSocketFixtureWithResponder(t,
		func(connection *websocket.Conn, _ []byte, _ string) {
			<-release
			_ = connection.Write(context.Background(), websocket.MessageText,
				[]byte(`{"type":"error","status":429,"error":{"code":"flex_unavailable"}}`))
		})
	headers := codexScenarioHeaders("synthetic-error-session", "codex-loopback/0.159.2")
	connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, headers)
	defer connection.CloseNow()
	writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
	waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	read := make(chan []byte, 1)
	go func() { _, event, _ := connection.Read(ctx); read <- event }()
	select {
	case <-read:
		t.Fatal("model notice appeared before actual upstream output")
	case <-time.After(50 * time.Millisecond):
	}
	close(release)
	released = true
	var notice map[string]any
	if json.Unmarshal(<-read, &notice) != nil || notice["type"] != "response.metadata" ||
		notice["headers"].(map[string]any)["openai-model"] != "gpt-loopback-ws" {
		t.Fatal("wrapped error lost the actual handshake model")
	}
	if event := readE2EWebSocketText(t, connection); !strings.Contains(event, `"flex_unavailable"`) {
		t.Fatal("model notice displaced the actual protocol failure")
	}
}

func assertUpgradeFailureClose(t *testing.T, connection *websocket.Conn, wantDelivery protocolv1.DeliveryState) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	_, _, err := connection.Read(ctx)
	var closed websocket.CloseError
	if !errors.As(err, &closed) || closed.Code != websocket.StatusCode(protocolv1.FailureCloseCode) {
		t.Fatal("deferred handshake failure did not produce a structured failure close")
	}
	var failure protocolv1.FailureMetadata
	if json.Unmarshal([]byte(closed.Reason), &failure) != nil || !failure.Valid() || failure.DeliveryState != wantDelivery {
		t.Fatal("deferred handshake failure changed delivery accounting")
	}
}
