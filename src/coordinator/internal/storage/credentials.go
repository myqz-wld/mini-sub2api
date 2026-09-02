package storage

import (
	"context"
	"database/sql"
	"fmt"
	"strings"
)

func (s *Store) CreateCredential(
	ctx context.Context,
	name, adapter, authKind, accountRef string,
	upstreamAccountID *string,
) (Credential, error) {
	if err := validateLabel(name); err != nil {
		return Credential{}, err
	}
	if adapter != "codex" {
		return Credential{}, fmt.Errorf("unsupported adapter %q", adapter)
	}
	if authKind != "codex_oauth" && authKind != "openai_api_key" {
		return Credential{}, fmt.Errorf("unsupported credential kind %q", authKind)
	}
	if !strings.HasPrefix(accountRef, "acct_") || len(accountRef) > 133 {
		return Credential{}, fmt.Errorf("invalid core account reference")
	}
	id, err := newID("cred_", 16)
	if err != nil {
		return Credential{}, err
	}
	now := s.clock().UTC()
	credential := Credential{
		ID:                id,
		Name:              name,
		Adapter:           adapter,
		AuthKind:          authKind,
		AccountRef:        accountRef,
		UpstreamAccountID: upstreamAccountID,
		Status:            CredentialEnabled,
		CreatedAt:         now,
		UpdatedAt:         now,
	}
	_, err = s.db.ExecContext(ctx, `
        INSERT INTO credentials(
            id, name, adapter, auth_kind, account_ref, upstream_account_id,
            status, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		credential.ID, credential.Name, credential.Adapter, credential.AuthKind,
		credential.AccountRef, credential.UpstreamAccountID, credential.Status,
		timestamp(now), timestamp(now),
	)
	if err != nil {
		return Credential{}, fmt.Errorf("store credential metadata: %w", err)
	}
	return credential, nil
}

func (s *Store) Credential(ctx context.Context, id string) (Credential, error) {
	row := s.db.QueryRowContext(ctx, credentialSelect+` WHERE id = ?`, id)
	credential, err := scanCredential(row)
	if err == sql.ErrNoRows {
		return Credential{}, ErrNotFound
	}
	return credential, err
}

func (s *Store) Credentials(ctx context.Context) ([]Credential, error) {
	rows, err := s.db.QueryContext(ctx, credentialSelect+` ORDER BY created_at, id`)
	if err != nil {
		return nil, fmt.Errorf("list credentials: %w", err)
	}
	defer rows.Close()
	credentials := make([]Credential, 0)
	for rows.Next() {
		credential, err := scanCredential(rows)
		if err != nil {
			return nil, err
		}
		credentials = append(credentials, credential)
	}
	return credentials, rows.Err()
}

func (s *Store) SetCredentialEnabled(ctx context.Context, id string, enabled bool) error {
	status := CredentialDisabled
	if enabled {
		status = CredentialEnabled
	}
	result, err := s.db.ExecContext(ctx, `
        UPDATE credentials SET status = ?, updated_at = ?
        WHERE id = ? AND deleted_at IS NULL`,
		status, timestamp(s.clock()), id,
	)
	if err != nil {
		return fmt.Errorf("update credential status: %w", err)
	}
	return requireChanged(result)
}

func (s *Store) MarkCredentialRequiresLogin(ctx context.Context, id string) error {
	result, err := s.db.ExecContext(ctx, `
        UPDATE credentials SET status = 'requires_login', updated_at = ?
        WHERE id = ? AND deleted_at IS NULL`,
		timestamp(s.clock()), id,
	)
	if err != nil {
		return fmt.Errorf("mark credential as requiring login: %w", err)
	}
	return requireChanged(result)
}

func (s *Store) DeleteCredentialMetadata(ctx context.Context, id string) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("begin credential removal: %w", err)
	}
	defer tx.Rollback()
	if err := s.deleteCredentialMetadata(ctx, tx, id); err != nil {
		return err
	}
	return tx.Commit()
}

func (s *Store) deleteCredentialMetadata(ctx context.Context, tx *sql.Tx, id string) error {
	var active int
	if err := tx.QueryRowContext(ctx,
		`SELECT count(*) FROM api_keys WHERE credential_id = ? AND status = 'active'`, id,
	).Scan(&active); err != nil {
		return fmt.Errorf("count active API keys: %w", err)
	}
	if active != 0 {
		return ErrConflict
	}
	now := timestamp(s.clock())
	result, err := tx.ExecContext(ctx, `
        UPDATE credentials
        SET status = 'deleted', deleted_at = ?, updated_at = ?
        WHERE id = ? AND deleted_at IS NULL`, now, now, id,
	)
	if err != nil {
		return fmt.Errorf("remove credential metadata: %w", err)
	}
	if err := requireChanged(result); err != nil {
		return err
	}
	return nil
}

func (s *Store) ActiveKeyCount(ctx context.Context, credentialID string) (int, error) {
	var count int
	err := s.db.QueryRowContext(ctx,
		`SELECT count(*) FROM api_keys WHERE credential_id = ? AND status = 'active'`,
		credentialID,
	).Scan(&count)
	return count, err
}

func (s *Store) InFlightCount(ctx context.Context, credentialID string) (int, error) {
	var count int
	err := s.db.QueryRowContext(ctx, `
        SELECT count(*) FROM requests
        WHERE credential_id_snapshot = ? AND terminal_status = 'in_progress'`,
		credentialID,
	).Scan(&count)
	return count, err
}

// WithCredentialMutationFence runs a short external credential mutation while an immediate
// SQLite transaction prevents credential enablement or request admission from racing it.
func (s *Store) WithCredentialMutationFence(
	ctx context.Context,
	credentialID string,
	mutate func(accountRef string) error,
) error {
	if mutate == nil {
		return fmt.Errorf("credential mutation callback is required")
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("begin credential mutation fence: %w", err)
	}
	defer tx.Rollback()

	accountRef, err := credentialMutationTarget(ctx, tx, credentialID)
	if err != nil {
		return err
	}
	if err := mutate(accountRef); err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("commit credential mutation fence: %w", err)
	}
	return nil
}

// RemoveCredentialWithMutationFence performs irreversible core cleanup and the matching
// metadata tombstone while one immediate transaction excludes enablement and key creation.
func (s *Store) RemoveCredentialWithMutationFence(
	ctx context.Context,
	credentialID string,
	mutate func(accountRef string) error,
) error {
	if mutate == nil {
		return fmt.Errorf("credential removal callback is required")
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("begin credential removal fence: %w", err)
	}
	defer tx.Rollback()

	accountRef, err := credentialMutationTarget(ctx, tx, credentialID)
	if err != nil {
		return err
	}
	var active int
	if err := tx.QueryRowContext(ctx,
		`SELECT count(*) FROM api_keys WHERE credential_id = ? AND status = 'active'`,
		credentialID,
	).Scan(&active); err != nil {
		return fmt.Errorf("count active API keys: %w", err)
	}
	if active != 0 {
		return fmt.Errorf(
			"credential still has %d active downstream API key(s): %w", active, ErrConflict,
		)
	}
	if err := mutate(accountRef); err != nil {
		return err
	}
	if err := s.deleteCredentialMetadata(ctx, tx, credentialID); err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("commit credential removal fence: %w", err)
	}
	return nil
}

func credentialMutationTarget(
	ctx context.Context,
	tx *sql.Tx,
	credentialID string,
) (string, error) {
	var accountRef, status string
	err := tx.QueryRowContext(ctx, `
        SELECT account_ref, status FROM credentials
        WHERE id = ? AND deleted_at IS NULL`, credentialID,
	).Scan(&accountRef, &status)
	if err == sql.ErrNoRows {
		return "", ErrNotFound
	}
	if err != nil {
		return "", fmt.Errorf("resolve credential mutation target: %w", err)
	}
	if status != CredentialDisabled {
		return "", fmt.Errorf("credential must be disabled before mutation: %w", ErrConflict)
	}

	var inFlight int
	if err := tx.QueryRowContext(ctx, `
        SELECT count(*) FROM requests
        WHERE credential_id_snapshot = ? AND terminal_status = 'in_progress'`,
		credentialID,
	).Scan(&inFlight); err != nil {
		return "", fmt.Errorf("count in-flight credential requests: %w", err)
	}
	if inFlight != 0 {
		return "", fmt.Errorf(
			"credential still has %d in-flight request(s): %w", inFlight, ErrConflict,
		)
	}
	return accountRef, nil
}

const credentialSelect = `
SELECT id, name, adapter, auth_kind, account_ref, upstream_account_id,
       status, created_at, updated_at, deleted_at
FROM credentials`

type rowScanner interface {
	Scan(dest ...any) error
}

func scanCredential(row rowScanner) (Credential, error) {
	var credential Credential
	var upstreamAccountID, deletedAt sql.NullString
	var createdAt, updatedAt string
	err := row.Scan(
		&credential.ID, &credential.Name, &credential.Adapter, &credential.AuthKind,
		&credential.AccountRef, &upstreamAccountID, &credential.Status,
		&createdAt, &updatedAt, &deletedAt,
	)
	if err != nil {
		return Credential{}, err
	}
	if upstreamAccountID.Valid {
		credential.UpstreamAccountID = &upstreamAccountID.String
	}
	credential.CreatedAt, err = parseTimestamp(createdAt)
	if err != nil {
		return Credential{}, err
	}
	credential.UpdatedAt, err = parseTimestamp(updatedAt)
	if err != nil {
		return Credential{}, err
	}
	credential.DeletedAt, err = optionalTimestamp(deletedAt)
	return credential, err
}

func validateLabel(value string) error {
	if strings.TrimSpace(value) == "" || len(value) > 128 {
		return fmt.Errorf("name must contain 1 to 128 characters")
	}
	return nil
}

func requireChanged(result sql.Result) error {
	count, err := result.RowsAffected()
	if err != nil {
		return err
	}
	if count == 0 {
		return ErrNotFound
	}
	return nil
}
