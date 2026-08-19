package buildmeta

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestMissingMetadataHasDistinctStatus(t *testing.T) {
	result, ok := Check(filepath.Join(t.TempDir(), "mini-sub2api"), t.TempDir(), "0.1.0", "test")
	if ok || result.Status != "metadata_missing" {
		t.Fatalf("result = %#v, ok=%v", result, ok)
	}
}

func TestAdjacentMetadataDetectsArtifactMismatch(t *testing.T) {
	directory := t.TempDir()
	executable := filepath.Join(directory, "mini-sub2api")
	if err := os.WriteFile(executable, []byte("test"), 0o700); err != nil {
		t.Fatal(err)
	}
	info := Info{
		PackageName: "mini-sub2api", Version: "0.1.0", FullCommit: "commit-a",
		ShortCommit: "commit-a", Branch: "main", BuildTimestamp: time.Now().Format(time.RFC3339),
	}
	if err := Write(filepath.Join(directory, "build-info.json"), info); err != nil {
		t.Fatal(err)
	}
	result, ok := Check(executable, directory, "0.1.0", "commit-b")
	if ok || result.Status != "artifact_mismatch" {
		t.Fatalf("result = %#v, ok=%v", result, ok)
	}
}
