package storage

import (
	"context"
	"testing"
	"time"
)

func TestUsageIsAttributedToEachDownstreamKey(t *testing.T) {
	clock := &testClock{now: time.Date(2026, 8, 2, 10, 0, 0, 0, time.UTC)}
	store, _ := openTestStore(t, clock)
	credential := createTestCredential(t, store, "shared")
	first, err := store.CreateAPIKey(context.Background(), credential.ID, "First")
	if err != nil {
		t.Fatal(err)
	}
	second, err := store.CreateAPIKey(context.Background(), credential.ID, "Second")
	if err != nil {
		t.Fatal(err)
	}

	finishRequest(t, store, first, "req_first", TokenUsage{InputTokens: 10, OutputTokens: 4, TotalTokens: 14})
	finishRequest(t, store, second, "req_second", TokenUsage{InputTokens: 20, OutputTokens: 8, TotalTokens: 28})
	firstStats, err := store.Stats(context.Background(), first.ID, "", "")
	if err != nil {
		t.Fatal(err)
	}
	secondStats, err := store.Stats(context.Background(), second.ID, "", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(firstStats) != 1 || firstStats[0].Usage.TotalTokens != 14 {
		t.Fatalf("first key stats = %#v", firstStats)
	}
	if len(secondStats) != 1 || secondStats[0].Usage.TotalTokens != 28 {
		t.Fatalf("second key stats = %#v", secondStats)
	}
}

func TestDetailPruneRetainsAggregatesUntilExplicitDeletion(t *testing.T) {
	clock := &testClock{now: time.Date(2026, 8, 2, 10, 0, 0, 0, time.UTC)}
	store, _ := openTestStore(t, clock)
	credential := createTestCredential(t, store, "retention")
	key, err := store.CreateAPIKey(context.Background(), credential.ID, "Retention")
	if err != nil {
		t.Fatal(err)
	}
	finishRequest(t, store, key, "req_old", TokenUsage{TotalTokens: 9})

	clock.Set(time.Date(2026, 8, 12, 10, 0, 0, 0, time.UTC))
	deleted, err := store.PruneDetailsBefore(
		context.Background(), time.Date(2026, 8, 5, 0, 0, 0, 0, time.UTC),
	)
	if err != nil {
		t.Fatal(err)
	}
	if deleted != 1 {
		t.Fatalf("deleted details = %d, want 1", deleted)
	}
	history, err := store.History(context.Background(), key.ID, nil, 100)
	if err != nil {
		t.Fatal(err)
	}
	if len(history) != 0 {
		t.Fatalf("history retained after prune: %#v", history)
	}
	stats, err := store.Stats(context.Background(), key.ID, "", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(stats) != 1 || stats[0].RequestCount != 1 || stats[0].Usage.TotalTokens != 9 {
		t.Fatalf("aggregate after detail prune = %#v", stats)
	}
	aggregates, err := store.DeleteAggregatesBefore(context.Background(), "2026-08-03")
	if err != nil {
		t.Fatal(err)
	}
	if aggregates != 1 {
		t.Fatalf("deleted aggregates = %d, want 1", aggregates)
	}
}

func TestRecoverInFlightCreatesErrorAggregate(t *testing.T) {
	clock := &testClock{now: time.Date(2026, 8, 3, 9, 0, 0, 0, time.UTC)}
	store, _ := openTestStore(t, clock)
	credential := createTestCredential(t, store, "recover")
	key, err := store.CreateAPIKey(context.Background(), credential.ID, "Recover")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.AuthenticateAndStart(context.Background(), key.Secret, "req_interrupted"); err != nil {
		t.Fatal(err)
	}
	count, err := store.RecoverInFlight(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if count != 1 {
		t.Fatalf("recovered = %d, want 1", count)
	}
	stats, err := store.Stats(context.Background(), key.ID, "", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(stats) != 1 || stats[0].ErrorCount != 1 || stats[0].Usage != nil {
		t.Fatalf("recovered aggregate = %#v", stats)
	}
	history, err := store.History(context.Background(), key.ID, nil, 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(history) != 1 || history[0].ProviderRequestID != nil {
		t.Fatalf("recovered request detail = %#v", history)
	}
}

func TestProviderRequestIDIsValidatedStoredAndPrunedWithRequestDetail(t *testing.T) {
	clock := &testClock{now: time.Date(2026, 8, 6, 10, 0, 0, 0, time.UTC)}
	store, _ := openTestStore(t, clock)
	credential := createTestCredential(t, store, "provider-request-id")
	key, err := store.CreateAPIKey(context.Background(), credential.ID, "Provider request ID")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.AuthenticateAndStart(context.Background(), key.Secret, "req_provider_id"); err != nil {
		t.Fatal(err)
	}
	invalid := "contains space"
	if err := store.FinalizeRequest(context.Background(), "req_provider_id", RequestResult{
		CompletedAt: clock.Time(), Status: RequestCompleted, ProviderRequestID: &invalid,
	}); err == nil {
		t.Fatal("invalid provider request ID was accepted")
	}
	if _, err := store.db.Exec(`UPDATE requests SET provider_request_id = 'contains space'
		WHERE request_id = 'req_provider_id'`); err == nil {
		t.Fatal("SQLite provider request ID constraint was bypassed")
	}
	providerRequestID := "provider-visible-ascii"
	if err := store.FinalizeRequest(context.Background(), "req_provider_id", RequestResult{
		CompletedAt: clock.Time(), Status: RequestCompleted, ProviderRequestID: &providerRequestID,
	}); err != nil {
		t.Fatal(err)
	}
	history, err := store.History(context.Background(), key.ID, nil, 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(history) != 1 || history[0].ProviderRequestID == nil ||
		*history[0].ProviderRequestID != providerRequestID {
		t.Fatalf("provider request ID history = %#v", history)
	}
	clock.Set(time.Date(2026, 8, 14, 10, 0, 0, 0, time.UTC))
	deleted, err := store.PruneDetailsBefore(
		context.Background(), time.Date(2026, 8, 13, 0, 0, 0, 0, time.UTC),
	)
	if err != nil || deleted != 1 {
		t.Fatalf("pruned request details = %d, %v", deleted, err)
	}
	history, err = store.History(context.Background(), key.ID, nil, 10)
	if err != nil || len(history) != 0 {
		t.Fatalf("history after provider request ID prune = %#v, %v", history, err)
	}
}

func TestPruneBoundaryUsesChronologicalFixedWidthTimestamps(t *testing.T) {
	cutoff := time.Date(2026, 8, 5, 0, 0, 0, 0, time.UTC)
	clock := &testClock{now: cutoff.Add(500 * time.Millisecond)}
	store, _ := openTestStore(t, clock)
	credential := createTestCredential(t, store, "boundary")
	key, err := store.CreateAPIKey(context.Background(), credential.ID, "Boundary")
	if err != nil {
		t.Fatal(err)
	}
	finishRequest(t, store, key, "req_after_cutoff", TokenUsage{TotalTokens: 1})
	deleted, err := store.PruneDetailsBefore(context.Background(), cutoff)
	if err != nil {
		t.Fatal(err)
	}
	if deleted != 0 {
		t.Fatalf("deleted %d request(s) at or after cutoff", deleted)
	}
	history, err := store.History(context.Background(), key.ID, &cutoff, 10)
	if err != nil || len(history) != 1 {
		t.Fatalf("history at cutoff = %#v, %v", history, err)
	}
}

func finishRequest(t *testing.T, store *Store, key CreatedAPIKey, requestID string, usage TokenUsage) {
	t.Helper()
	if _, err := store.AuthenticateAndStart(context.Background(), key.Secret, requestID); err != nil {
		t.Fatal(err)
	}
	httpStatus := 200
	ttfb := 25 * time.Millisecond
	if err := store.FinalizeRequest(context.Background(), requestID, RequestResult{
		CompletedAt: store.clock(), Status: RequestCompleted, HTTPStatus: &httpStatus,
		TTFB: &ttfb, Duration: 80 * time.Millisecond, Usage: &usage,
	}); err != nil {
		t.Fatal(err)
	}
}
