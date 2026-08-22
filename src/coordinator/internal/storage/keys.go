package storage

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/base64"
	"fmt"
	"strings"
)

const downstreamKeyBytes = 32

func (s *Store) CreateAPIKey(
	ctx context.Context,
	credentialID, name string,
) (CreatedAPIKey, error) {
	if err := validateLabel(name); err != nil {
		return CreatedAPIKey{}, err
	}
	randomPart, err := newID("", downstreamKeyBytes)
	if err != nil {
		return CreatedAPIKey{}, err
	}
	secret := "ms2a_" + randomPart
	hash := sha256.Sum256([]byte(secret))
	id, err := newID("key_", 16)
	if err != nil {
		return CreatedAPIKey{}, err
	}
	now := s.clock().UTC()
	key := APIKey{
		ID:           id,
		Name:         name,
		Prefix:       secret[:13],
		CredentialID: credentialID,
		Status:       KeyActive,
		CreatedAt:    now,
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return CreatedAPIKey{}, fmt.Errorf("begin API key creation: %w", err)
	}
	defer tx.Rollback()
	var status string
	if err := tx.QueryRowContext(ctx,
		`SELECT status FROM credentials WHERE id = ? AND deleted_at IS NULL`, credentialID,
	).Scan(&status); err == sql.ErrNoRows {
		return CreatedAPIKey{}, ErrNotFound
	} else if err != nil {
		return CreatedAPIKey{}, fmt.Errorf("resolve credential: %w", err)
	}
	if status != CredentialEnabled {
		return CreatedAPIKey{}, ErrConflict
	}
	_, err = tx.ExecContext(ctx, `
        INSERT INTO api_keys(
            id, name, display_prefix, key_hash, credential_id, status, created_at
        ) VALUES (?, ?, ?, ?, ?, 'active', ?)`,
		key.ID, key.Name, key.Prefix, hash[:], key.CredentialID, timestamp(now),
	)
	if err != nil {
		return CreatedAPIKey{}, fmt.Errorf("store API key: %w", err)
	}
	if err := tx.Commit(); err != nil {
		return CreatedAPIKey{}, fmt.Errorf("commit API key creation: %w", err)
	}
	return CreatedAPIKey{APIKey: key, Secret: secret}, nil
}

func (s *Store) APIKeys(ctx context.Context) ([]APIKey, error) {
	rows, err := s.db.QueryContext(ctx, `
        SELECT id, name, display_prefix, credential_id, status, created_at, revoked_at
        FROM api_keys ORDER BY created_at, id`)
	if err != nil {
		return nil, fmt.Errorf("list API keys: %w", err)
	}
	defer rows.Close()
	keys := make([]APIKey, 0)
	for rows.Next() {
		key, err := scanAPIKey(rows)
		if err != nil {
			return nil, err
		}
		keys = append(keys, key)
	}
	return keys, rows.Err()
}

func (s *Store) RevokeAPIKey(ctx context.Context, id string) error {
	now := timestamp(s.clock())
	result, err := s.db.ExecContext(ctx, `
        UPDATE api_keys SET status = 'revoked', revoked_at = ?
        WHERE id = ? AND status = 'active'`, now, id,
	)
	if err != nil {
		return fmt.Errorf("revoke API key: %w", err)
	}
	return requireChanged(result)
}

func (s *Store) AuthenticateAndStart(
	ctx context.Context,
	secret, requestID string,
) (Route, error) {
	if !validDownstreamKey(secret) || !strings.HasPrefix(requestID, "req_") || len(requestID) > 132 {
		return Route{}, ErrUnauthorized
	}
	hash := sha256.Sum256([]byte(secret))
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return Route{}, fmt.Errorf("begin request authentication: %w", err)
	}
	defer tx.Rollback()
	var route Route
	err = tx.QueryRowContext(ctx, `
        SELECT k.id, c.id, c.adapter, c.auth_kind, c.account_ref
        FROM api_keys k JOIN credentials c ON c.id = k.credential_id
        WHERE k.key_hash = ? AND k.status = 'active'
          AND c.status = 'enabled' AND c.deleted_at IS NULL`, hash[:],
	).Scan(&route.APIKeyID, &route.CredentialID, &route.Adapter, &route.AuthKind, &route.AccountRef)
	if err == sql.ErrNoRows {
		return Route{}, ErrUnauthorized
	}
	if err != nil {
		return Route{}, fmt.Errorf("authenticate API key: %w", err)
	}
	_, err = tx.ExecContext(ctx, `
        INSERT INTO requests(
            request_id, api_key_id, credential_id_snapshot, started_at, terminal_status,
            transport, operation_kind
        ) VALUES (?, ?, ?, ?, 'in_progress', 'http', 'inference')`,
		requestID, route.APIKeyID, route.CredentialID, timestamp(s.clock()),
	)
	if err != nil {
		return Route{}, fmt.Errorf("start request history: %w", err)
	}
	if err := tx.Commit(); err != nil {
		return Route{}, fmt.Errorf("commit request authentication: %w", err)
	}
	return route, nil
}

func (s *Store) AuthenticateConnection(ctx context.Context, secret string) (Route, error) {
	if !validDownstreamKey(secret) {
		return Route{}, ErrUnauthorized
	}
	hash := sha256.Sum256([]byte(secret))
	var route Route
	err := s.db.QueryRowContext(ctx, `
        SELECT k.id, c.id, c.adapter, c.auth_kind, c.account_ref
        FROM api_keys k JOIN credentials c ON c.id = k.credential_id
        WHERE k.key_hash = ? AND k.status = 'active'
          AND c.status = 'enabled' AND c.deleted_at IS NULL`, hash[:],
	).Scan(&route.APIKeyID, &route.CredentialID, &route.Adapter, &route.AuthKind, &route.AccountRef)
	if err == sql.ErrNoRows {
		return Route{}, ErrUnauthorized
	}
	if err != nil {
		return Route{}, fmt.Errorf("authenticate WebSocket API key: %w", err)
	}
	return route, nil
}

func (s *Store) StartWebSocketOperation(
	ctx context.Context,
	route Route,
	requestID, operationKind string,
) error {
	if !strings.HasPrefix(requestID, "req_") || len(requestID) > 132 ||
		(operationKind != OperationInference && operationKind != OperationWebSocketPrewarm) {
		return fmt.Errorf("invalid WebSocket operation")
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("begin WebSocket operation: %w", err)
	}
	defer tx.Rollback()
	var eligible int
	err = tx.QueryRowContext(ctx, `
        SELECT 1
        FROM api_keys k JOIN credentials c ON c.id = k.credential_id
        WHERE k.id = ? AND c.id = ? AND c.adapter = ? AND c.auth_kind = ?
          AND c.account_ref = ? AND k.status = 'active'
          AND c.status = 'enabled' AND c.deleted_at IS NULL`,
		route.APIKeyID, route.CredentialID, route.Adapter, route.AuthKind, route.AccountRef,
	).Scan(&eligible)
	if err == sql.ErrNoRows {
		return ErrUnauthorized
	}
	if err != nil {
		return fmt.Errorf("revalidate WebSocket route: %w", err)
	}
	_, err = tx.ExecContext(ctx, `
        INSERT INTO requests(
            request_id, api_key_id, credential_id_snapshot, started_at, terminal_status,
            transport, operation_kind
        ) VALUES (?, ?, ?, ?, 'in_progress', 'websocket', ?)`,
		requestID, route.APIKeyID, route.CredentialID, timestamp(s.clock()), operationKind,
	)
	if err != nil {
		return fmt.Errorf("start WebSocket operation history: %w", err)
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("commit WebSocket operation: %w", err)
	}
	return nil
}

func validDownstreamKey(secret string) bool {
	if !strings.HasPrefix(secret, "ms2a_") {
		return false
	}
	decoded, err := base64.RawURLEncoding.DecodeString(strings.TrimPrefix(secret, "ms2a_"))
	return err == nil && len(decoded) == downstreamKeyBytes
}

func scanAPIKey(row rowScanner) (APIKey, error) {
	var key APIKey
	var createdAt string
	var revokedAt sql.NullString
	err := row.Scan(
		&key.ID, &key.Name, &key.Prefix, &key.CredentialID, &key.Status,
		&createdAt, &revokedAt,
	)
	if err != nil {
		return APIKey{}, err
	}
	key.CreatedAt, err = parseTimestamp(createdAt)
	if err != nil {
		return APIKey{}, err
	}
	key.RevokedAt, err = optionalTimestamp(revokedAt)
	return key, err
}
