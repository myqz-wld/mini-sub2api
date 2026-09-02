package storage

import (
	"context"
	"errors"
	"testing"
	"time"
)

func TestCredentialMutationFenceRequiresDisabledAndDrained(t *testing.T) {
	store := openCredentialFenceTestStore(t)
	credential := createTestCredential(t, store, "mutation-gate")
	called := false
	mutate := func(string) error {
		called = true
		return nil
	}

	err := store.WithCredentialMutationFence(context.Background(), credential.ID, mutate)
	if !errors.Is(err, ErrConflict) || called {
		t.Fatalf("enabled fence error = %v, called = %v", err, called)
	}

	key, err := store.CreateAPIKey(context.Background(), credential.ID, "Mutation client")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.AuthenticateAndStart(
		context.Background(), key.Secret, "req_mutation_in_flight",
	); err != nil {
		t.Fatal(err)
	}
	if err := store.SetCredentialEnabled(context.Background(), credential.ID, false); err != nil {
		t.Fatal(err)
	}
	err = store.WithCredentialMutationFence(context.Background(), credential.ID, mutate)
	if !errors.Is(err, ErrConflict) || called {
		t.Fatalf("in-flight fence error = %v, called = %v", err, called)
	}
}

func TestCredentialMutationFenceRollsBackCallbackFailure(t *testing.T) {
	store := openCredentialFenceTestStore(t)
	credential := createTestCredential(t, store, "mutation-failure")
	if err := store.SetCredentialEnabled(context.Background(), credential.ID, false); err != nil {
		t.Fatal(err)
	}
	want := errors.New("core mutation failed")
	err := store.WithCredentialMutationFence(
		context.Background(), credential.ID,
		func(accountRef string) error {
			if accountRef != credential.AccountRef {
				t.Fatalf("account ref = %q", accountRef)
			}
			return want
		},
	)
	if !errors.Is(err, want) {
		t.Fatalf("mutation error = %v", err)
	}
	stored, err := store.Credential(context.Background(), credential.ID)
	if err != nil {
		t.Fatal(err)
	}
	if stored.Status != CredentialDisabled {
		t.Fatalf("credential status = %q", stored.Status)
	}
}

func TestCredentialMutationFenceSerializesConcurrentEnable(t *testing.T) {
	store := openCredentialFenceTestStore(t)
	credential := createTestCredential(t, store, "mutation-race")
	if err := store.SetCredentialEnabled(context.Background(), credential.ID, false); err != nil {
		t.Fatal(err)
	}
	entered := make(chan struct{})
	release := make(chan struct{})
	fenceDone := make(chan error, 1)
	go func() {
		fenceDone <- store.WithCredentialMutationFence(
			context.Background(), credential.ID,
			func(string) error {
				close(entered)
				<-release
				return nil
			},
		)
	}()
	select {
	case <-entered:
	case <-time.After(2 * time.Second):
		t.Fatal("mutation did not enter fenced callback")
	}
	enableDone := make(chan error, 1)
	go func() {
		enableDone <- store.SetCredentialEnabled(context.Background(), credential.ID, true)
	}()
	select {
	case err := <-enableDone:
		t.Fatalf("enable escaped mutation fence: %v", err)
	case <-time.After(100 * time.Millisecond):
	}
	close(release)
	if err := <-fenceDone; err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-enableDone:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("enable did not resume after mutation fence")
	}
}

func TestCredentialRemovalFenceRequiresNoActiveKeysAndRollsBackFailure(t *testing.T) {
	store := openCredentialFenceTestStore(t)
	credential := createTestCredential(t, store, "removal-gate")
	key, err := store.CreateAPIKey(context.Background(), credential.ID, "Removal client")
	if err != nil {
		t.Fatal(err)
	}
	if err := store.SetCredentialEnabled(context.Background(), credential.ID, false); err != nil {
		t.Fatal(err)
	}
	called := false
	err = store.RemoveCredentialWithMutationFence(
		context.Background(), credential.ID,
		func(string) error {
			called = true
			return nil
		},
	)
	if !errors.Is(err, ErrConflict) || called {
		t.Fatalf("active-key removal error = %v, called = %v", err, called)
	}
	if err := store.RevokeAPIKey(context.Background(), key.ID); err != nil {
		t.Fatal(err)
	}
	want := errors.New("core removal failed")
	err = store.RemoveCredentialWithMutationFence(
		context.Background(), credential.ID,
		func(accountRef string) error {
			if accountRef != credential.AccountRef {
				t.Fatalf("account ref = %q", accountRef)
			}
			return want
		},
	)
	if !errors.Is(err, want) {
		t.Fatalf("callback error = %v", err)
	}
	stored, err := store.Credential(context.Background(), credential.ID)
	if err != nil {
		t.Fatal(err)
	}
	if stored.Status != CredentialDisabled || stored.DeletedAt != nil {
		t.Fatalf("credential after rollback = %#v", stored)
	}
}

func TestCredentialRemovalFenceBlocksConcurrentEnableAndTombstonesMetadata(t *testing.T) {
	store := openCredentialFenceTestStore(t)
	credential := createTestCredential(t, store, "removal-race")
	if err := store.SetCredentialEnabled(context.Background(), credential.ID, false); err != nil {
		t.Fatal(err)
	}
	entered := make(chan struct{})
	release := make(chan struct{})
	removalDone := make(chan error, 1)
	go func() {
		removalDone <- store.RemoveCredentialWithMutationFence(
			context.Background(), credential.ID,
			func(string) error {
				close(entered)
				<-release
				return nil
			},
		)
	}()
	select {
	case <-entered:
	case <-time.After(2 * time.Second):
		t.Fatal("removal did not enter fenced callback")
	}
	enableAndCreateDone := make(chan error, 1)
	go func() {
		if err := store.SetCredentialEnabled(context.Background(), credential.ID, true); err != nil {
			enableAndCreateDone <- err
			return
		}
		_, err := store.CreateAPIKey(context.Background(), credential.ID, "Racing client")
		enableAndCreateDone <- err
	}()
	select {
	case err := <-enableAndCreateDone:
		t.Fatalf("concurrent mutation escaped removal fence: %v", err)
	case <-time.After(100 * time.Millisecond):
	}
	close(release)
	if err := <-removalDone; err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-enableAndCreateDone:
		if !errors.Is(err, ErrNotFound) {
			t.Fatalf("concurrent enable error = %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("concurrent enable did not resume after removal")
	}
	stored, err := store.Credential(context.Background(), credential.ID)
	if err != nil {
		t.Fatal(err)
	}
	if stored.Status != CredentialDeleted || stored.DeletedAt == nil {
		t.Fatalf("removed credential = %#v", stored)
	}
	active, err := store.ActiveKeyCount(context.Background(), credential.ID)
	if err != nil || active != 0 {
		t.Fatalf("active keys = %d, %v", active, err)
	}
}

func openCredentialFenceTestStore(t *testing.T) *Store {
	t.Helper()
	clock := &testClock{now: time.Date(2026, 8, 1, 12, 0, 0, 0, time.UTC)}
	store, _ := openTestStore(t, clock)
	return store
}
