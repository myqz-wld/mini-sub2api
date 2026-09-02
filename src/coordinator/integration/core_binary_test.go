package integration

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sync"
	"testing"
	"time"
)

var defaultCoreBuild struct {
	sync.Once
	path string
	err  error
}

func findCoreBinary(t *testing.T) string {
	t.Helper()
	if configured := os.Getenv("MINI_SUB2API_CORE_CODEX_BINARY"); configured != "" {
		path, err := filepath.Abs(configured)
		if err != nil {
			t.Fatalf("resolve explicit Rust core binary: %v", err)
		}
		if err := validateCoreBinary(path); err != nil {
			t.Fatal(err)
		}
		return path
	}
	defaultCoreBuild.Do(func() {
		defaultCoreBuild.path, defaultCoreBuild.err = buildCurrentCore()
	})
	if defaultCoreBuild.err != nil {
		t.Fatalf("build current Rust core for integration tests: %v", defaultCoreBuild.err)
	}
	return defaultCoreBuild.path
}

func buildCurrentCore() (string, error) {
	_, source, _, ok := runtime.Caller(0)
	if !ok {
		return "", fmt.Errorf("resolve integration test source path")
	}
	repositoryRoot := filepath.Clean(filepath.Join(filepath.Dir(source), "..", "..", ".."))
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()
	command := exec.CommandContext(
		ctx, "cargo", "build", "--locked", "-p", "mini-sub2api-core-codex",
	)
	command.Dir = repositoryRoot
	command.Env = append(os.Environ(), "CARGO_NET_OFFLINE=true")
	output, err := command.CombinedOutput()
	if err != nil {
		if ctx.Err() != nil {
			return "", fmt.Errorf("cargo build timed out: %w", ctx.Err())
		}
		const maxOutputBytes = 16 * 1024
		if len(output) > maxOutputBytes {
			output = output[len(output)-maxOutputBytes:]
		}
		return "", fmt.Errorf("cargo build failed: %w\n%s", err, output)
	}
	path := filepath.Join(
		repositoryRoot, "build", "cargo-target", "debug", "mini-sub2api-core-codex",
	)
	if err := validateCoreBinary(path); err != nil {
		return "", err
	}
	return path, nil
}

func validateCoreBinary(path string) error {
	info, err := os.Stat(path)
	if err != nil {
		return fmt.Errorf("Rust core binary is unavailable at %s: %w", path, err)
	}
	if !info.Mode().IsRegular() || info.Mode().Perm()&0o111 == 0 {
		return fmt.Errorf("Rust core binary is not an executable regular file at %s", path)
	}
	return nil
}
