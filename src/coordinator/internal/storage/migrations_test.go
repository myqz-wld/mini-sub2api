package storage

import (
	"context"
	"database/sql"
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
	foundProviderRequestID := false
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
		if lower == "provider_request_id" {
			foundProviderRequestID = true
			if dataType != "TEXT" || notNull != 0 {
				t.Fatalf("provider_request_id shape = %s/not-null-%d", dataType, notNull)
			}
		}
	}
	if !foundProviderRequestID {
		t.Fatal("provider_request_id column is missing")
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

func TestVersionOneDatabaseUpgradesWebSocketMetadataTransactionally(t *testing.T) {
	stateDir := t.TempDir()
	databasePath := filepath.Join(stateDir, "coordinator.sqlite3")
	database, err := sql.Open("sqlite", databasePath)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := database.Exec(migrations[0]); err != nil {
		database.Close()
		t.Fatal(err)
	}
	if _, err := database.Exec(`UPDATE schema_meta SET version = 1 WHERE singleton = 1`); err != nil {
		database.Close()
		t.Fatal(err)
	}
	timestamp := "2026-08-01T00:00:00.000000000Z"
	if _, err := database.Exec(`
        INSERT INTO credentials(
            id, name, adapter, auth_kind, account_ref, status, created_at, updated_at
        ) VALUES ('cred_upgrade', 'Upgrade', 'codex', 'openai_api_key', 'acct_upgrade',
                  'enabled', ?, ?);
        INSERT INTO api_keys(
            id, name, display_prefix, key_hash, credential_id, status, created_at
        ) VALUES ('key_upgrade', 'Upgrade', 'ms2a_upgrade', zeroblob(32), 'cred_upgrade',
                  'active', ?);
        INSERT INTO requests(
            request_id, api_key_id, credential_id_snapshot, started_at, terminal_status
        ) VALUES ('req_upgrade', 'key_upgrade', 'cred_upgrade', ?, 'in_progress');`,
		timestamp, timestamp, timestamp, timestamp,
	); err != nil {
		database.Close()
		t.Fatal(err)
	}
	if err := database.Close(); err != nil {
		t.Fatal(err)
	}

	store, err := Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	var version int
	var transport, operationKind string
	var providerRequestID sql.NullString
	if err := store.db.QueryRow(`
        SELECT m.version, r.transport, r.operation_kind, r.provider_request_id
        FROM schema_meta m JOIN requests r ON r.request_id = 'req_upgrade'
        WHERE m.singleton = 1`,
	).Scan(&version, &transport, &operationKind, &providerRequestID); err != nil {
		t.Fatal(err)
	}
	if version != 3 || transport != TransportHTTP || operationKind != OperationInference || providerRequestID.Valid {
		t.Fatalf("upgraded values = %d/%q/%q/%#v", version, transport, operationKind, providerRequestID)
	}
}

func TestVersionTwoDatabaseUpgradesProviderRequestIDTransactionally(t *testing.T) {
	stateDir := t.TempDir()
	databasePath := filepath.Join(stateDir, "coordinator.sqlite3")
	database, err := sql.Open("sqlite", databasePath)
	if err != nil {
		t.Fatal(err)
	}
	for index := 0; index < 2; index++ {
		if _, err := database.Exec(migrations[index]); err != nil {
			database.Close()
			t.Fatal(err)
		}
	}
	if _, err := database.Exec(`UPDATE schema_meta SET version = 2 WHERE singleton = 1`); err != nil {
		database.Close()
		t.Fatal(err)
	}
	if err := database.Close(); err != nil {
		t.Fatal(err)
	}

	store, err := Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	var version int
	if err := store.db.QueryRow(`SELECT version FROM schema_meta WHERE singleton = 1`).Scan(&version); err != nil {
		t.Fatal(err)
	}
	if version != 3 {
		t.Fatalf("schema version = %d, want 3", version)
	}
	var columnCount int
	if err := store.db.QueryRow(`
        SELECT count(*) FROM pragma_table_info('requests') WHERE name = 'provider_request_id'`,
	).Scan(&columnCount); err != nil {
		t.Fatal(err)
	}
	if columnCount != 1 {
		t.Fatalf("provider_request_id column count = %d", columnCount)
	}
}

func TestFailedVersionThreeMigrationRollsBackSchemaVersionForRecovery(t *testing.T) {
	stateDir := t.TempDir()
	databasePath := filepath.Join(stateDir, "coordinator.sqlite3")
	database, err := sql.Open("sqlite", databasePath)
	if err != nil {
		t.Fatal(err)
	}
	for index := 0; index < 2; index++ {
		if _, err := database.Exec(migrations[index]); err != nil {
			database.Close()
			t.Fatal(err)
		}
	}
	if _, err := database.Exec(`
		ALTER TABLE requests ADD COLUMN provider_request_id TEXT;
		UPDATE schema_meta SET version = 2 WHERE singleton = 1;`); err != nil {
		database.Close()
		t.Fatal(err)
	}
	if err := database.Close(); err != nil {
		t.Fatal(err)
	}

	if store, err := Open(context.Background(), stateDir, time.Now); err == nil {
		store.Close()
		t.Fatal("conflicting migration unexpectedly succeeded")
	}
	database, err = sql.Open("sqlite", databasePath)
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	var version, columnCount int
	if err := database.QueryRow(`SELECT version FROM schema_meta WHERE singleton = 1`).Scan(&version); err != nil {
		t.Fatal(err)
	}
	if err := database.QueryRow(`
		SELECT count(*) FROM pragma_table_info('requests') WHERE name = 'provider_request_id'`,
	).Scan(&columnCount); err != nil {
		t.Fatal(err)
	}
	if version != 2 || columnCount != 1 {
		t.Fatalf("failed migration recovery state = version %d, columns %d", version, columnCount)
	}
}
