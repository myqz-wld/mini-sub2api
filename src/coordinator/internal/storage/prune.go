package storage

import (
	"context"
	"database/sql"
	"fmt"
	"time"
)

type PrunePreview struct {
	RequestDetails  int64 `json:"requestDetails"`
	DailyAggregates int64 `json:"dailyAggregates"`
}

func (s *Store) PreviewPrune(
	ctx context.Context,
	cutoff time.Time,
	includeAggregates bool,
) (PrunePreview, error) {
	var preview PrunePreview
	if err := s.db.QueryRowContext(ctx, `
        SELECT count(*) FROM requests
        WHERE started_at < ? AND terminal_status <> 'in_progress'`, timestamp(cutoff),
	).Scan(&preview.RequestDetails); err != nil {
		return preview, fmt.Errorf("preview request-detail prune: %w", err)
	}
	if includeAggregates {
		if err := s.db.QueryRowContext(ctx, `
            SELECT count(*) FROM daily_usage WHERE day < ?`, cutoff.UTC().Format(time.DateOnly),
		).Scan(&preview.DailyAggregates); err != nil {
			return preview, fmt.Errorf("preview aggregate prune: %w", err)
		}
	}
	return preview, nil
}

func (s *Store) PruneDetailsBefore(ctx context.Context, cutoff time.Time) (int64, error) {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return 0, fmt.Errorf("begin request-detail prune: %w", err)
	}
	defer tx.Rollback()

	pending, err := loadUnaggregatedBefore(ctx, tx, cutoff)
	if err != nil {
		return 0, err
	}
	for _, item := range pending {
		if err := addDailyUsageTx(ctx, tx, item.started, item.apiKeyID, item.result); err != nil {
			return 0, err
		}
		if _, err := tx.ExecContext(ctx,
			`UPDATE requests SET aggregated = 1 WHERE request_id = ?`, item.requestID,
		); err != nil {
			return 0, fmt.Errorf("mark recovered aggregate: %w", err)
		}
	}
	result, err := tx.ExecContext(ctx, `
        DELETE FROM requests
        WHERE started_at < ? AND terminal_status <> 'in_progress' AND aggregated = 1`,
		timestamp(cutoff),
	)
	if err != nil {
		return 0, fmt.Errorf("delete request details: %w", err)
	}
	count, err := result.RowsAffected()
	if err != nil {
		return 0, err
	}
	if err := tx.Commit(); err != nil {
		return 0, fmt.Errorf("commit request-detail prune: %w", err)
	}
	return count, nil
}

func (s *Store) DeleteAggregatesBefore(ctx context.Context, beforeDay string) (int64, error) {
	if err := ValidateDay(beforeDay); err != nil || beforeDay == "" {
		if err != nil {
			return 0, err
		}
		return 0, fmt.Errorf("aggregate cutoff date is required")
	}
	result, err := s.db.ExecContext(ctx, `DELETE FROM daily_usage WHERE day < ?`, beforeDay)
	if err != nil {
		return 0, fmt.Errorf("delete daily aggregates: %w", err)
	}
	return result.RowsAffected()
}

type unaggregatedRequest struct {
	requestID string
	apiKeyID  string
	started   time.Time
	result    RequestResult
}

func loadUnaggregatedBefore(
	ctx context.Context,
	tx *sql.Tx,
	cutoff time.Time,
) ([]unaggregatedRequest, error) {
	rows, err := tx.QueryContext(ctx, `
        SELECT request_id, api_key_id, started_at, completed_at, terminal_status,
               http_status, ttfb_ms, duration_ms, input_tokens, cached_input_tokens,
               cache_write_input_tokens, output_tokens, reasoning_output_tokens, total_tokens
        FROM requests
        WHERE started_at < ? AND terminal_status <> 'in_progress' AND aggregated = 0`,
		timestamp(cutoff),
	)
	if err != nil {
		return nil, fmt.Errorf("find unaggregated request details: %w", err)
	}
	defer rows.Close()
	var items []unaggregatedRequest
	for rows.Next() {
		item, err := scanUnaggregated(rows)
		if err != nil {
			return nil, err
		}
		items = append(items, item)
	}
	return items, rows.Err()
}

func scanUnaggregated(row rowScanner) (unaggregatedRequest, error) {
	var item unaggregatedRequest
	var startedAt, completedAt string
	var status string
	var httpStatus, ttfb sql.NullInt64
	var duration int64
	var usageFields [6]sql.NullInt64
	err := row.Scan(
		&item.requestID, &item.apiKeyID, &startedAt, &completedAt, &status,
		&httpStatus, &ttfb, &duration, &usageFields[0], &usageFields[1],
		&usageFields[2], &usageFields[3], &usageFields[4], &usageFields[5],
	)
	if err != nil {
		return item, err
	}
	item.started, err = parseTimestamp(startedAt)
	if err != nil {
		return item, err
	}
	completed, err := parseTimestamp(completedAt)
	if err != nil {
		return item, err
	}
	item.result = RequestResult{
		CompletedAt: completed,
		Status:      status,
		HTTPStatus:  optionalInt(httpStatus),
		Duration:    time.Duration(duration) * time.Millisecond,
	}
	if ttfb.Valid {
		value := time.Duration(ttfb.Int64) * time.Millisecond
		item.result.TTFB = &value
	}
	if usageFields[0].Valid {
		item.result.Usage = &TokenUsage{
			InputTokens: usageFields[0].Int64, CachedInputTokens: usageFields[1].Int64,
			CacheWriteInputTokens: usageFields[2].Int64, OutputTokens: usageFields[3].Int64,
			ReasoningOutputTokens: usageFields[4].Int64, TotalTokens: usageFields[5].Int64,
		}
	}
	return item, nil
}
