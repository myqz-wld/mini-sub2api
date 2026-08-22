package cli

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"path/filepath"
	"time"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/httpapi"
	"mini-sub2api/src/coordinator/internal/storage"
)

func runServe(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	flags := newFlagSet("serve", environment.Stderr)
	listen := flags.String("listen", "127.0.0.1:8787", "public listen address")
	tlsCertificate := flags.String("tls-cert", "", "TLS certificate file")
	tlsKey := flags.String("tls-key", "", "TLS private-key file")
	retentionDays := flags.Int("usage-retention-days", 7, "request-detail retention in days; 0 disables")
	if err := flags.Parse(arguments); err != nil {
		return err
	}
	if flags.NArg() != 0 {
		return fmt.Errorf("serve accepts no positional arguments")
	}
	if *retentionDays < 0 || *retentionDays > 36500 {
		return fmt.Errorf("--usage-retention-days must be between 0 and 36500")
	}
	listener, err := httpapi.OpenListener(*listen, *tlsCertificate, *tlsKey)
	if err != nil {
		return err
	}
	defer listener.Close()
	store, err := openStore(ctx, options)
	if err != nil {
		return err
	}
	defer store.Close()
	serviceLock, err := storage.AcquireServiceLock(options.stateDir)
	if err != nil {
		return err
	}
	defer serviceLock.Close()
	if _, err := store.RecoverInFlight(ctx); err != nil {
		return err
	}
	if *retentionDays > 0 {
		if _, err := store.PruneDetailsBefore(ctx, time.Now().UTC().Add(-time.Duration(*retentionDays)*24*time.Hour)); err != nil {
			return err
		}
	}
	binary, err := resolveCoreBinary(options.coreBinary)
	if err != nil {
		return err
	}
	supervisor, err := adapter.Start(ctx, adapter.Config{
		Binary: binary, StateDir: filepath.Join(options.stateDir, "core-codex"),
	})
	if err != nil {
		return err
	}
	defer supervisor.Close()
	go retentionLoop(ctx, store, *retentionDays, environment.Stderr)
	handler := httpapi.NewHandler(store, supervisor, logger(environment.Stderr))
	server := &http.Server{
		Handler:           handler,
		ReadTimeout:       30 * time.Second,
		ReadHeaderTimeout: 10 * time.Second,
		IdleTimeout:       120 * time.Second,
		MaxHeaderBytes:    1 << 20,
	}
	server.RegisterOnShutdown(handler.ShutdownWebSockets)
	scheme := "http"
	if listener.TLS {
		scheme = "https"
	}
	fmt.Fprintf(environment.Stderr, "mini-sub2api listening on %s://%s (TLS %s)\n", scheme, listener.Addr(), tlsState(listener.TLS))
	serveErr := httpapi.Serve(ctx, server, listener)
	handler.ShutdownWebSockets()
	return serveErr
}

func retentionLoop(ctx context.Context, store interface {
	PruneDetailsBefore(context.Context, time.Time) (int64, error)
}, retentionDays int, errorOutput io.Writer) {
	if retentionDays == 0 {
		return
	}
	ticker := time.NewTicker(24 * time.Hour)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case now := <-ticker.C:
			if _, err := store.PruneDetailsBefore(ctx, now.UTC().Add(-time.Duration(retentionDays)*24*time.Hour)); err != nil {
				fmt.Fprintf(errorOutput, "mini-sub2api: usage retention failed: %v\n", err)
			}
		}
	}
}

func tlsState(enabled bool) string {
	if enabled {
		return "enabled"
	}
	return "disabled (loopback only)"
}
