package storage

import (
	"context"
	"crypto/rand"
	"database/sql"
	"encoding/base64"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"time"

	_ "modernc.org/sqlite"
)

var (
	ErrUnauthorized = errors.New("invalid or unavailable API key")
	ErrNotFound     = errors.New("record not found")
	ErrConflict     = errors.New("record conflicts with current state")
)

type Clock func() time.Time

type Store struct {
	db    *sql.DB
	clock Clock
}

const storedTimestampFormat = "2006-01-02T15:04:05.000000000Z07:00"

func Open(ctx context.Context, stateDir string, clock Clock) (*Store, error) {
	if clock == nil {
		clock = time.Now
	}
	if err := os.MkdirAll(stateDir, 0o700); err != nil {
		return nil, fmt.Errorf("create state directory: %w", err)
	}
	if err := os.Chmod(stateDir, 0o700); err != nil {
		return nil, fmt.Errorf("protect state directory: %w", err)
	}
	databasePath := filepath.Join(stateDir, "coordinator.sqlite3")
	databaseURL := &url.URL{Scheme: "file", Path: databasePath}
	query := databaseURL.Query()
	query.Set("_busy_timeout", "5000")
	query.Set("_foreign_keys", "on")
	query.Set("_journal_mode", "WAL")
	query.Set("_synchronous", "FULL")
	query.Set("_txlock", "immediate")
	query.Set("_dqs", "0")
	databaseURL.RawQuery = query.Encode()
	dsn := databaseURL.String()
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, fmt.Errorf("open coordinator database: %w", err)
	}
	db.SetMaxOpenConns(8)
	db.SetMaxIdleConns(4)
	store := &Store{db: db, clock: clock}
	if err := store.migrate(ctx); err != nil {
		db.Close()
		return nil, err
	}
	if err := os.Chmod(databasePath, 0o600); err != nil {
		db.Close()
		return nil, fmt.Errorf("protect coordinator database: %w", err)
	}
	return store, nil
}

func (s *Store) Close() error {
	return s.db.Close()
}

func newID(prefix string, byteCount int) (string, error) {
	bytes := make([]byte, byteCount)
	if _, err := rand.Read(bytes); err != nil {
		return "", fmt.Errorf("generate identifier: %w", err)
	}
	return prefix + base64.RawURLEncoding.EncodeToString(bytes), nil
}

func timestamp(value time.Time) string {
	return value.UTC().Format(storedTimestampFormat)
}

func parseTimestamp(value string) (time.Time, error) {
	parsed, err := time.Parse(time.RFC3339Nano, value)
	if err != nil {
		return time.Time{}, fmt.Errorf("decode stored timestamp: %w", err)
	}
	return parsed, nil
}

func optionalTimestamp(value sql.NullString) (*time.Time, error) {
	if !value.Valid {
		return nil, nil
	}
	parsed, err := parseTimestamp(value.String)
	if err != nil {
		return nil, err
	}
	return &parsed, nil
}

func nullableMilliseconds(value *time.Duration) any {
	if value == nil {
		return nil
	}
	return value.Milliseconds()
}
