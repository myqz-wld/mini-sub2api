//go:build nativeparity && scaffoldparity

package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

type openCodeClient struct{ endpoint, model, project, provider string }

func startOpenCode(t *testing.T, endpoint, secret, model string) openCodeClient {
	return startOpenCodeWithRead(t, endpoint, secret, model, false)
}

func startOpenCodeWithRead(t *testing.T, endpoint, secret, model string, allowRead bool) openCodeClient {
	return startOpenCodeProvider(t, endpoint, secret, model, "capture", allowRead)
}

func startOpenCodeProvider(t *testing.T, endpoint, secret, model, provider string, allowRead bool) openCodeClient {
	t.Helper()
	assertLoopbackURL(t, endpoint)
	binary := os.Getenv("MINI_SUB2API_OPENCODE_BINARY")
	if binary == "" {
		binary = filepath.Join(nativeRepository(), "build/third-party/opencode/platform/package/bin/opencode")
	}
	root := t.TempDir()
	project := filepath.Join(root, "project")
	data := filepath.Join(root, "data")
	logDir := filepath.Join(data, "opencode/log")
	for _, p := range []string{project, logDir} {
		if os.MkdirAll(p, 0700) != nil {
			t.Fatal("create isolated OpenCode directories")
		}
	}
	if canonical, err := filepath.EvalSymlinks(project); err == nil {
		project = canonical
	}
	// Upstream OpenCode always installs a file logger, even with --print-logs. Redirect the
	// task-owned exact log file to the null device; SQLite itself is configured in memory.
	if os.Symlink(os.DevNull, filepath.Join(logDir, "opencode.log")) != nil {
		t.Fatal("disable OpenCode file log persistence")
	}
	config := map[string]any{
		"autoupdate": false, "share": "disabled", "snapshot": false,
		"enabled_providers": []string{provider}, "model": provider + "/" + model, "small_model": provider + "/" + model,
		"permission": map[string]string{"*": "deny"},
		"agent":      map[string]any{"probe": map[string]any{"mode": "primary", "prompt": "Return a short answer to the supplied synthetic task.", "tools": map[string]bool{"*": false}}},
		"provider": map[string]any{provider: map[string]any{
			"npm": "@ai-sdk/openai", "name": "Isolated Responses capture",
			"options": map[string]any{"baseURL": endpoint + "/v1", "apiKey": secret, "timeout": 60000},
			"models":  map[string]any{model: map[string]any{"name": model, "limit": map[string]int{"context": 128000, "output": 256}}},
		}},
	}
	if allowRead {
		// Give permission matching a fixed worktree and allow only the synthetic fixture.
		// Never expose arbitrary host files to a real model's tool arguments.
		init := exec.Command("git", "-c", "init.templateDir=", "-c", "core.hooksPath="+os.DevNull, "init", "--quiet", project)
		init.Env = []string{"PATH=" + os.Getenv("PATH"), "HOME=" + root, "GIT_CONFIG_NOSYSTEM=1"}
		if init.Run() != nil {
			t.Fatal("initialize isolated OpenCode fixture worktree")
		}
		agent := config["agent"].(map[string]any)["probe"].(map[string]any)
		delete(agent, "tools")
		agent["permission"] = map[string]any{"*": "deny", "read": map[string]string{"*": "deny", "fixture.txt": "allow"}}
		agent["steps"] = 3
		if os.WriteFile(filepath.Join(project, "fixture.txt"), []byte("SYNTHETIC_FILE_VALUE\n"), 0600) != nil {
			t.Fatal("write scoped OpenCode read fixture")
		}
	}
	configPath := filepath.Join(root, "opencode.json")
	if os.WriteFile(configPath, mustRequestJSON(t, config), 0600) != nil {
		t.Fatal("write isolated OpenCode provider configuration")
	}
	denied := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(http.StatusBadGateway) }))
	t.Cleanup(denied.Close)
	disablePlugins := "1"
	if provider == "openai" {
		// The explicit-header control needs the built-in Codex hook. It still uses the isolated
		// config/home, synthetic API key, disabled model fetch and loopback-only network boundary.
		disablePlugins = "0"
	}
	environment := []string{"PATH=" + os.Getenv("PATH"), "HOME=" + root, "OPENCODE_TEST_HOME=" + root,
		"XDG_DATA_HOME=" + data, "XDG_CACHE_HOME=" + filepath.Join(root, "cache"), "XDG_CONFIG_HOME=" + filepath.Join(root, "config"), "XDG_STATE_HOME=" + filepath.Join(root, "state"), "TMPDIR=" + root,
		"OPENCODE_CONFIG=" + configPath, "OPENCODE_DB=:memory:", "OPENCODE_LOG_LEVEL=ERROR",
		"OPENCODE_DISABLE_AUTOUPDATE=1", "OPENCODE_DISABLE_MODELS_FETCH=1", "OPENCODE_DISABLE_SHARE=1",
		"OPENCODE_DISABLE_DEFAULT_PLUGINS=" + disablePlugins, "OPENCODE_DISABLE_EXTERNAL_SKILLS=1", "OPENCODE_DISABLE_CLAUDE_CODE=1",
		"OPENCODE_DISABLE_LSP_DOWNLOAD=1", "OPENCODE_EXPERIMENTAL_DISABLE_FILEWATCHER=1", "OPENCODE_DISABLE_FFF=1",
		"NO_PROXY=127.0.0.1,::1", "no_proxy=127.0.0.1,::1", "TERM=dumb"}
	for _, name := range []string{"HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"} {
		environment = append(environment, name+"="+denied.URL)
	}
	version := exec.Command(binary, "--version")
	version.Env = environment
	version.Dir = project
	if output, err := version.Output(); err != nil || strings.TrimSpace(string(output)) != "1.18.29" {
		t.Fatal("OpenCode fixture requires exact version 1.18.29")
	}
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal("reserve OpenCode port")
	}
	port := listener.Addr().(*net.TCPAddr).Port
	_ = listener.Close()
	args := []string{"serve", "--hostname", "127.0.0.1", "--port", fmt.Sprint(port)}
	executable := binary
	if runtime.GOOS == "darwin" {
		executable = "/usr/bin/sandbox-exec"
		args = append([]string{"-p", `(version 1) (allow default) (deny network-outbound) (allow network-outbound (remote ip "localhost:*"))`, binary}, args...)
	}
	cmd := exec.Command(executable, args...)
	cmd.Env = environment
	cmd.Dir = project
	cmd.Stdout = io.Discard
	cmd.Stderr = io.Discard
	if cmd.Start() != nil {
		t.Fatal("start isolated OpenCode server")
	}
	done := make(chan struct{})
	go func() { _ = cmd.Wait(); close(done) }()
	t.Cleanup(func() {
		_ = cmd.Process.Kill()
		<-done
		_ = filepath.WalkDir(root, func(path string, entry os.DirEntry, err error) error {
			if err == nil && !entry.IsDir() && (strings.HasSuffix(path, ".db") || strings.HasSuffix(path, ".sqlite") || strings.HasSuffix(path, ".jsonl")) {
				t.Error("OpenCode fixture unexpectedly persisted session data")
			}
			return nil
		})
	})
	url := fmt.Sprintf("http://127.0.0.1:%d", port)
	client := http.Client{Timeout: time.Second}
	deadline := time.Now().Add(30 * time.Second)
	for time.Now().Before(deadline) {
		select {
		case <-done:
			t.Fatal("OpenCode exited before readiness")
		default:
		}
		response, err := client.Get(url + "/global/health")
		if err == nil {
			_, _ = io.Copy(io.Discard, response.Body)
			_ = response.Body.Close()
			if response.StatusCode == 200 {
				return openCodeClient{endpoint: url, model: model, project: project, provider: provider}
			}
		}
		time.Sleep(50 * time.Millisecond)
	}
	t.Fatal("OpenCode readiness timed out")
	return openCodeClient{}
}

func (c openCodeClient) call(t *testing.T, path string, value any) map[string]any {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
	defer cancel()
	req, _ := http.NewRequestWithContext(ctx, http.MethodPost, c.endpoint+path, bytes.NewReader(mustRequestJSON(t, value)))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("X-Opencode-Directory", c.project)
	response, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal("OpenCode request failed")
	}
	defer response.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(response.Body, 2*1024*1024+1))
	if err != nil || len(raw) > 2*1024*1024 || response.StatusCode >= 300 {
		t.Fatalf("OpenCode API status=%d", response.StatusCode)
	}
	var result map[string]any
	if json.Unmarshal(raw, &result) != nil {
		t.Fatal("invalid OpenCode API response")
	}
	return result
}

func (c openCodeClient) turn(t *testing.T, session, text string) map[string]any {
	return c.call(t, "/session/"+session+"/message", map[string]any{"agent": "probe", "model": map[string]string{"providerID": c.provider, "modelID": c.model}, "parts": []any{map[string]any{"type": "text", "text": text}}})
}
