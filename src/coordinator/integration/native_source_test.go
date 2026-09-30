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
		source = filepath.Join(nativeRepository(), "ref", "sources", "codex-v0.159.2")
	}
	source, err := filepath.Abs(source)
	if err != nil {
		t.Fatal("native source path")
	}
	revision, err := exec.Command("git", "-C", source, "rev-parse", "HEAD").Output()
	if err != nil || strings.TrimSpace(string(revision)) != "ff6aec96948b70d94983af2641a6b67c94faeff5" {
		t.Fatal("native parity requires the exact v0.159.2 source checkout")
	}
	if exec.Command("git", "-C", source, "diff", "--quiet", "HEAD", "--").Run() != nil {
		t.Fatal("native parity source reference has local changes")
	}
	return source
}
