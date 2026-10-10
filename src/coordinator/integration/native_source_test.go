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
		source = filepath.Join(nativeRepository(), "ref", "sources", "codex-v0.162.0")
	}
	source, err := filepath.Abs(source)
	if err != nil {
		t.Fatal("native source path")
	}
	revision, err := exec.Command("git", "-C", source, "rev-parse", "HEAD").Output()
	if err != nil || strings.TrimSpace(string(revision)) != "c1382380de69521303b416720a52f42d51af6248" {
		t.Fatal("native parity requires the exact v0.162.0 source checkout")
	}
	if exec.Command("git", "-C", source, "diff", "--quiet", "HEAD", "--").Run() != nil {
		t.Fatal("native parity source reference has local changes")
	}
	return source
}
