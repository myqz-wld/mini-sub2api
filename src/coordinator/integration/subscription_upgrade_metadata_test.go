package integration

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestSubscriptionWebSocketUpgradeMetadata(t *testing.T) {
	for _, flag := range []string{"absent", "", "false", "true"} {
		t.Run("reasoning_"+flag, func(t *testing.T) {
			handshakes := make(chan http.Header, 4)
			fixture := newResponsesProfileWebSocketFixtureWithHandshake(t,
				func(connection *websocket.Conn, payload []byte, id string) {
					writeResponsesProfileEvents(connection, id, isSyntheticResponsesProfilePrewarm(payload))
				}, func(w http.ResponseWriter, r *http.Request, number int) bool {
					handshakes <- r.Header.Clone()
					if flag != "absent" {
						w.Header().Set("X-Reasoning-Included", flag)
					}
					model := "synthetic-actual-model"
					if number == 1 {
						model = "synthetic-probe-model"
					}
					w.Header().Set("OpenAI-Model", model)
					return true
				})
			headers := codexScenarioHeaders("synthetic-metadata-session", "codex-loopback/0.159.2")
			connection, response := dialResponsesProfileWebSocketWithResponse(t, fixture.public, fixture.subscriptionKey, headers)
			defer connection.CloseNow()
			values := response.Header.Values("X-Reasoning-Included")
			if (len(values) > 0) != (flag != "absent") || (len(values) > 0 && values[0] != flag) {
				t.Fatalf("reasoning presence/value changed: %q", values)
			}
			if response.Header.Get("OpenAI-Model") != "" {
				t.Fatal("probe model was exposed before actual route admission")
			}
			probe := <-handshakes
			for _, name := range []string{"session_id", "conversation_id", "Session-Id", "Thread-Id", "X-Codex-Turn-Metadata", "X-Codex-Guardian", "X-OpenAI-Subagent", "X-Client-Request-Id"} {
				if probe.Get(name) != "" {
					t.Fatalf("caller identity/role %s entered auth-only probe", name)
				}
			}
			if probe.Get("Authorization") == "" || probe.Get("OpenAI-Beta") == "" {
				t.Fatal("probe did not use authenticated native WebSocket setup")
			}
			assertProfileStateFileCount(t, fixture.coreStateDir, 0)
			assertNoUpgradeFrames(t, fixture)
			first, second := responsesProfileWebSocketFrames(t, "gpt-5.5")
			for _, frame := range [][]byte{first, second} {
				value := decodeResponsesProfileWebSocketFrame(t, frame)
				value["client_metadata"].(map[string]any)["session_id"] = headers.Get("Session-Id")
				writeE2EWebSocketText(t, connection, string(mustRequestJSON(t, value)))
				events := readResponsesProfileTerminalEvents(t, connection)
				var firstEvent map[string]any
				if json.Unmarshal([]byte(events[0]), &firstEvent) != nil ||
					firstEvent["headers"].(map[string]any)["openai-model"] != "synthetic-actual-model" {
					t.Fatal("actual handshake model missing from sampling operation")
				}
			}
			canonical := <-handshakes
			if canonical.Get("Session-Id") == "" || canonical.Get("Session-Id") == headers.Get("Session-Id") {
				t.Fatal("canonical handshake lost scoped first-frame admission")
			}
			waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 2)
			assertNoUpgradeFrames(t, fixture)
		})
	}
}

func TestSubscriptionWebSocketRejectsReasoningDriftBeforeInference(t *testing.T) {
	for _, advertised := range []bool{false, true} {
		for _, reconnect := range []bool{false, true} {
			name := "absent"
			if advertised {
				name = "present"
			}
			if reconnect {
				name += "_replacement"
			}
			t.Run(name, func(t *testing.T) {
				fixture := newResponsesProfileWebSocketFixtureWithHandshake(t,
					func(connection *websocket.Conn, payload []byte, id string) {
						// A failed hidden prewarm requires a new actual inference connection.
						connection.CloseNow()
					}, func(w http.ResponseWriter, _ *http.Request, number int) bool {
						present := advertised
						if (!reconnect && number == 2) || (reconnect && number == 3) {
							present = !present
						}
						if present {
							w.Header().Set("X-Reasoning-Included", "false")
						}
						return true
					})
				connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, nil)
				defer connection.CloseNow()
				writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
				assertUpgradeNotDelivered(t, connection)
				if reconnect {
					frames := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)
					if !isSyntheticResponsesProfilePrewarm(frames[0].Frame) {
						t.Fatal("drift allowed a public inference frame")
					}
				}
				actualConnection := 2
				if reconnect {
					actualConnection = 3
				}
				assertProfileWebSocketDiagnosticHistory(t, fixture.store, fixture.subscriptionKeyID,
					responsesProfileProviderRequestID(actualConnection), 1)
				assertNoUpgradeFrames(t, fixture)
			})
		}
	}
}

func TestSubscriptionWebSocketReplacementUsesActualModel(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixtureWithHandshake(t,
		func(connection *websocket.Conn, payload []byte, id string) {
			if isSyntheticResponsesProfilePrewarm(payload) {
				connection.CloseNow()
				return
			}
			writeResponsesProfileEvents(connection, id, false)
		}, func(w http.ResponseWriter, _ *http.Request, number int) bool {
			// Value may differ: native treats both values as presence, not booleans.
			w.Header().Set("X-Reasoning-Included", "false")
			w.Header().Set("OpenAI-Model", "synthetic-setup-model")
			if number == 3 {
				w.Header().Set("X-Reasoning-Included", "true")
				w.Header().Set("OpenAI-Model", "synthetic-replacement-model")
			}
			return true
		})
	connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, nil)
	defer connection.CloseNow()
	writeE2EWebSocketText(t, connection, `{"type":"response.create","model":"gpt-5.5","input":[]}`)
	events := readResponsesProfileTerminalEvents(t, connection)
	if !strings.Contains(events[0], `"openai-model":"synthetic-replacement-model"`) {
		t.Fatal("public response did not carry replacement connection model")
	}
	frames := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 2)
	if !isSyntheticResponsesProfilePrewarm(frames[0].Frame) || isSyntheticResponsesProfilePrewarm(frames[1].Frame) {
		t.Fatal("hidden failure replayed public inference")
	}
}

func TestSubscriptionWebSocketProbeRejectsBeforeUpgrade(t *testing.T) {
	for _, test := range []struct {
		name        string
		status      int
		usageWindow bool
	}{
		{"fallback", http.StatusUpgradeRequired, false},
		{"rate_limit", http.StatusTooManyRequests, false},
		{"usage_limit", http.StatusTooManyRequests, true},
		{"authentication", http.StatusUnauthorized, false},
	} {
		t.Run(test.name, func(t *testing.T) {
			fixture := newResponsesProfileWebSocketFixtureWithHandshake(t, nil,
				func(w http.ResponseWriter, _ *http.Request, _ int) bool {
					w.Header().Set("Content-Type", "application/json")
					w.Header().Set("X-Retry-Metadata", "NO_MORE_RETRY")
					w.Header().Set("Retry-After", "5")
					w.WriteHeader(test.status)
					if test.usageWindow {
						_, _ = io.WriteString(w, `{"error":{"type":"usage_limit_reached","limit_window_minutes":5,"message":"synthetic-private"}}`)
					} else {
						_, _ = io.WriteString(w, `{"error":{"code":"rate_limit_exceeded","message":"synthetic-private"}}`)
					}
					return false
				})
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			connection, response, err := websocket.Dial(ctx, fixture.public.URL+"/v1/responses", &websocket.DialOptions{
				HTTPHeader: http.Header{"Authorization": {"Bearer " + fixture.subscriptionKey}},
			})
			if connection != nil || err == nil || response == nil || response.StatusCode != test.status {
				t.Fatalf("probe did not reject with expected HTTP status %d", test.status)
			}
			defer response.Body.Close()
			var envelope struct {
				Error struct {
					protocolv1.CoreError
					UsageWindow *uint16 `json:"limit_window_minutes"`
				}
			}
			if err := json.NewDecoder(response.Body).Decode(&envelope); err != nil {
				t.Fatal(err)
			}
			if envelope.Error.DeliveryState != protocolv1.DeliveryNotDelivered ||
				!envelope.Error.FailureMetadata.Valid() || strings.Contains(envelope.Error.Message, "synthetic-private") {
				t.Fatal("probe rejection lost delivery truth or exposed provider message")
			}
			if response.Header.Get("X-Codex-Installation-Id") != "" {
				t.Fatal("private upgrade header crossed rejection")
			}
			if test.status != http.StatusUnauthorized && (response.Header.Get("Retry-After") != "5" || response.Header.Get("X-Retry-Metadata") != "NO_MORE_RETRY") {
				t.Fatal("probe rejection dropped retry headers")
			}
			if test.usageWindow && (envelope.Error.UsageWindow == nil || *envelope.Error.UsageWindow != 5) {
				t.Fatal("probe rejection dropped native usage window")
			}
			assertProfileStateFileCount(t, fixture.coreStateDir, 0)
			assertNoUpgradeFrames(t, fixture)
		})
	}
}

func TestSubscriptionWebSocketProbeDeadlineAndCancellation(t *testing.T) {
	for _, cancelEarly := range []bool{false, true} {
		name := "deadline"
		if cancelEarly {
			name = "cancel"
		}
		t.Run(name, func(t *testing.T) {
			release := make(chan struct{})
			defer close(release)
			seen := make(chan struct{}, 1)
			fixture := newResponsesProfileWebSocketFixtureWithHandshake(t, nil,
				func(_ http.ResponseWriter, r *http.Request, _ int) bool {
					seen <- struct{}{}
					select {
					case <-r.Context().Done():
					case <-release:
					}
					return false
				})
			ctx, cancel := context.WithTimeout(context.Background(), 13*time.Second)
			defer cancel()
			if cancelEarly {
				go func() { <-seen; cancel() }()
			}
			started := time.Now()
			connection, response, err := websocket.Dial(ctx, fixture.public.URL+"/v1/responses", &websocket.DialOptions{
				HTTPHeader: http.Header{"Authorization": {"Bearer " + fixture.subscriptionKey}},
			})
			if err == nil || connection != nil {
				t.Fatal("stalled probe upgraded downstream")
			}
			if !cancelEarly {
				if response == nil || response.StatusCode != http.StatusBadGateway || time.Since(started) > 12*time.Second {
					t.Fatal("probe exceeded bounded connect deadline")
				}
				defer response.Body.Close()
				var envelope protocolv1.ErrorEnvelope
				if json.NewDecoder(response.Body).Decode(&envelope) != nil || envelope.Error.DeliveryState != protocolv1.DeliveryNotDelivered {
					t.Fatal("probe timeout lost not-delivered proof")
				}
			}
			assertProfileStateFileCount(t, fixture.coreStateDir, 0)
			assertNoUpgradeFrames(t, fixture)
		})
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

func assertUpgradeNotDelivered(t *testing.T, connection *websocket.Conn) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	_, _, err := connection.Read(ctx)
	var closed websocket.CloseError
	if !errors.As(err, &closed) || closed.Code != websocket.StatusCode(protocolv1.FailureCloseCode) {
		t.Fatal("metadata drift did not produce a structured failure close")
	}
	var failure protocolv1.FailureMetadata
	if json.Unmarshal([]byte(closed.Reason), &failure) != nil || !failure.Valid() || failure.DeliveryState != protocolv1.DeliveryNotDelivered {
		t.Fatal("metadata drift reported inference delivery")
	}
}
