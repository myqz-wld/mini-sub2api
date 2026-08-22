package storage

import (
	"context"
	"errors"
	"testing"
	"time"
)

func TestWebSocketRouteIsRevalidatedForEveryOperation(t *testing.T) {
	clock := &testClock{now: time.Date(2026, 8, 4, 10, 0, 0, 0, time.UTC)}
	store, _ := openTestStore(t, clock)
	credential := createTestCredential(t, store, "websocket-route")
	key, err := store.CreateAPIKey(context.Background(), credential.ID, "WebSocket route")
	if err != nil {
		t.Fatal(err)
	}
	route, err := store.AuthenticateConnection(context.Background(), key.Secret)
	if err != nil {
		t.Fatal(err)
	}
	var requestCount int
	if err := store.db.QueryRow(`SELECT count(*) FROM requests`).Scan(&requestCount); err != nil {
		t.Fatal(err)
	}
	if requestCount != 0 {
		t.Fatalf("handshake created %d request rows", requestCount)
	}
	if err := store.StartWebSocketOperation(
		context.Background(), route, "req_ws_first", OperationInference,
	); err != nil {
		t.Fatal(err)
	}
	if err := store.RevokeAPIKey(context.Background(), key.ID); err != nil {
		t.Fatal(err)
	}
	if err := store.StartWebSocketOperation(
		context.Background(), route, "req_ws_revoked", OperationInference,
	); !errors.Is(err, ErrUnauthorized) {
		t.Fatalf("revoked route error = %v", err)
	}
	if err := store.FinalizeRequest(context.Background(), "req_ws_first", RequestResult{
		CompletedAt: clock.Time(), Status: RequestCompleted, Duration: time.Second,
	}); err != nil {
		t.Fatal(err)
	}
	history, err := store.History(context.Background(), key.ID, nil, 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(history) != 1 || history[0].Transport != TransportWebSocket ||
		history[0].OperationKind != OperationInference {
		t.Fatalf("history = %#v", history)
	}
}

func TestWebSocketPrewarmParticipatesInFlightButNotDailyInference(t *testing.T) {
	clock := &testClock{now: time.Date(2026, 8, 4, 11, 0, 0, 0, time.UTC)}
	store, _ := openTestStore(t, clock)
	credential := createTestCredential(t, store, "websocket-prewarm")
	key, err := store.CreateAPIKey(context.Background(), credential.ID, "WebSocket prewarm")
	if err != nil {
		t.Fatal(err)
	}
	route, err := store.AuthenticateConnection(context.Background(), key.Secret)
	if err != nil {
		t.Fatal(err)
	}
	if err := store.StartWebSocketOperation(
		context.Background(), route, "req_ws_prewarm", OperationWebSocketPrewarm,
	); err != nil {
		t.Fatal(err)
	}
	if count, err := store.InFlightCount(context.Background(), credential.ID); err != nil || count != 1 {
		t.Fatalf("in-flight = %d, %v", count, err)
	}
	usage := &TokenUsage{InputTokens: 1, OutputTokens: 2, TotalTokens: 3}
	if err := store.FinalizeRequest(context.Background(), "req_ws_prewarm", RequestResult{
		CompletedAt: clock.Time(), Status: RequestCompleted, Duration: time.Second, Usage: usage,
	}); err != nil {
		t.Fatal(err)
	}
	stats, err := store.Stats(context.Background(), key.ID, "", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(stats) != 0 {
		t.Fatalf("prewarm entered inference aggregates: %#v", stats)
	}
	history, err := store.History(context.Background(), key.ID, nil, 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(history) != 1 || history[0].OperationKind != OperationWebSocketPrewarm ||
		history[0].Usage == nil || history[0].Usage.TotalTokens != 3 {
		t.Fatalf("prewarm history = %#v", history)
	}
}

func TestDisabledCredentialRejectsNextWebSocketOperation(t *testing.T) {
	clock := &testClock{now: time.Date(2026, 8, 4, 12, 0, 0, 0, time.UTC)}
	store, _ := openTestStore(t, clock)
	credential := createTestCredential(t, store, "websocket-disabled")
	key, err := store.CreateAPIKey(context.Background(), credential.ID, "WebSocket disabled")
	if err != nil {
		t.Fatal(err)
	}
	route, err := store.AuthenticateConnection(context.Background(), key.Secret)
	if err != nil {
		t.Fatal(err)
	}
	if err := store.SetCredentialEnabled(context.Background(), credential.ID, false); err != nil {
		t.Fatal(err)
	}
	if err := store.StartWebSocketOperation(
		context.Background(), route, "req_ws_disabled", OperationInference,
	); !errors.Is(err, ErrUnauthorized) {
		t.Fatalf("disabled credential error = %v", err)
	}
}
