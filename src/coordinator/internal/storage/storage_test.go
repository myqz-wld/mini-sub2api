package storage

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

type testClock struct {
	mu  sync.Mutex
	now time.Time
}

func (c *testClock) Time() time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.now
}

func (c *testClock) Set(value time.Time) {
	c.mu.Lock()
	c.now = value
	c.mu.Unlock()
}

func openTestStore(t *testing.T, clock *testClock) (*Store, string) {
	t.Helper()
	stateDir := t.TempDir()
	store, err := Open(context.Background(), stateDir, clock.Time)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_ = store.Close()
	})
	return store, stateDir
}

func createTestCredential(t *testing.T, store *Store, suffix string) Credential {
	t.Helper()
	credential, err := store.CreateCredential(
		context.Background(), "Credential "+suffix, "codex", "openai_api_key",
		"acct_"+suffix, nil,
	)
	if err != nil {
		t.Fatal(err)
	}
	return credential
}

func TestAPIKeyStoredAsHashAndMappingIsImmutable(t *testing.T) {
	clock := &testClock{now: time.Date(2026, 8, 1, 12, 0, 0, 0, time.UTC)}
	store, stateDir := openTestStore(t, clock)
	credential := createTestCredential(t, store, "first")
	other := createTestCredential(t, store, "second")
	created, err := store.CreateAPIKey(context.Background(), credential.ID, "Test key")
	if err != nil {
		t.Fatal(err)
	}
	if created.Secret == "" || created.Prefix == created.Secret {
		t.Fatalf("expected one-time secret and short prefix: %#v", created)
	}
	if _, err := store.AuthenticateAndStart(
		context.Background(), created.Secret, "req_auth_test",
	); err != nil {
		t.Fatalf("authenticate generated key: %v", err)
	}
	if _, err := store.db.ExecContext(context.Background(),
		`UPDATE api_keys SET credential_id = ? WHERE id = ?`, other.ID, created.ID,
	); err == nil {
		t.Fatal("immutable key mapping update unexpectedly succeeded")
	}

	if err := store.Close(); err != nil {
		t.Fatal(err)
	}
	files, err := filepath.Glob(filepath.Join(stateDir, "coordinator.sqlite3*"))
	if err != nil {
		t.Fatal(err)
	}
	for _, path := range files {
		contents, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if bytes.Contains(contents, []byte(created.Secret)) {
			t.Fatalf("recoverable downstream key found in %s", filepath.Base(path))
		}
	}
}

func TestRevokedOrDisabledRoutesAreUnauthorized(t *testing.T) {
	clock := &testClock{now: time.Date(2026, 8, 1, 12, 0, 0, 0, time.UTC)}
	store, _ := openTestStore(t, clock)
	credential := createTestCredential(t, store, "route")
	first, err := store.CreateAPIKey(context.Background(), credential.ID, "First")
	if err != nil {
		t.Fatal(err)
	}
	if err := store.RevokeAPIKey(context.Background(), first.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := store.AuthenticateAndStart(
		context.Background(), first.Secret, "req_revoked",
	); err != ErrUnauthorized {
		t.Fatalf("revoked key error = %v", err)
	}
	second, err := store.CreateAPIKey(context.Background(), credential.ID, "Second")
	if err != nil {
		t.Fatal(err)
	}
	if err := store.SetCredentialEnabled(context.Background(), credential.ID, false); err != nil {
		t.Fatal(err)
	}
	if _, err := store.AuthenticateAndStart(
		context.Background(), second.Secret, "req_disabled",
	); err != ErrUnauthorized {
		t.Fatalf("disabled credential error = %v", err)
	}
}

func TestConcurrentKeyCreationUsesDistinctSecrets(t *testing.T) {
	clock := &testClock{now: time.Date(2026, 8, 1, 12, 0, 0, 0, time.UTC)}
	store, _ := openTestStore(t, clock)
	credential := createTestCredential(t, store, "concurrent")
	const count = 24
	secrets := make(chan string, count)
	errors := make(chan error, count)
	var group sync.WaitGroup
	for index := 0; index < count; index++ {
		group.Add(1)
		go func(index int) {
			defer group.Done()
			created, err := store.CreateAPIKey(
				context.Background(), credential.ID, fmt.Sprintf("Key %d", index),
			)
			if err != nil {
				errors <- err
				return
			}
			secrets <- created.Secret
		}(index)
	}
	group.Wait()
	close(errors)
	close(secrets)
	for err := range errors {
		t.Errorf("create key: %v", err)
	}
	seen := map[string]bool{}
	for secret := range secrets {
		if seen[secret] {
			t.Fatalf("duplicate key generated")
		}
		seen[secret] = true
	}
	if len(seen) != count {
		t.Fatalf("created %d keys, want %d", len(seen), count)
	}
}

func TestOnlyOneServiceLockUsesAStateDirectory(t *testing.T) {
	directory := t.TempDir()
	first, err := AcquireServiceLock(directory)
	if err != nil {
		t.Fatal(err)
	}
	defer first.Close()
	if _, err := AcquireServiceLock(directory); err == nil {
		t.Fatal("second service lock unexpectedly succeeded")
	}
}
