package cli

import (
	"context"
	"fmt"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
)

func runUsage(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	if len(arguments) == 0 {
		return fmt.Errorf("usage command is required")
	}
	switch arguments[0] {
	case "history":
		return usageHistory(ctx, options, arguments[1:], environment)
	case "stats":
		return usageStats(ctx, options, arguments[1:], environment)
	case "prune":
		return usagePrune(ctx, options, arguments[1:], environment)
	case "help", "--help", "-h":
		_, err := fmt.Fprintln(environment.Stdout, "usage commands: history, stats, prune")
		return err
	default:
		return fmt.Errorf("unknown usage command %q", arguments[0])
	}
}

func usageHistory(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	flags := newFlagSet("usage history", environment.Stderr)
	keyID := flags.String("key", "", "downstream API key id")
	sinceRaw := flags.String("since", "", "RFC3339 start time")
	limit := flags.Int("limit", 100, "maximum records")
	if err := flags.Parse(arguments); err != nil {
		return err
	}
	if flags.NArg() != 0 {
		return fmt.Errorf("usage history accepts only flags")
	}
	var since *time.Time
	if *sinceRaw != "" {
		parsed, err := time.Parse(time.RFC3339, *sinceRaw)
		if err != nil {
			return fmt.Errorf("--since must use RFC3339: %w", err)
		}
		since = &parsed
	}
	store, err := openStore(ctx, options)
	if err != nil {
		return err
	}
	defer store.Close()
	records, err := store.History(ctx, *keyID, since, *limit)
	if err != nil {
		return err
	}
	if options.json {
		return writeResult(environment.Stdout, true, records, "")
	}
	if len(records) == 0 {
		_, err = fmt.Fprintln(environment.Stdout, "No request history.")
		return err
	}
	for _, record := range records {
		tokens := "unknown"
		if record.Usage != nil {
			tokens = fmt.Sprintf("%d", record.Usage.TotalTokens)
		}
		duration := "unknown"
		if record.DurationMilliseconds != nil {
			duration = fmt.Sprintf("%dms", *record.DurationMilliseconds)
		}
		providerRequestID := "unknown"
		if record.ProviderRequestID != nil {
			providerRequestID = *record.ProviderRequestID
		}
		if _, err := fmt.Fprintf(
			environment.Stdout, "%s\t%s\t%s\t%s\ttokens=%s\tduration=%s\tprovider_request_id=%s\n",
			record.RequestID, record.APIKeyID, record.StartedAt.Format(time.RFC3339),
			record.Status, tokens, duration, providerRequestID,
		); err != nil {
			return err
		}
	}
	return nil
}

func usageStats(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	flags := newFlagSet("usage stats", environment.Stderr)
	keyID := flags.String("key", "", "downstream API key id")
	since := flags.String("since", "", "first UTC day, YYYY-MM-DD")
	until := flags.String("until", "", "last UTC day, YYYY-MM-DD")
	if err := flags.Parse(arguments); err != nil {
		return err
	}
	if flags.NArg() != 0 {
		return fmt.Errorf("usage stats accepts only flags")
	}
	if err := storage.ValidateDay(*since); err != nil {
		return err
	}
	if err := storage.ValidateDay(*until); err != nil {
		return err
	}
	store, err := openStore(ctx, options)
	if err != nil {
		return err
	}
	defer store.Close()
	stats, err := store.Stats(ctx, *keyID, *since, *until)
	if err != nil {
		return err
	}
	if options.json {
		return writeResult(environment.Stdout, true, stats, "")
	}
	if len(stats) == 0 {
		_, err = fmt.Fprintln(environment.Stdout, "No usage statistics.")
		return err
	}
	for _, entry := range stats {
		tokens := "unknown"
		if entry.Usage != nil {
			tokens = fmt.Sprintf("%d", entry.Usage.TotalTokens)
		}
		if _, err := fmt.Fprintf(
			environment.Stdout,
			"%s\t%s\trequests=%d\terrors=%d\tdisconnected=%d\ttokens=%s\tduration=%dms\n",
			entry.Day, entry.APIKeyID, entry.RequestCount, entry.ErrorCount,
			entry.DisconnectedCount, tokens, entry.DurationMilliseconds,
		); err != nil {
			return err
		}
	}
	return nil
}

func usagePrune(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	flags := newFlagSet("usage prune", environment.Stderr)
	before := flags.String("before", "", "delete records before UTC day, YYYY-MM-DD")
	includeAggregates := flags.Bool("include-aggregates", false, "also delete permanent daily aggregates")
	yes := flags.Bool("yes", false, "confirm deletion")
	if err := flags.Parse(arguments); err != nil {
		return err
	}
	if flags.NArg() != 0 || *before == "" {
		return fmt.Errorf("usage: usage prune --before YYYY-MM-DD [--include-aggregates] [--yes]")
	}
	if err := storage.ValidateDay(*before); err != nil {
		return err
	}
	cutoff, _ := time.Parse(time.DateOnly, *before)
	store, err := openStore(ctx, options)
	if err != nil {
		return err
	}
	defer store.Close()
	preview, err := store.PreviewPrune(ctx, cutoff, *includeAggregates)
	if err != nil {
		return err
	}
	prompt := fmt.Sprintf(
		"Delete %d request detail row(s) and %d daily aggregate row(s)?",
		preview.RequestDetails, preview.DailyAggregates,
	)
	confirmed, err := confirm(environment.Stdin, environment.Stdout, prompt, *yes)
	if err != nil || !confirmed {
		if err != nil {
			return err
		}
		return fmt.Errorf("usage prune cancelled")
	}
	details, err := store.PruneDetailsBefore(ctx, cutoff)
	if err != nil {
		return err
	}
	aggregates := int64(0)
	if *includeAggregates {
		aggregates, err = store.DeleteAggregatesBefore(ctx, *before)
		if err != nil {
			return err
		}
	}
	result := map[string]int64{"requestDetails": details, "dailyAggregates": aggregates}
	return writeResult(
		environment.Stdout, options.json, result,
		fmt.Sprintf("Deleted %d request detail row(s) and %d daily aggregate row(s).", details, aggregates),
	)
}
