package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/cli"
	"mini-sub2api/src/coordinator/internal/storage"
)

type coordinatorFingerprintResult struct {
	ID       string `json:"id"`
	Mode     string `json:"mode"`
	Revision uint64 `json:"revision"`
	Status   string `json:"status,omitempty"`
}

func TestCoordinatorFingerprintModePersistsInCoreOnly(t *testing.T) {
	coreBinary := findCoreBinary(t)
	stateDir := t.TempDir()
	coreStateDir := filepath.Join(stateDir, "core-codex")
	metadata := createCoreCredential(t, coreBinary, []string{
		"credential", "add-api-key", "--state-dir", coreStateDir,
		"--upstream-url", "http://127.0.0.1:1/v1/responses", "--secret-stdin",
	}, upstreamAPIKey+"\n")
	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	credential := persistCredential(t, store, "Fingerprint API", metadata)
	if err := store.SetCredentialEnabled(context.Background(), credential.ID, false); err != nil {
		t.Fatal(err)
	}
	if err := store.Close(); err != nil {
		t.Fatal(err)
	}

	initial := runCoordinatorFingerprint(t, coreBinary, stateDir, credential.ID)
	assertCoordinatorFingerprint(t, initial, credential.ID, "device", 1, "")
	mutated := runCoordinatorFingerprint(
		t, coreBinary, stateDir, credential.ID, "--mode", "off",
	)
	assertCoordinatorFingerprint(
		t, mutated, credential.ID, "off", 2, storage.CredentialDisabled,
	)
	reloaded := runCoordinatorFingerprint(t, coreBinary, stateDir, credential.ID)
	assertCoordinatorFingerprint(t, reloaded, credential.ID, "off", 2, "")

	store, err = storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	stored, err := store.Credential(context.Background(), credential.ID)
	if err != nil {
		t.Fatal(err)
	}
	if stored.Status != storage.CredentialDisabled {
		t.Fatalf("credential status = %q", stored.Status)
	}
}

func runCoordinatorFingerprint(
	t *testing.T,
	coreBinary, stateDir, credentialID string,
	arguments ...string,
) string {
	t.Helper()
	commandArguments := []string{
		"--state-dir", stateDir, "--core-binary", coreBinary, "--json",
		"credential", "fingerprint", credentialID,
	}
	commandArguments = append(commandArguments, arguments...)
	var stdout, stderr bytes.Buffer
	if err := cli.Run(context.Background(), commandArguments, cli.Environment{
		Stdin: strings.NewReader(""), Stdout: &stdout, Stderr: &stderr,
	}); err != nil {
		t.Fatalf("coordinator fingerprint: %v; stderr=%s", err, stderr.String())
	}
	return stdout.String()
}

func assertCoordinatorFingerprint(
	t *testing.T,
	output, credentialID, mode string,
	revision uint64,
	status string,
) {
	t.Helper()
	var result coordinatorFingerprintResult
	if err := json.Unmarshal([]byte(output), &result); err != nil {
		t.Fatal(err)
	}
	if result.ID != credentialID || result.Mode != mode || result.Revision != revision ||
		result.Status != status {
		t.Fatalf("fingerprint result = %#v", result)
	}
	uuid := regexp.MustCompile(`(?i)[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}`)
	if uuid.MatchString(output) || strings.Contains(output, "installation") ||
		strings.Contains(output, "acct_") {
		t.Fatalf("private core identity appeared in coordinator output: %q", output)
	}
}
