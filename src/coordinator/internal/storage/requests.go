package storage

import (
	"context"
	"database/sql"
	"fmt"
	"time"
)

func (s *Store) FinalizeRequest(
	ctx context.Context,
	requestID string,
	result RequestResult,
) error {
	if !validTerminalStatus(result.Status) {
		return fmt.Errorf("invalid terminal request status %q", result.Status)
	}
	if result.ProviderRequestID != nil && !validProviderRequestID(*result.ProviderRequestID) {
		return fmt.Errorf("invalid provider request ID")
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("begin request completion: %w", err)
	}
	defer tx.Rollback()

	var apiKeyID, startedAt, operationKind string
	err = tx.QueryRowContext(ctx, `
        SELECT api_key_id, started_at, operation_kind FROM requests
        WHERE request_id = ? AND terminal_status = 'in_progress'`, requestID,
	).Scan(&apiKeyID, &startedAt, &operationKind)
	if err == sql.ErrNoRows {
		return ErrNotFound
	}
	if err != nil {
		return fmt.Errorf("load in-progress request: %w", err)
	}
	started, err := parseTimestamp(startedAt)
	if err != nil {
		return err
	}
	if result.CompletedAt.IsZero() {
		result.CompletedAt = s.clock().UTC()
	}
	if result.Duration < 0 {
		result.Duration = 0
	}
	if err := completeRequestTx(ctx, tx, requestID, result); err != nil {
		return err
	}
	if operationKind == OperationInference {
		if err := addDailyUsageTx(ctx, tx, started, apiKeyID, result); err != nil {
			return err
		}
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("commit request completion: %w", err)
	}
	return nil
}

func completeRequestTx(
	ctx context.Context,
	tx *sql.Tx,
	requestID string,
	result RequestResult,
) error {
	input, cached, cacheWrite, output, reasoning, total := nullableUsage(result.Usage)
	queryResult, err := tx.ExecContext(ctx, `
        UPDATE requests SET
            completed_at = ?, terminal_status = ?, http_status = ?, ttfb_ms = ?,
            duration_ms = ?, input_tokens = ?, cached_input_tokens = ?,
            cache_write_input_tokens = ?, output_tokens = ?, reasoning_output_tokens = ?,
            total_tokens = ?, provider_request_id = ?, aggregated = 1
        WHERE request_id = ? AND terminal_status = 'in_progress'`,
		timestamp(result.CompletedAt), result.Status, result.HTTPStatus,
		nullableMilliseconds(result.TTFB), result.Duration.Milliseconds(), input, cached,
		cacheWrite, output, reasoning, total, result.ProviderRequestID, requestID,
	)
	if err != nil {
		return fmt.Errorf("complete request history: %w", err)
	}
	return requireChanged(queryResult)
}

func addDailyUsageTx(
	ctx context.Context,
	tx *sql.Tx,
	started time.Time,
	apiKeyID string,
	result RequestResult,
) error {
	completed, errors, disconnected := int64(0), int64(0), int64(0)
	switch result.Status {
	case RequestCompleted:
		completed = 1
	case RequestUpstreamErr:
		errors = 1
	case RequestDisconnected:
		disconnected = 1
	}
	usageCount := int64(0)
	usage := TokenUsage{}
	if result.Usage != nil {
		usageCount = 1
		usage = *result.Usage
	}
	_, err := tx.ExecContext(ctx, `
        INSERT INTO daily_usage(
            day, api_key_id, request_count, completed_count, error_count,
            disconnected_count, duration_ms, usage_observation_count,
            input_tokens, cached_input_tokens, cache_write_input_tokens,
            output_tokens, reasoning_output_tokens, total_tokens
        ) VALUES (?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(day, api_key_id) DO UPDATE SET
            request_count = request_count + 1,
            completed_count = completed_count + excluded.completed_count,
            error_count = error_count + excluded.error_count,
            disconnected_count = disconnected_count + excluded.disconnected_count,
            duration_ms = duration_ms + excluded.duration_ms,
            usage_observation_count = usage_observation_count + excluded.usage_observation_count,
            input_tokens = input_tokens + excluded.input_tokens,
            cached_input_tokens = cached_input_tokens + excluded.cached_input_tokens,
            cache_write_input_tokens = cache_write_input_tokens + excluded.cache_write_input_tokens,
            output_tokens = output_tokens + excluded.output_tokens,
            reasoning_output_tokens = reasoning_output_tokens + excluded.reasoning_output_tokens,
            total_tokens = total_tokens + excluded.total_tokens`,
		started.UTC().Format(time.DateOnly), apiKeyID, completed, errors, disconnected,
		result.Duration.Milliseconds(), usageCount, usage.InputTokens, usage.CachedInputTokens,
		usage.CacheWriteInputTokens, usage.OutputTokens, usage.ReasoningOutputTokens,
		usage.TotalTokens,
	)
	if err != nil {
		return fmt.Errorf("update daily usage: %w", err)
	}
	return nil
}

func nullableUsage(usage *TokenUsage) (any, any, any, any, any, any) {
	if usage == nil {
		return nil, nil, nil, nil, nil, nil
	}
	return usage.InputTokens, usage.CachedInputTokens, usage.CacheWriteInputTokens,
		usage.OutputTokens, usage.ReasoningOutputTokens, usage.TotalTokens
}

func validTerminalStatus(status string) bool {
	return status == RequestCompleted || status == RequestUpstreamErr || status == RequestDisconnected
}

func validProviderRequestID(value string) bool {
	if len(value) == 0 || len(value) > 512 {
		return false
	}
	for index := 0; index < len(value); index++ {
		if value[index] < 0x21 || value[index] > 0x7e {
			return false
		}
	}
	return true
}

func (s *Store) RecoverInFlight(ctx context.Context) (int, error) {
	rows, err := s.db.QueryContext(ctx, `
        SELECT request_id FROM requests WHERE terminal_status = 'in_progress'`)
	if err != nil {
		return 0, fmt.Errorf("find interrupted requests: %w", err)
	}
	var ids []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return 0, err
		}
		ids = append(ids, id)
	}
	if err := rows.Close(); err != nil {
		return 0, err
	}
	for _, id := range ids {
		status := 503
		if err := s.FinalizeRequest(ctx, id, RequestResult{
			CompletedAt: s.clock().UTC(),
			Status:      RequestUpstreamErr,
			HTTPStatus:  &status,
		}); err != nil {
			return 0, err
		}
	}
	return len(ids), nil
}
