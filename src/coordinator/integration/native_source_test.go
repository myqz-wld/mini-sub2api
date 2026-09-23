//go:build nativeparity

package integration

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func nativeRepository() string {
	_, source, _, _ := runtime.Caller(0)
	return filepath.Clean(filepath.Join(filepath.Dir(source), "..", "..", ".."))
}

func nativeSource(t *testing.T) string {
	t.Helper()
	source := os.Getenv("MINI_SUB2API_CODEX_SOURCE")
	if source == "" {
		source = filepath.Join(nativeRepository(), ".ref", "sources", "codex-v0.156.0")
	}
	source, err := filepath.Abs(source)
	if err != nil {
		t.Fatal("native source path")
	}
	revision, err := exec.Command("git", "-C", source, "rev-parse", "HEAD").Output()
	if err != nil || strings.TrimSpace(string(revision)) != "fe74a774532af67b5a4a3dec03ce9469e17f89af" {
		t.Fatal("native parity requires the exact v0.156.0 source checkout")
	}
	if exec.Command("git", "-C", source, "diff", "--quiet", "HEAD", "--", "codex-rs/models-manager").Run() != nil {
		t.Fatal("native parity model reference has local changes")
	}
	return source
}
