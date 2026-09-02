package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
)

func TestMain(m *testing.M) {
	if os.Getenv("MINI_SUB2API_FAKE_CLI_CORE") == "1" {
		os.Exit(runFakeCredentialCore())
	}
	os.Exit(m.Run())
}

func TestCredentialAndKeyCLIPrintsDownstreamSecretOnce(t *testing.T) {
	t.Setenv("MINI_SUB2API_FAKE_CLI_CORE", "1")
	stateDir := t.TempDir()
	coreBinary := os.Args[0]
	upstreamSecret := "upstream-secret-supplied-only-on-stdin"
	credentialOutput := runCLI(t, strings.NewReader(upstreamSecret+"\n"),
		"--state-dir", stateDir, "--core-binary", coreBinary, "--json",
		"credential", "add-api-key", "codex", "--name", "Upstream", "--secret-stdin",
	)
	if strings.Contains(credentialOutput, upstreamSecret) {
		t.Fatal("upstream secret appeared in coordinator output")
	}
	var credential storage.Credential
	if err := json.Unmarshal([]byte(credentialOutput), &credential); err != nil {
		t.Fatal(err)
	}
	if credential.AuthKind != "openai_api_key" {
		t.Fatalf("credential = %#v", credential)
	}

	createdOutput := runCLI(t, nil,
		"--state-dir", stateDir, "key", "create",
		"--credential", credential.ID, "--name", "Client",
	)
	secret := regexp.MustCompile(`ms2a_[A-Za-z0-9_-]{43}`).FindString(createdOutput)
	if secret == "" || strings.Count(createdOutput, secret) != 1 {
		t.Fatalf("one-time key output = %q", createdOutput)
	}
	keyList := runCLI(t, nil, "--state-dir", stateDir, "key", "list")
	if strings.Contains(keyList, secret) || strings.Contains(keyList, upstreamSecret) {
		t.Fatalf("secret appeared in key list: %q", keyList)
	}
	credentialList := runCLI(t, nil, "--state-dir", stateDir, "credential", "list", "--json")
	if strings.Contains(credentialList, upstreamSecret) || strings.Contains(credentialList, secret) {
		t.Fatalf("secret appeared in credential list: %q", credentialList)
	}

	var keys []storage.APIKey
	if err := json.Unmarshal([]byte(runCLI(t, nil,
		"--state-dir", stateDir, "--json", "key", "list",
	)), &keys); err != nil {
		t.Fatal(err)
	}
	if len(keys) != 1 {
		t.Fatalf("keys = %#v", keys)
	}
	runCLI(t, nil, "--state-dir", stateDir, "key", "revoke", keys[0].ID, "--yes")
	runCLI(t, nil,
		"--state-dir", stateDir, "--core-binary", coreBinary,
		"credential", "remove", credential.ID, "--yes",
	)
}

func TestImportCodexAuthPersistsOnlyCoreMetadata(t *testing.T) {
	t.Setenv("MINI_SUB2API_FAKE_CLI_CORE", "1")
	stateDir := t.TempDir()
	authFile := filepath.Join(stateDir, "auth.json")
	upstreamSecret := "refresh-import-must-not-cross-coordinator"
	if err := os.WriteFile(authFile, []byte(upstreamSecret), 0o600); err != nil {
		t.Fatal(err)
	}
	output := runCLI(t, nil,
		"--state-dir", stateDir, "--core-binary", os.Args[0], "--json",
		"credential", "import-codex", "--name", "Subscription", "--auth-file", authFile,
	)
	if strings.Contains(output, upstreamSecret) {
		t.Fatal("imported secret appeared in coordinator output")
	}
	var credential storage.Credential
	if err := json.Unmarshal([]byte(output), &credential); err != nil {
		t.Fatal(err)
	}
	if credential.AuthKind != "codex_oauth" || credential.UpstreamAccountID == nil {
		t.Fatalf("credential = %#v", credential)
	}
}

func TestJSONUsageCommandsAndHelpAreStable(t *testing.T) {
	stateDir := t.TempDir()
	if got := runCLI(t, nil, "--state-dir", stateDir, "--json", "usage", "stats"); strings.TrimSpace(got) != "[]" {
		t.Fatalf("empty JSON stats = %q", got)
	}
	help := runCLI(t, nil, "help")
	for _, command := range []string{"serve", "credential", "key", "usage"} {
		if !strings.Contains(help, command) {
			t.Fatalf("root help missing %s: %q", command, help)
		}
	}
}

func TestUsageHistoryShowsProviderRequestIDOnlyInLocalCLIOutput(t *testing.T) {
	stateDir := t.TempDir()
	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal(err)
	}
	credential, err := store.CreateCredential(
		context.Background(), "History", "codex", "openai_api_key", "acct_history_cli", nil,
	)
	if err != nil {
		t.Fatal(err)
	}
	key, err := store.CreateAPIKey(context.Background(), credential.ID, "History")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.AuthenticateAndStart(context.Background(), key.Secret, "req_history_cli"); err != nil {
		t.Fatal(err)
	}
	providerRequestID := "provider-cli-visible"
	if err := store.FinalizeRequest(context.Background(), "req_history_cli", storage.RequestResult{
		CompletedAt: time.Now().UTC(), Status: storage.RequestCompleted,
		Duration: time.Second, ProviderRequestID: &providerRequestID,
	}); err != nil {
		t.Fatal(err)
	}
	if err := store.Close(); err != nil {
		t.Fatal(err)
	}

	plain := runCLI(t, nil,
		"--state-dir", stateDir, "usage", "history", "--key", key.ID,
	)
	if !strings.Contains(plain, "provider_request_id="+providerRequestID) {
		t.Fatalf("plain history = %q", plain)
	}
	jsonOutput := runCLI(t, nil,
		"--state-dir", stateDir, "--json", "usage", "history", "--key", key.ID,
	)
	var records []storage.RequestRecord
	if err := json.Unmarshal([]byte(jsonOutput), &records); err != nil {
		t.Fatal(err)
	}
	if len(records) != 1 || records[0].ProviderRequestID == nil ||
		*records[0].ProviderRequestID != providerRequestID {
		t.Fatalf("JSON history = %#v", records)
	}
}

func runCLI(t *testing.T, input io.Reader, arguments ...string) string {
	t.Helper()
	if input == nil {
		input = strings.NewReader("")
	}
	var output, errors bytes.Buffer
	err := Run(context.Background(), arguments, Environment{
		Stdin: input, Stdout: &output, Stderr: &errors,
	})
	if err != nil {
		t.Fatalf("Run(%q): %v\nstderr: %s", arguments, err, errors.String())
	}
	return output.String()
}

func runFakeCredentialCore() int {
	arguments := os.Args[1:]
	secret, _ := io.ReadAll(os.Stdin)
	trimmedSecret := strings.TrimSpace(string(secret))
	if trimmedSecret != "" {
		for _, value := range append(arguments, os.Environ()...) {
			if strings.Contains(value, trimmedSecret) {
				return 31
			}
		}
	}
	if len(arguments) < 2 || arguments[0] != "credential" {
		return 32
	}
	if expected := os.Getenv("MINI_SUB2API_EXPECT_FINGERPRINT_MODE"); expected != "" {
		actual, found := fakeArgumentValue(arguments, "--fingerprint-mode")
		if !found || actual != expected {
			return 35
		}
	}
	switch arguments[1] {
	case "login":
		fmt.Fprintln(os.Stdout, `{"accountRef":"acct_fake_login","authKind":"codex_oauth","upstreamAccountId":"chatgpt-fake-login","status":"ready"}`)
	case "add-api-key":
		if trimmedSecret == "" {
			return 33
		}
		fmt.Fprintln(os.Stdout, `{"accountRef":"acct_fake_cli","authKind":"openai_api_key","status":"ready"}`)
	case "import-codex-auth":
		fmt.Fprintln(os.Stdout, `{"accountRef":"acct_fake_import","authKind":"codex_oauth","upstreamAccountId":"chatgpt-fake-account","status":"ready"}`)
	case "fingerprint":
		if os.Getenv("MINI_SUB2API_FAKE_FINGERPRINT_FAIL") == "1" {
			return 36
		}
		mode := os.Getenv("MINI_SUB2API_FAKE_FINGERPRINT_MODE")
		if mode == "" {
			mode = "device"
		}
		revision := 1
		if mode == "off" {
			revision = 2
		}
		if os.Getenv("MINI_SUB2API_FAKE_FINGERPRINT_LEAK") == "1" {
			fmt.Fprintf(os.Stdout, `{"accountRef":"acct_fake_cli","mode":%q,"revision":%d,"installationId":"11111111-1111-4111-8111-111111111111"}`+"\n", mode, revision)
		} else {
			fmt.Fprintf(os.Stdout, `{"accountRef":"acct_fake_cli","mode":%q,"revision":%d}`+"\n", mode, revision)
		}
	case "remove":
		fmt.Fprintln(os.Stdout, `{"accountRef":"acct_fake_cli","removed":true}`)
	case "revoke":
		fmt.Fprintln(os.Stdout, `{"accountRef":"acct_fake_cli","revoked":true}`)
	default:
		return 34
	}
	return 0
}

func fakeArgumentValue(arguments []string, name string) (string, bool) {
	for index, argument := range arguments {
		if argument == name && index+1 < len(arguments) {
			return arguments[index+1], true
		}
		if strings.HasPrefix(argument, name+"=") {
			return strings.TrimPrefix(argument, name+"="), true
		}
	}
	return "", false
}
