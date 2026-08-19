package integration

import (
	"bytes"
	"context"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/cli"
	"mini-sub2api/src/coordinator/internal/storage"
)

func TestCredentialCleanupRecoversAfterCoreCompletedFirst(t *testing.T) {
	badProxy := "http://127.0.0.1:1"
	for _, name := range []string{"HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"} {
		t.Setenv(name, badProxy)
	}
	t.Setenv("NO_PROXY", "")
	t.Setenv("no_proxy", "")
	coreBinary := findCoreBinary(t)
	upstream := newMockUpstream(t)
	defer upstream.server.Close()
	stateDir := t.TempDir()
	coreStateDir := filepath.Join(stateDir, "core-codex")
	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()

	apiMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "add-api-key", "--state-dir", coreStateDir,
		"--upstream-url", upstream.server.URL + "/responses", "--secret-stdin",
	}, upstreamAPIKey+"\n")
	apiCredential := persistCredential(t, store, "Cleanup API", apiMetadata)
	runCoreCleanup(t, coreBinary, "remove", coreStateDir, apiMetadata.AccountRef)
	runCoordinatorCleanup(t, coreBinary, stateDir, "remove", apiCredential.ID)
	assertCredentialDeleted(t, store, apiCredential.ID)

	oauthMetadata := createCoreCredential(t, coreBinary, []string{
		"credential", "login", "--state-dir", coreStateDir, "--flow", "device",
		"--issuer", upstream.server.URL, "--client-id", "client-e2e-test",
		"--upstream-url", upstream.server.URL + "/responses",
	}, "")
	oauthCredential := persistCredential(t, store, "Cleanup OAuth", oauthMetadata)
	first := coreCleanupCommand(coreBinary, "revoke", coreStateDir, oauthMetadata.AccountRef)
	if err := first.Start(); err != nil {
		t.Fatal(err)
	}
	select {
	case <-upstream.revokeEntered:
	case <-time.After(2 * time.Second):
		t.Fatal("first revoke did not reach upstream")
	}
	second := coreCleanupCommand(coreBinary, "revoke", coreStateDir, oauthMetadata.AccountRef)
	if err := second.Start(); err != nil {
		t.Fatal(err)
	}
	time.Sleep(100 * time.Millisecond)
	close(upstream.revokeRelease)
	if err := first.Wait(); err != nil {
		t.Fatalf("first concurrent revoke: %v", err)
	}
	if err := second.Wait(); err != nil {
		t.Fatalf("second concurrent revoke: %v", err)
	}
	if upstream.revokes.Load() != 1 {
		t.Fatalf("concurrent upstream revoke calls = %d", upstream.revokes.Load())
	}
	runCoordinatorCleanup(t, coreBinary, stateDir, "revoke", oauthCredential.ID)
	if upstream.revokes.Load() != 1 {
		t.Fatalf("recovery repeated upstream revoke; calls = %d", upstream.revokes.Load())
	}
	assertCredentialDeleted(t, store, oauthCredential.ID)
}

func runCoreCleanup(t *testing.T, binary, action, stateDir, accountRef string) {
	t.Helper()
	command := exec.Command(
		binary, "credential", action, "--state-dir", stateDir, accountRef,
	)
	var stdout, stderr bytes.Buffer
	command.Stdout = &stdout
	command.Stderr = &stderr
	if err := command.Run(); err != nil {
		t.Fatalf("core credential %s: %v; stderr=%s", action, err, stderr.String())
	}
}

func coreCleanupCommand(binary, action, stateDir, accountRef string) *exec.Cmd {
	return exec.Command(
		binary, "credential", action, "--state-dir", stateDir, accountRef,
	)
}

func runCoordinatorCleanup(
	t *testing.T,
	coreBinary, stateDir, action, credentialID string,
) {
	t.Helper()
	var stdout, stderr bytes.Buffer
	err := cli.Run(context.Background(), []string{
		"--state-dir", stateDir, "--core-binary", coreBinary,
		"credential", action, credentialID, "--yes",
	}, cli.Environment{
		Stdin: strings.NewReader(""), Stdout: &stdout, Stderr: &stderr,
	})
	if err != nil {
		t.Fatalf("coordinator credential %s: %v; stderr=%s", action, err, stderr.String())
	}
}

func assertCredentialDeleted(t *testing.T, store *storage.Store, credentialID string) {
	t.Helper()
	credential, err := store.Credential(context.Background(), credentialID)
	if err != nil {
		t.Fatal(err)
	}
	if credential.Status != storage.CredentialDeleted || credential.DeletedAt == nil {
		t.Fatalf("credential was not tombstoned: %#v", credential)
	}
}
