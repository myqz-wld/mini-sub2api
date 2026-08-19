package storage

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

func TestFreshMigrationHasExpectedVersionAndNoBodyColumns(t *testing.T) {
	clock := &testClock{now: time.Date(2026, 8, 1, 0, 0, 0, 0, time.UTC)}
	store, stateDir := openTestStore(t, clock)
	var version int
	if err := store.db.QueryRowContext(context.Background(),
		`SELECT version FROM schema_meta WHERE singleton = 1`,
	).Scan(&version); err != nil {
		t.Fatal(err)
	}
	if version != schemaVersion {
		t.Fatalf("schema version = %d, want %d", version, schemaVersion)
	}
	rows, err := store.db.QueryContext(context.Background(), `PRAGMA table_info(requests)`)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	for rows.Next() {
		var cid, notNull, primaryKey int
		var name, dataType string
		var defaultValue any
		if err := rows.Scan(&cid, &name, &dataType, &notNull, &defaultValue, &primaryKey); err != nil {
			t.Fatal(err)
		}
		lower := strings.ToLower(name)
		if strings.Contains(lower, "body") || strings.Contains(lower, "prompt") || lower == "response" {
			t.Fatalf("request-content column must not exist: %s", name)
		}
	}
	if runtime.GOOS != "windows" {
		info, err := os.Stat(filepath.Join(stateDir, "coordinator.sqlite3"))
		if err != nil {
			t.Fatal(err)
		}
		if mode := info.Mode().Perm(); mode != 0o600 {
			t.Fatalf("database mode = %o, want 600", mode)
		}
	}
}

func TestDatabasePathSupportsURICharactersAndSpaces(t *testing.T) {
	stateDir := filepath.Join(t.TempDir(), "state with spaces ? and #")
	store, err := Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	if _, err := os.Stat(filepath.Join(stateDir, "coordinator.sqlite3")); err != nil {
		t.Fatal(err)
	}
}
