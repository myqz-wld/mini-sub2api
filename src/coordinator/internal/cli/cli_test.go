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
	switch arguments[1] {
	case "add-api-key":
		if trimmedSecret == "" {
			return 33
		}
		fmt.Fprintln(os.Stdout, `{"accountRef":"acct_fake_cli","authKind":"openai_api_key","status":"ready"}`)
	case "import-codex-auth":
		fmt.Fprintln(os.Stdout, `{"accountRef":"acct_fake_import","authKind":"codex_oauth","upstreamAccountId":"chatgpt-fake-account","status":"ready"}`)
	case "remove":
		fmt.Fprintln(os.Stdout, `{"accountRef":"acct_fake_cli","removed":true}`)
	case "revoke":
		fmt.Fprintln(os.Stdout, `{"accountRef":"acct_fake_cli","revoked":true}`)
	default:
		return 34
	}
	return 0
}
