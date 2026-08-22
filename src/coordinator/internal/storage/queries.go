package storage

import (
	"context"
	"database/sql"
	"fmt"
	"strings"
	"time"
)

func (s *Store) History(
	ctx context.Context,
	apiKeyID string,
	since *time.Time,
	limit int,
) ([]RequestRecord, error) {
	if limit <= 0 || limit > 1000 {
		return nil, fmt.Errorf("history limit must be between 1 and 1000")
	}
	query := `
        SELECT request_id, api_key_id, credential_id_snapshot, transport, operation_kind,
               started_at, completed_at,
               terminal_status, http_status, ttfb_ms, duration_ms, input_tokens,
               cached_input_tokens, cache_write_input_tokens, output_tokens,
               reasoning_output_tokens, total_tokens
        FROM requests WHERE 1 = 1`
	var arguments []any
	if apiKeyID != "" {
		query += ` AND api_key_id = ?`
		arguments = append(arguments, apiKeyID)
	}
	if since != nil {
		query += ` AND started_at >= ?`
		arguments = append(arguments, timestamp(*since))
	}
	query += ` ORDER BY started_at DESC, request_id DESC LIMIT ?`
	arguments = append(arguments, limit)
	rows, err := s.db.QueryContext(ctx, query, arguments...)
	if err != nil {
		return nil, fmt.Errorf("query request history: %w", err)
	}
	defer rows.Close()
	records := make([]RequestRecord, 0)
	for rows.Next() {
		record, err := scanRequestRecord(rows)
		if err != nil {
			return nil, err
		}
		records = append(records, record)
	}
	return records, rows.Err()
}

func (s *Store) Stats(
	ctx context.Context,
	apiKeyID, sinceDay, untilDay string,
) ([]DailyUsage, error) {
	query := `
        SELECT day, api_key_id, request_count, completed_count, error_count,
               disconnected_count, duration_ms, usage_observation_count,
               input_tokens, cached_input_tokens, cache_write_input_tokens,
               output_tokens, reasoning_output_tokens, total_tokens
        FROM daily_usage WHERE 1 = 1`
	var arguments []any
	if apiKeyID != "" {
		query += ` AND api_key_id = ?`
		arguments = append(arguments, apiKeyID)
	}
	if sinceDay != "" {
		query += ` AND day >= ?`
		arguments = append(arguments, sinceDay)
	}
	if untilDay != "" {
		query += ` AND day <= ?`
		arguments = append(arguments, untilDay)
	}
	query += ` ORDER BY day, api_key_id`
	rows, err := s.db.QueryContext(ctx, query, arguments...)
	if err != nil {
		return nil, fmt.Errorf("query daily usage: %w", err)
	}
	defer rows.Close()
	entries := make([]DailyUsage, 0)
	for rows.Next() {
		var entry DailyUsage
		var usage TokenUsage
		if err := rows.Scan(
			&entry.Day, &entry.APIKeyID, &entry.RequestCount, &entry.CompletedCount,
			&entry.ErrorCount, &entry.DisconnectedCount, &entry.DurationMilliseconds,
			&entry.UsageObservationCount, &usage.InputTokens, &usage.CachedInputTokens,
			&usage.CacheWriteInputTokens, &usage.OutputTokens,
			&usage.ReasoningOutputTokens, &usage.TotalTokens,
		); err != nil {
			return nil, err
		}
		if entry.UsageObservationCount > 0 {
			entry.Usage = &usage
		}
		entries = append(entries, entry)
	}
	return entries, rows.Err()
}

func scanRequestRecord(row rowScanner) (RequestRecord, error) {
	var record RequestRecord
	var startedAt string
	var completedAt sql.NullString
	var httpStatus, ttfb, duration sql.NullInt64
	var usageFields [6]sql.NullInt64
	err := row.Scan(
		&record.RequestID, &record.APIKeyID, &record.CredentialID,
		&record.Transport, &record.OperationKind, &startedAt,
		&completedAt, &record.Status, &httpStatus, &ttfb, &duration,
		&usageFields[0], &usageFields[1], &usageFields[2], &usageFields[3],
		&usageFields[4], &usageFields[5],
	)
	if err != nil {
		return RequestRecord{}, err
	}
	record.StartedAt, err = parseTimestamp(startedAt)
	if err != nil {
		return RequestRecord{}, err
	}
	record.CompletedAt, err = optionalTimestamp(completedAt)
	if err != nil {
		return RequestRecord{}, err
	}
	record.HTTPStatus = optionalInt(httpStatus)
	record.TTFBMilliseconds = optionalInt64(ttfb)
	record.DurationMilliseconds = optionalInt64(duration)
	if usageFields[0].Valid {
		record.Usage = &TokenUsage{
			InputTokens: usageFields[0].Int64, CachedInputTokens: usageFields[1].Int64,
			CacheWriteInputTokens: usageFields[2].Int64, OutputTokens: usageFields[3].Int64,
			ReasoningOutputTokens: usageFields[4].Int64, TotalTokens: usageFields[5].Int64,
		}
	}
	return record, nil
}

func optionalInt(value sql.NullInt64) *int {
	if !value.Valid {
		return nil
	}
	converted := int(value.Int64)
	return &converted
}

func optionalInt64(value sql.NullInt64) *int64 {
	if !value.Valid {
		return nil
	}
	return &value.Int64
}

func ValidateDay(value string) error {
	if strings.TrimSpace(value) == "" {
		return nil
	}
	if _, err := time.Parse(time.DateOnly, value); err != nil {
		return fmt.Errorf("date must use YYYY-MM-DD")
	}
	return nil
}
