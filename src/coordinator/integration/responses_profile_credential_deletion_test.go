package integration

import (
	"bytes"
	"context"
	"net/http"
	"path/filepath"
	"strings"
	"testing"

	coordinatorcli "mini-sub2api/src/coordinator/internal/cli"
	"mini-sub2api/src/coordinator/internal/storage"
)

func TestCodexProfileCredentialDeletionRemovesOnlyItsIdentityNamespace(t *testing.T) {
	fixture := newResponsesProfileHTTPFixture(t)
	request := `{"model":"gpt-5.4","input":[{"type":"message","id":"msg-delete","role":"user","content":"materialize"}],"stream":false}`
	status, _, _ := publicRequestWithHeaders(
		t, fixture.public, fixture.apiKey, request,
		http.Header{"Originator": []string{"codex_exec"}},
	)
	if status != http.StatusOK {
		t.Fatalf("API-key materialization = %d", status)
	}
	_ = waitForRoutingCapture(t, fixture.captures)
	status, _, _ = publicRequest(t, fixture.public, fixture.subscriptionKey, request)
	if status != http.StatusOK {
		t.Fatalf("subscription materialization = %d", status)
	}
	_ = waitForRoutingCapture(t, fixture.captures)
	assertProfileStateFileCount(t, fixture.coreStateDir, 2)

	if err := fixture.store.RevokeAPIKey(context.Background(), fixture.apiKeyID); err != nil {
		t.Fatal(err)
	}
	runProfileCredentialRemove(t, fixture, fixture.apiCredentialID, false)
	assertProfileStateFileCount(t, fixture.coreStateDir, 1)
	apiCredential, err := fixture.store.Credential(context.Background(), fixture.apiCredentialID)
	if err != nil || apiCredential.Status != storage.CredentialDeleted {
		t.Fatalf("removed API credential = %#v, %v", apiCredential, err)
	}
	status, _, _ = publicRequest(t, fixture.public, fixture.subscriptionKey, request)
	if status != http.StatusOK {
		t.Fatalf("API credential deletion affected subscription namespace: %d", status)
	}
	_ = waitForRoutingCapture(t, fixture.captures)

	if err := fixture.store.RevokeAPIKey(context.Background(), fixture.subscriptionKeyID); err != nil {
		t.Fatal(err)
	}
	runProfileCredentialRemove(t, fixture, fixture.subscriptionCredentialID, true)
	assertProfileStateFileCount(t, fixture.coreStateDir, 0)
	subscriptionCredential, err := fixture.store.Credential(
		context.Background(), fixture.subscriptionCredentialID,
	)
	if err != nil || subscriptionCredential.Status != storage.CredentialDeleted {
		t.Fatalf("removed subscription credential = %#v, %v", subscriptionCredential, err)
	}
}

func runProfileCredentialRemove(
	t *testing.T,
	fixture *responsesProfileHTTPFixture,
	credentialID string,
	forceServiceOnly bool,
) {
	t.Helper()
	arguments := []string{
		"--state-dir", fixture.stateDir,
		"--core-binary", fixture.coreBinary,
		"credential", "remove", credentialID, "--yes",
	}
	if forceServiceOnly {
		arguments = append(arguments, "--force-service-only")
	}
	var stdout, stderr bytes.Buffer
	err := coordinatorcli.Run(context.Background(), arguments, coordinatorcli.Environment{
		Stdin: strings.NewReader(""), Stdout: &stdout, Stderr: &stderr,
	})
	if err != nil {
		t.Fatalf("credential remove: %v; stdout=%s stderr=%s", err, stdout.String(), stderr.String())
	}
}

func assertProfileStateFileCount(t *testing.T, coreStateDir string, want int) {
	t.Helper()
	files, err := filepath.Glob(filepath.Join(coreStateDir, "accounts", "rs_*.request-state.json"))
	if err != nil || len(files) != want {
		t.Fatalf("request-state file count = %d, want %d: %#v, %v", len(files), want, files, err)
	}
}
