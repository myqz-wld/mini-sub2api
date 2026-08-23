package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"mini-sub2api/src/coordinator/internal/storage"
)

func TestCredentialCreationForwardsDefaultAndExplicitFingerprintModes(t *testing.T) {
	t.Setenv("MINI_SUB2API_FAKE_CLI_CORE", "1")
	for _, variant := range []struct {
		name     string
		expected string
		explicit bool
	}{
		{name: "default", expected: fingerprintModeDevice},
		{name: "explicit-device", expected: fingerprintModeDevice, explicit: true},
		{name: "explicit-off", expected: fingerprintModeOff, explicit: true},
	} {
		variant := variant
		for _, creation := range []struct {
			name      string
			input     string
			arguments func(stateDir string) []string
		}{
			{
				name: "login",
				arguments: func(stateDir string) []string {
					return []string{"--state-dir", stateDir, "--core-binary", os.Args[0],
						"credential", "login", "codex", "--name", "OAuth"}
				},
			},
			{
				name: "import",
				arguments: func(stateDir string) []string {
					return []string{"--state-dir", stateDir, "--core-binary", os.Args[0],
						"credential", "import-codex", "--name", "Import",
						"--auth-file", filepath.Join(stateDir, "auth.json")}
				},
			},
			{
				name:  "api-key",
				input: "upstream-secret\n",
				arguments: func(stateDir string) []string {
					return []string{"--state-dir", stateDir, "--core-binary", os.Args[0],
						"credential", "add-api-key", "codex", "--name", "API",
						"--secret-stdin"}
				},
			},
		} {
			creation := creation
			t.Run(creation.name+"-"+variant.name, func(t *testing.T) {
				t.Setenv("MINI_SUB2API_EXPECT_FINGERPRINT_MODE", variant.expected)
				arguments := creation.arguments(t.TempDir())
				if variant.explicit {
					arguments = append(arguments, "--fingerprint-mode", variant.expected)
				}
				runCLI(t, strings.NewReader(creation.input), arguments...)
			})
		}
	}
}

func TestCredentialCreationRejectsUnsupportedFingerprintModeBeforeCore(t *testing.T) {
	stateDir := t.TempDir()
	_, _, err := runCLIResult(
		strings.NewReader("secret\n"),
		"--state-dir", stateDir, "--core-binary", filepath.Join(stateDir, "missing-core"),
		"credential", "add-api-key", "codex", "--name", "API", "--secret-stdin",
		"--fingerprint-mode", "session",
	)
	if err == nil || !strings.Contains(err.Error(), "--fingerprint-mode must be off or device") {
		t.Fatalf("invalid mode error = %v", err)
	}
}

func TestCredentialFingerprintInspectAndMutateDoNotExposeInstallationID(t *testing.T) {
	t.Setenv("MINI_SUB2API_FAKE_CLI_CORE", "1")
	stateDir := t.TempDir()
	credential := createFakeAPICredential(t, stateDir)

	t.Setenv("MINI_SUB2API_FAKE_FINGERPRINT_MODE", fingerprintModeDevice)
	inspection := runCLI(t, nil,
		"--state-dir", stateDir, "--core-binary", os.Args[0], "--json",
		"credential", "fingerprint", credential.ID,
	)
	assertSafeFingerprintOutput(t, inspection, credential.ID, fingerprintModeDevice, 1, "")

	_, _, err := runCLIResult(nil,
		"--state-dir", stateDir, "--core-binary", os.Args[0],
		"credential", "fingerprint", credential.ID, "--mode", fingerprintModeOff,
	)
	if err == nil || !strings.Contains(err.Error(), "must be disabled") {
		t.Fatalf("enabled mutation error = %v", err)
	}
	runCLI(t, nil, "--state-dir", stateDir, "credential", "disable", credential.ID)
	t.Setenv("MINI_SUB2API_FAKE_FINGERPRINT_MODE", fingerprintModeOff)
	mutation := runCLI(t, nil,
		"--state-dir", stateDir, "--core-binary", os.Args[0], "--json",
		"credential", "fingerprint", credential.ID, "--mode", fingerprintModeOff,
	)
	assertSafeFingerprintOutput(
		t, mutation, credential.ID, fingerprintModeOff, 2, storage.CredentialDisabled,
	)
	store, err := storage.Open(context.Background(), stateDir, nil)
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

func TestCredentialFingerprintChildFailureLeavesCredentialDisabled(t *testing.T) {
	t.Setenv("MINI_SUB2API_FAKE_CLI_CORE", "1")
	stateDir := t.TempDir()
	credential := createFakeAPICredential(t, stateDir)
	runCLI(t, nil, "--state-dir", stateDir, "credential", "disable", credential.ID)
	t.Setenv("MINI_SUB2API_FAKE_FINGERPRINT_FAIL", "1")
	_, output, err := runCLIResult(nil,
		"--state-dir", stateDir, "--core-binary", os.Args[0], "--json",
		"credential", "fingerprint", credential.ID, "--mode=off",
	)
	if err == nil || output != "" {
		t.Fatalf("child failure = %v, output = %q", err, output)
	}
	store, openErr := storage.Open(context.Background(), stateDir, nil)
	if openErr != nil {
		t.Fatal(openErr)
	}
	defer store.Close()
	stored, loadErr := store.Credential(context.Background(), credential.ID)
	if loadErr != nil {
		t.Fatal(loadErr)
	}
	if stored.Status != storage.CredentialDisabled {
		t.Fatalf("credential status = %q", stored.Status)
	}
}

func TestCredentialFingerprintRejectsUnexpectedCoreFieldsWithoutEchoingThem(t *testing.T) {
	t.Setenv("MINI_SUB2API_FAKE_CLI_CORE", "1")
	stateDir := t.TempDir()
	credential := createFakeAPICredential(t, stateDir)
	t.Setenv("MINI_SUB2API_FAKE_FINGERPRINT_LEAK", "1")
	_, output, err := runCLIResult(nil,
		"--state-dir", stateDir, "--core-binary", os.Args[0], "--json",
		"credential", "fingerprint", credential.ID,
	)
	if err == nil || output != "" {
		t.Fatalf("unexpected-field result = %v, output = %q", err, output)
	}
	if strings.Contains(err.Error(), "11111111") {
		t.Fatalf("identifier appeared in error: %v", err)
	}
}

func TestCredentialFingerprintHelpAndModeErrorsAreEnglish(t *testing.T) {
	help := runCLI(t, nil, "credential", "fingerprint", "--help")
	if !strings.Contains(help, "credential fingerprint ID [--mode off|device]") {
		t.Fatalf("fingerprint help = %q", help)
	}
	_, _, err := runCLIResult(nil, "credential", "fingerprint", "cred_test", "--mode", "full")
	if err == nil || err.Error() != "--mode must be off or device" {
		t.Fatalf("mode error = %v", err)
	}
}

func createFakeAPICredential(t *testing.T, stateDir string) storage.Credential {
	t.Helper()
	output := runCLI(t, strings.NewReader("upstream-secret\n"),
		"--state-dir", stateDir, "--core-binary", os.Args[0], "--json",
		"credential", "add-api-key", "codex", "--name", "API", "--secret-stdin",
	)
	var credential storage.Credential
	if err := json.Unmarshal([]byte(output), &credential); err != nil {
		t.Fatal(err)
	}
	return credential
}

func assertSafeFingerprintOutput(
	t *testing.T,
	output, credentialID, mode string,
	revision uint64,
	status string,
) {
	t.Helper()
	var result credentialFingerprintResult
	if err := json.Unmarshal([]byte(output), &result); err != nil {
		t.Fatal(err)
	}
	if result.ID != credentialID || result.Mode != mode || result.Revision != revision ||
		result.Status != status {
		t.Fatalf("fingerprint result = %#v", result)
	}
	uuid := regexp.MustCompile(`(?i)[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}`)
	if uuid.MatchString(output) || strings.Contains(output, "installation") ||
		strings.Contains(output, "acct_fake_cli") {
		t.Fatalf("private core identity appeared in output: %q", output)
	}
}

func runCLIResult(input io.Reader, arguments ...string) (string, string, error) {
	if input == nil {
		input = strings.NewReader("")
	}
	var output, errors bytes.Buffer
	err := Run(context.Background(), arguments, Environment{
		Stdin: input, Stdout: &output, Stderr: &errors,
	})
	return errors.String(), output.String(), err
}
