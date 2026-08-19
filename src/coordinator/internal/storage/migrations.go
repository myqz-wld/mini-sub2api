package storage

import (
	"context"
	"database/sql"
	"fmt"
)

const schemaVersion = 1

var migrations = []string{`
CREATE TABLE schema_meta (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    version INTEGER NOT NULL
);
INSERT INTO schema_meta(singleton, version) VALUES (1, 0);

CREATE TABLE credentials (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    adapter TEXT NOT NULL,
    auth_kind TEXT NOT NULL,
    account_ref TEXT NOT NULL UNIQUE,
    upstream_account_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('enabled', 'disabled', 'requires_login', 'deleted')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);

CREATE TABLE api_keys (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    display_prefix TEXT NOT NULL,
    key_hash BLOB NOT NULL UNIQUE CHECK (length(key_hash) = 32),
    credential_id TEXT NOT NULL REFERENCES credentials(id),
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
    created_at TEXT NOT NULL,
    revoked_at TEXT
);
CREATE INDEX api_keys_credential_idx ON api_keys(credential_id);
CREATE TRIGGER api_keys_immutable_credential
BEFORE UPDATE OF credential_id ON api_keys
WHEN OLD.credential_id <> NEW.credential_id
BEGIN
    SELECT RAISE(ABORT, 'api key credential mapping is immutable');
END;

CREATE TABLE requests (
    request_id TEXT PRIMARY KEY,
    api_key_id TEXT NOT NULL REFERENCES api_keys(id),
    credential_id_snapshot TEXT NOT NULL REFERENCES credentials(id),
    started_at TEXT NOT NULL,
    completed_at TEXT,
    terminal_status TEXT NOT NULL CHECK (terminal_status IN
        ('in_progress', 'completed', 'upstream_error', 'client_disconnected')),
    http_status INTEGER,
    ttfb_ms INTEGER,
    duration_ms INTEGER,
    input_tokens INTEGER,
    cached_input_tokens INTEGER,
    cache_write_input_tokens INTEGER,
    output_tokens INTEGER,
    reasoning_output_tokens INTEGER,
    total_tokens INTEGER,
    aggregated INTEGER NOT NULL DEFAULT 0 CHECK (aggregated IN (0, 1))
);
CREATE INDEX requests_key_started_idx ON requests(api_key_id, started_at DESC);
CREATE INDEX requests_started_idx ON requests(started_at);

CREATE TABLE daily_usage (
    day TEXT NOT NULL,
    api_key_id TEXT NOT NULL REFERENCES api_keys(id),
    request_count INTEGER NOT NULL DEFAULT 0,
    completed_count INTEGER NOT NULL DEFAULT 0,
    error_count INTEGER NOT NULL DEFAULT 0,
    disconnected_count INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    usage_observation_count INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    cached_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_output_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(day, api_key_id)
);
`}

func (s *Store) migrate(ctx context.Context) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("begin schema migration: %w", err)
	}
	defer tx.Rollback()

	if _, err := tx.ExecContext(ctx, `CREATE TABLE IF NOT EXISTS schema_probe (id INTEGER)`); err != nil {
		return fmt.Errorf("probe schema: %w", err)
	}
	if _, err := tx.ExecContext(ctx, `DROP TABLE schema_probe`); err != nil {
		return fmt.Errorf("clean schema probe: %w", err)
	}
	current, err := currentSchemaVersion(ctx, tx)
	if err != nil {
		return err
	}
	if current > schemaVersion {
		return fmt.Errorf("database schema %d is newer than supported schema %d", current, schemaVersion)
	}
	for version := current + 1; version <= schemaVersion; version++ {
		if _, err := tx.ExecContext(ctx, migrations[version-1]); err != nil {
			return fmt.Errorf("apply schema migration %d: %w", version, err)
		}
		if _, err := tx.ExecContext(ctx, `UPDATE schema_meta SET version = ? WHERE singleton = 1`, version); err != nil {
			return fmt.Errorf("record schema migration %d: %w", version, err)
		}
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("commit schema migration: %w", err)
	}
	return nil
}

func currentSchemaVersion(ctx context.Context, tx *sql.Tx) (int, error) {
	var table string
	err := tx.QueryRowContext(ctx,
		`SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'schema_meta'`,
	).Scan(&table)
	if err == sql.ErrNoRows {
		return 0, nil
	}
	if err != nil {
		return 0, fmt.Errorf("inspect schema metadata: %w", err)
	}
	var version int
	if err := tx.QueryRowContext(ctx, `SELECT version FROM schema_meta WHERE singleton = 1`).Scan(&version); err != nil {
		return 0, fmt.Errorf("read schema version: %w", err)
	}
	return version, nil
}
