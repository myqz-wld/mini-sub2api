//go:build nativeparity

package integration

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"
)

const nativeVersion = "codex-cli 0.156.0"

type nativeClient struct {
	t           *testing.T
	input       io.WriteCloser
	events      chan map[string]any
	id          int
	pending     []map[string]any
	readTimeout time.Duration
	observe     func(map[string]any)
}

type nativeOptions struct {
	endpoint               string
	model                  string
	ws                     bool
	bearer                 string
	base                   string
	project                string
	metadataEndpoint       string
	tools                  []any
	neutralPersonality     bool
	configOverrides        map[string]string
	homeFiles              map[string]string
	threadParams           map[string]any
	providerName           string
	traceEndpoint          string
	deadline               time.Duration
	observe                func(map[string]any)
	reasoningEffortUpdates bool
	builtinOpenAI          bool
}

func startNativeClient(t *testing.T, options nativeOptions) *nativeClient {
	t.Helper()
	assertLoopbackURL(t, options.endpoint)
	metadataEndpoint := options.metadataEndpoint
	if metadataEndpoint == "" {
		metadataEndpoint = options.endpoint
	}
	assertLoopbackURL(t, metadataEndpoint)
	providerName := options.providerName
	if providerName == "" {
		providerName = "OpenAI"
	}
	if providerName != "OpenAI" && providerName != "Native Local Capture" {
		t.Fatal("native fixture provider capability name is not allowlisted")
	}
	binary := os.Getenv("MINI_SUB2API_NATIVE_CODEX_BINARY")
	if binary == "" {
		var err error
		binary, err = exec.LookPath("codex")
		if err != nil {
			t.Fatal("native parity requires Codex v0.156.0")
		}
	}
	version, err := exec.Command(binary, "--version").Output()
	if err != nil || strings.TrimSpace(string(version)) != nativeVersion {
		t.Fatal("native parity binary version mismatch")
	}
	isolated := t.TempDir()
	catalogPath := filepath.Join(nativeSource(t), "codex-rs", "models-manager", "models.json")
	if options.reasoningEffortUpdates {
		data, err := os.ReadFile(catalogPath)
		var catalog map[string]any
		if err != nil || json.Unmarshal(data, &catalog) != nil {
			t.Fatal("read native capability fixture")
		}
		for _, raw := range catalog["models"].([]any) {
			entry := raw.(map[string]any)
			if entry["slug"] == options.model {
				entry["supports_reasoning_effort_updates"] = true
			}
		}
		catalogPath = filepath.Join(isolated, "models.json")
		if os.WriteFile(catalogPath, mustRequestJSON(t, catalog), 0600) != nil {
			t.Fatal("write native capability fixture")
		}
	}
	deadline := options.deadline
	if deadline == 0 {
		deadline = 30 * time.Second
	}
	if deadline < 30*time.Second || deadline > 3*time.Minute {
		t.Fatal("native fixture deadline outside bounded range")
	}
	idleMillis := 5000
	if deadline > 30*time.Second {
		idleMillis = 60000
	}
	isolatedUser := t.TempDir()
	project := options.project
	if project == "" {
		project = t.TempDir()
	}
	// This proxy never forwards requests. All supported test routes use literal loopback hosts.
	denied := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(http.StatusBadGateway) }))
	t.Cleanup(denied.Close)
	config := fmt.Sprintf(`model = %q
model_catalog_json = %q
model_provider = "native_capture"
approval_policy = "never"
sandbox_mode = "read-only"
chatgpt_base_url = %q
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
[model_providers.native_capture]
name = %q
base_url = %q
wire_api = "responses"
requires_openai_auth = true
supports_websockets = %t
request_max_retries = 0
stream_max_retries = 0
stream_idle_timeout_ms = %d
websocket_connect_timeout_ms = 3000
`, options.model, catalogPath, metadataEndpoint+"/backend-api", providerName, options.endpoint+"/v1", options.ws, idleMillis)
	if options.builtinOpenAI {
		config = strings.Replace(config, `model_provider = "native_capture"`, `model_provider = "openai"`, 1)
		config = fmt.Sprintf("openai_base_url = %q\n", options.endpoint+"/backend-api/codex") + config
	}
	if err := os.WriteFile(filepath.Join(isolated, "config.toml"), []byte(config), 0600); err != nil {
		t.Fatal("write native config")
	}
	writeNativeHomeFixtures(t, isolated, options.homeFiles)
	account := "native-loopback-account"
	bearer := options.bearer
	if bearer == "" {
		bearer = testJWT(nil, 3600)
	}
	auth := map[string]any{"auth_mode": "chatgpt", "last_refresh": time.Now().UTC().Format(time.RFC3339), "tokens": map[string]string{"id_token": testJWT(&account, 3600), "access_token": bearer, "refresh_token": "synthetic-unused", "account_id": account}}
	encoded, _ := json.Marshal(auth)
	if err := os.WriteFile(filepath.Join(isolated, "auth.json"), encoded, 0600); err != nil {
		t.Fatal("write synthetic native auth")
	}
	ctx, cancel := context.WithTimeout(context.Background(), deadline)
	args := []string{"app-server", "--stdio"}
	args = append(args, nativeConfigArguments(t, options.configOverrides)...)
	if options.traceEndpoint != "" {
		assertLoopbackURL(t, options.traceEndpoint)
		args = append(args,
			"-c", fmt.Sprintf(`otel.trace_exporter={otlp-http={endpoint=%q,protocol="json"}}`, options.traceEndpoint),
			"-c", `otel.exporter="none"`, "-c", `otel.metrics_exporter="none"`, "-c", "otel.log_user_prompt=false")
	}
	executable := binary
	if runtime.GOOS == "darwin" {
		executable = "/usr/bin/sandbox-exec"
		args = append([]string{"-p", `(version 1) (allow default) (deny network-outbound) (allow network-outbound (remote ip "localhost:*"))`, binary}, args...)
	}
	command := exec.CommandContext(ctx, executable, args...)
	command.Dir = project
	// Override documented configuration only in this child; never change the parent environment.
	for _, key := range []string{"PATH", "TMPDIR", "USER", "LOGNAME", "SHELL"} {
		if value := os.Getenv(key); value != "" {
			command.Env = append(command.Env, key+"="+value)
		}
	}
	command.Env = append(command.Env, "HOME="+isolatedUser, "CODEX_HOME="+isolated, "RUST_LOG=off", "TERM=dumb", "TERM_PROGRAM=native-parity", "TERM_PROGRAM_VERSION=1", "NO_PROXY=127.0.0.1,::1", "no_proxy=127.0.0.1,::1")
	for _, key := range []string{"HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"} {
		command.Env = append(command.Env, key+"="+denied.URL)
	}
	return startNativeProcess(t, options, ctx, cancel, command)
}

func startNativeProcess(t *testing.T, options nativeOptions, ctx context.Context, cancel context.CancelFunc, command *exec.Cmd) *nativeClient {
	t.Helper()
	input, err := command.StdinPipe()
	if err != nil {
		cancel()
		t.Fatal("native stdin")
	}
	output, err := command.StdoutPipe()
	if err != nil {
		cancel()
		t.Fatal("native stdout")
	}
	command.Stderr = io.Discard // No prompts, auth, response text or native diagnostic bodies enter logs.
	if err := command.Start(); err != nil {
		cancel()
		t.Fatal("start isolated native test process")
	}
	readTimeout := 15 * time.Second
	if options.deadline > 30*time.Second {
		readTimeout = 90 * time.Second
	}
	client := &nativeClient{t: t, input: input, events: make(chan map[string]any, 128), readTimeout: readTimeout, observe: options.observe}
	var reader sync.WaitGroup
	reader.Add(1)
	go func() {
		defer reader.Done()
		defer close(client.events)
		scanner := bufio.NewScanner(output)
		scanner.Buffer(make([]byte, 4096), 16*1024*1024)
		for scanner.Scan() {
			var event map[string]any
			if json.Unmarshal(scanner.Bytes(), &event) != nil {
				continue
			}
			select {
			case client.events <- event:
			case <-ctx.Done():
				return
			}
		}
	}()
	t.Cleanup(func() { _ = input.Close(); cancel(); _ = command.Wait(); reader.Wait() })
	client.call("initialize", map[string]any{"clientInfo": map[string]any{"name": "codex-tui", "version": "0.156.0"}, "capabilities": map[string]any{"experimentalApi": true}})
	client.send(map[string]any{"method": "initialized"})
	return client
}

func (c *nativeClient) send(value map[string]any) {
	c.t.Helper()
	data, err := json.Marshal(value)
	if err != nil {
		c.t.Fatal("encode native control")
	}
	data = append(data, '\n')
	if _, err = c.input.Write(data); err != nil {
		c.t.Fatal("write native control")
	}
}
func (c *nativeClient) read() map[string]any {
	c.t.Helper()
	select {
	case event, ok := <-c.events:
		if !ok {
			c.t.Fatal("native test process exited before expected control event")
		}
		return event
	case <-time.After(c.readTimeout):
		c.t.Fatal("native control event deadline")
	}
	return nil
}
func (c *nativeClient) handle(event map[string]any) {
	if c.observe != nil {
		c.observe(event)
	}
	if event["method"] == "item/tool/call" {
		c.send(map[string]any{"id": event["id"], "result": map[string]any{"success": true, "contentItems": []any{map[string]any{"type": "inputText", "text": "synthetic tool result"}}}})
		return
	}
	if event["method"] == "turn/completed" {
		c.pending = append(c.pending, event)
	}
}
func (c *nativeClient) call(method string, params map[string]any) map[string]any {
	return c.callExpect(method, params, 0)
}
func (c *nativeClient) callExpect(method string, params map[string]any, errorCode int) map[string]any {
	c.t.Helper()
	c.id++
	id := c.id
	c.send(map[string]any{"id": id, "method": method, "params": params})
	for {
		event := c.read()
		if event["method"] == nil && event["id"] == float64(id) {
			if failure, ok := event["error"].(map[string]any); ok {
				if errorCode != 0 && failure["code"] == float64(errorCode) {
					return nil
				}
				c.t.Fatalf("native %s rejected (code %v)", method, failure["code"])
			}
			if errorCode != 0 {
				c.t.Fatal("native request unexpectedly accepted")
			}
			result, _ := event["result"].(map[string]any)
			return result
		}
		c.handle(event)
	}
}
func (c *nativeClient) thread(options nativeOptions) string {
	c.t.Helper()
	params := map[string]any{"model": options.model, "ephemeral": true, "approvalPolicy": "never", "sandbox": "read-only", "environments": []any{}, "dynamicTools": []any{map[string]any{"type": "function", "name": "native_probe", "description": "Synthetic test tool", "inputSchema": map[string]any{"type": "object", "properties": map[string]any{}}}}}
	if options.tools != nil {
		params["dynamicTools"] = options.tools
	}
	if options.project != "" {
		params["cwd"] = options.project
	}
	if options.base != "" {
		params["baseInstructions"] = options.base
	}
	for key, value := range options.threadParams {
		params[key] = value
	}
	validateNativeThreadParams(c.t, params)
	result := c.call("thread/start", params)
	thread, ok := result["thread"].(map[string]any)
	if !ok {
		c.t.Fatal("native thread response missing")
	}
	id, ok := thread["id"].(string)
	if !ok || id == "" {
		c.t.Fatal("native thread id missing")
	}
	if thread["ephemeral"] != true {
		c.t.Fatal("native capture must use an ephemeral thread")
	}
	return id
}
func (c *nativeClient) turn(thread, text string) {
	c.turnWith(thread, map[string]any{"input": []any{map[string]any{"type": "text", "text": text}}})
}

func (c *nativeClient) turnWith(thread string, params map[string]any) {
	c.t.Helper()
	request := make(map[string]any, len(params)+1)
	for key, value := range params {
		request[key] = value
	}
	request["threadId"] = thread
	c.call("turn/start", request)
	for {
		if len(c.pending) > 0 {
			event := c.pending[0]
			c.pending = c.pending[1:]
			params, _ := event["params"].(map[string]any)
			turn, _ := params["turn"].(map[string]any)
			if turn["status"] != "completed" {
				c.t.Fatalf("native turn status = %v", turn["status"])
			}
			return
		}
		c.handle(c.read())
	}
}
