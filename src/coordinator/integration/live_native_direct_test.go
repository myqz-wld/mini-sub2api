//go:build nativeparity && liveparity

package integration

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

type liveAccessCredentials struct {
	IDToken     string `json:"id_token"`
	AccessToken string `json:"access_token"`
	AccountID   string `json:"account_id"`
}

func readLiveAccessCredentials(t *testing.T) liveAccessCredentials {
	t.Helper()
	if os.Getenv("MINI_SUB2API_LIVE_SUBSCRIPTION") != "1" {
		t.Fatal("live Subscription requires explicit opt-in")
	}
	path := os.Getenv("MINI_SUB2API_LIVE_AUTH_FILE")
	if path == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			t.Fatal("locate existing Codex login")
		}
		path = filepath.Join(home, ".codex", "auth.json")
	}
	raw, err := os.ReadFile(path)
	var auth struct {
		Mode   string                `json:"auth_mode"`
		Tokens liveAccessCredentials `json:"tokens"`
	}
	if err != nil || len(raw) > 128*1024 || json.Unmarshal(raw, &auth) != nil || auth.Mode != "chatgpt" || auth.Tokens.IDToken == "" || auth.Tokens.AccountID == "" {
		t.Fatal("valid existing ChatGPT login required")
	}
	parts := strings.Split(auth.Tokens.AccessToken, ".")
	var claims struct {
		Exp int64 `json:"exp"`
	}
	if len(parts) != 3 {
		t.Fatal("live access token shape")
	}
	payload, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil || json.Unmarshal(payload, &claims) != nil || claims.Exp < time.Now().Add(5*time.Minute).Unix() {
		t.Fatal("live access token needs renewal outside the test")
	}
	digest := sha256.Sum256(raw)
	t.Cleanup(func() {
		current, err := os.ReadFile(path)
		if err != nil || sha256.Sum256(current) != digest {
			t.Error("original Codex login changed during live validation")
		}
	})
	return auth.Tokens // The refresh token is deliberately never copied.
}

func liveSyntheticDirectory(t *testing.T) string {
	t.Helper()
	// A neutral path avoids sending a personal home/worktree prefix as native workspace metadata.
	dir, err := os.MkdirTemp("/tmp", "mini-sub2api-live-")
	if err != nil {
		t.Fatal("create isolated live workspace")
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	return dir
}

// This separate launcher is the only native driver with direct provider networking. The normal
// launcher remains loopback-only and cannot acquire a real credential or fall back to a provider.
func startLiveNativeDirect(t *testing.T, options nativeOptions) *nativeClient {
	t.Helper()
	auth := readLiveAccessCredentials(t)
	binary := os.Getenv("MINI_SUB2API_NATIVE_CODEX_BINARY")
	if binary == "" {
		t.Fatal("live native baseline requires an explicit pinned CLI path")
	}
	version, err := exec.Command(binary, "--version").Output()
	if err != nil || strings.TrimSpace(string(version)) != nativeVersion {
		t.Fatal("live native binary version mismatch")
	}
	if options.model != "gpt-5.5" && options.model != "gpt-6-astra" {
		t.Fatal("live model outside bounded matrix")
	}
	isolated, home := liveSyntheticDirectory(t), liveSyntheticDirectory(t)
	config := fmt.Sprintf(`model = %q
model_catalog_json = %q
model_provider = "openai"
approval_policy = "never"
sandbox_mode = "read-only"
chatgpt_base_url = "https://chatgpt.com/backend-api"
web_search = "disabled"
check_for_update_on_startup = false
cli_auth_credentials_store = "file"
[analytics]
enabled = false
[features]
apps = false
plugins = false
recommended_plugins = false
code_mode = false
code_mode_host = false
`, options.model, filepath.Join(nativeSource(t), "codex-rs", "models-manager", "models.json"))
	if !options.ws {
		// 0.156.0 selects WS by provider capability. Its old feature toggles are removed.
		config = strings.Replace(config, `model_provider = "openai"`, `model_provider = "live_http"`, 1)
		config += `
[model_providers.live_http]
name = "OpenAI"
base_url = "https://chatgpt.com/backend-api/codex"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = false
http_headers = { version = "0.156.0" }
request_max_retries = 0
stream_max_retries = 0
stream_idle_timeout_ms = 90000
`
	}
	if os.WriteFile(filepath.Join(isolated, "config.toml"), []byte(config), 0600) != nil {
		t.Fatal("write isolated native config")
	}
	credentials := map[string]any{"auth_mode": "chatgpt", "last_refresh": time.Now().UTC().Format(time.RFC3339), "tokens": map[string]any{
		"id_token": auth.IDToken, "access_token": auth.AccessToken, "account_id": auth.AccountID, "refresh_token": "",
	}}
	if os.WriteFile(filepath.Join(isolated, "auth.json"), mustRequestJSON(t, credentials), 0600) != nil {
		t.Fatal("write isolated access-only native auth")
	}
	options.deadline = 3 * time.Minute
	ctx, cancel := context.WithTimeout(context.Background(), options.deadline)
	args := append([]string{"app-server", "--stdio"}, nativeConfigArguments(t, options.configOverrides)...)
	command := exec.CommandContext(ctx, binary, args...)
	command.Dir = options.project
	for _, key := range []string{"PATH", "TMPDIR", "SHELL"} {
		if value := os.Getenv(key); value != "" {
			command.Env = append(command.Env, key+"="+value)
		}
	}
	command.Env = append(command.Env, "HOME="+home, "CODEX_HOME="+isolated, "RUST_LOG=off", "TERM=dumb", "TERM_PROGRAM=native-parity", "TERM_PROGRAM_VERSION=1")
	return startNativeProcess(t, options, ctx, cancel, command)
}
