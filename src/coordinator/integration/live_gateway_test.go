//go:build nativeparity && liveparity

package integration

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/httpapi"
	"mini-sub2api/src/coordinator/internal/storage"
)

// This factory is deliberately separate from the loopback-only native gateway. Both a build tag
// and explicit runtime opt-in are required. The destination cannot be supplied through the request.
func newLiveSubscriptionGateway(t *testing.T) nativeGateway {
	return newLiveSubscriptionGatewayBounded(t, 8)
}

func newLiveSubscriptionGatewayBounded(t *testing.T, maxHTTP int32) nativeGateway {
	return newLiveSubscriptionGatewayAt(t, maxHTTP, "https://chatgpt.com/backend-api/codex/responses", false)
}

func newLiveSubscriptionGatewayAt(t *testing.T, maxHTTP int32, upstream string, httpOnly bool) nativeGateway {
	t.Helper()
	if upstream != "https://chatgpt.com/backend-api/codex/responses" {
		assertLoopbackURL(t, upstream)
	}
	if maxHTTP < 1 || maxHTTP > 64 {
		t.Fatal("live fixture HTTP bound must be between 1 and 64")
	}
	if os.Getenv("MINI_SUB2API_LIVE_SUBSCRIPTION") != "1" {
		t.Fatal("live Subscription requires explicit opt-in")
	}
	logs := "off"
	if os.Getenv("MINI_SUB2API_LIVE_DIAGNOSTICS") == "1" {
		logs = "off,mini_sub2api_core_codex::request_diagnostics=info,mini_sub2api_core_codex::server::http_forward=info,mini_sub2api_core_codex::response_sse_diagnostics=info"
	}
	t.Setenv("RUST_LOG", logs)
	authPath := os.Getenv("MINI_SUB2API_LIVE_AUTH_FILE")
	if authPath == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			t.Fatal("locate existing Codex login")
		}
		authPath = filepath.Join(home, ".codex/auth.json")
	}
	original, err := os.ReadFile(authPath)
	if err != nil {
		t.Fatal("existing Codex login unavailable")
	}
	digest := sha256.Sum256(original)
	var auth struct {
		AuthMode string                                  `json:"auth_mode"`
		Tokens   struct{ AccessToken, AccountID string } `json:"tokens"`
	}
	// Decode only to check the login kind; never print credential material or copy its refresh token.
	if json.Unmarshal(original, &auth) != nil || auth.AuthMode != "chatgpt" {
		t.Fatal("live test requires an existing ChatGPT login")
	}
	original = nil
	t.Cleanup(func() {
		current, err := os.ReadFile(authPath)
		if err != nil || sha256.Sum256(current) != digest {
			t.Error("original Codex login changed during live validation")
		}
	})
	binary := findCoreBinary(t)
	stateDir := t.TempDir()
	coreDir := filepath.Join(stateDir, "core-codex")
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, binary, "credential", "import-codex-auth", "--state-dir", coreDir, "--auth-file", authPath, "--upstream-url", upstream)
	command.Stderr = io.Discard
	output, err := command.Output()
	if err != nil {
		t.Fatal("isolated live credential import failed")
	}
	var metadata coreMetadata
	if json.Unmarshal(output, &metadata) != nil {
		t.Fatal("invalid live import metadata")
	}
	output = nil
	store, err := storage.Open(context.Background(), stateDir, time.Now)
	if err != nil {
		t.Fatal("open isolated live state")
	}
	t.Cleanup(func() { _ = store.Close() })
	credential := persistCredential(t, store, "Isolated live Subscription", metadata)
	key := createDownstreamKey(t, store, credential.ID, "Synthetic live caller")
	supervisor, err := adapter.Start(context.Background(), adapter.Config{Binary: binary, StateDir: coreDir})
	if err != nil {
		t.Fatal("start isolated live Core")
	}
	t.Cleanup(func() { _ = supervisor.Close() })
	handler := httpapi.NewHandler(store, supervisor, nil)
	var calls atomic.Int32
	guard := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/backend-api/codex/responses" {
			if httpOnly && r.Method == http.MethodGet {
				w.WriteHeader(http.StatusMethodNotAllowed)
				return
			}
			r.URL.Path = "/v1/responses"
		}
		if r.Method == http.MethodPost && calls.Add(1) > maxHTTP {
			w.WriteHeader(http.StatusTooManyRequests)
			return
		}
		handler.ServeHTTP(w, r)
	})
	server, tap := newNativeTappedServer(t, guard)
	t.Cleanup(handler.ShutdownWebSockets)
	return nativeGateway{server: server, tap: tap, secret: key.Secret, stateDir: coreDir}
}

func liveResponse(t *testing.T, client nativeOrdinaryClient, request map[string]any) map[string]any {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
	defer cancel()
	request["stream"] = client.stream
	var events [][]byte
	output := make(map[int]any)
	if client.ws != nil {
		request["type"] = "response.create"
		if client.ws.Write(ctx, 1, mustRequestJSON(t, request)) != nil {
			t.Fatal("live WS send failed")
		}
		for i := 0; i < 4096; i++ {
			_, raw, err := client.ws.Read(ctx)
			if err != nil {
				t.Fatal("live WS response read failed")
			}
			var event map[string]any
			if json.Unmarshal(raw, &event) != nil {
				t.Fatal("live WS JSON invalid")
			}
			if response := collectOrdinaryOutput(t, event, output); response != nil {
				return response
			}
			if event["type"] == "error" || event["type"] == "response.failed" || event["type"] == "response.incomplete" {
				t.Fatal("live WS upstream rejected or did not complete the request")
			}
		}
		t.Fatal("live WS event bound exceeded")
	} else {
		req, _ := http.NewRequestWithContext(ctx, http.MethodPost, client.gateway.server.URL+"/v1/responses", bytes.NewReader(mustRequestJSON(t, request)))
		req.Header = client.headers.Clone()
		req.Header.Set("Content-Type", "application/json")
		response, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal("live HTTP delivery failed")
		}
		defer response.Body.Close()
		raw, err := io.ReadAll(io.LimitReader(response.Body, 2*1024*1024+1))
		if err != nil || len(raw) > 2*1024*1024 {
			t.Fatal("live HTTP response bound or read failed")
		}
		if response.StatusCode != 200 {
			var failure struct {
				Error struct {
					Code    any    `json:"code"`
					Type    string `json:"type"`
					Message string `json:"message"`
				} `json:"error"`
			}
			_ = json.Unmarshal(raw, &failure)
			kind := "unclassified"
			for _, known := range []string{"invalid_request", "invalid_request_error", "state_unavailable", "upstream_error", "authentication_error", "invalid_api_key", "unsupported_value"} {
				if failure.Error.Type == known || failure.Error.Code == known {
					kind = known
				}
			}
			var mentions []string
			for _, word := range []string{"tools", "instructions", "model", "reasoning", "unsupported", "required", "token", "expired", "input"} {
				if strings.Contains(strings.ToLower(failure.Error.Message), word) {
					mentions = append(mentions, word)
				}
			}
			t.Logf("LIVE_DIAGNOSTIC status=%d kind=%s mentions=%s", response.StatusCode, kind, strings.Join(mentions, ","))
			t.Fatalf("live HTTP status=%d", response.StatusCode)
		}
		if !client.stream {
			var value map[string]any
			if json.Unmarshal(raw, &value) != nil {
				t.Fatal("live JSON response invalid")
			}
			return value
		}
		events = bytes.Split(raw, []byte("\n"))
	}
	for _, line := range events {
		if !bytes.HasPrefix(line, []byte("data: ")) {
			continue
		}
		var event map[string]any
		if json.Unmarshal(line[6:], &event) == nil {
			if response := collectOrdinaryOutput(t, event, output); response != nil {
				return response
			}
		}
	}
	t.Fatal("live SSE completion missing")
	return nil
}

func liveText(response map[string]any) string {
	var result bytes.Buffer
	items, _ := response["output"].([]any)
	for _, raw := range items {
		item, _ := raw.(map[string]any)
		content, _ := item["content"].([]any)
		for _, part := range content {
			entry, _ := part.(map[string]any)
			if entry["type"] == "output_text" {
				if text, ok := entry["text"].(string); ok {
					result.WriteString(text)
				}
			}
		}
	}
	return result.String()
}

func describeLiveResponse(t *testing.T, response map[string]any, expected string) {
	t.Helper()
	status := "unknown"
	for _, known := range []string{"completed", "failed", "incomplete", "in_progress"} {
		if response["status"] == known {
			status = known
		}
	}
	items, _ := response["output"].([]any)
	messages, textParts := 0, 0
	for _, raw := range items {
		item, _ := raw.(map[string]any)
		if item["type"] == "message" {
			messages++
		}
		parts, _ := item["content"].([]any)
		for _, raw := range parts {
			part, _ := raw.(map[string]any)
			if part["type"] == "output_text" {
				textParts++
			}
		}
	}
	text := liveText(response)
	errorKind := "none"
	if failure, ok := response["error"].(map[string]any); ok {
		errorKind = "unclassified"
		for _, known := range []string{"rate_limit_exceeded", "insufficient_quota", "invalid_request_error", "server_error", "model_not_found", "invalid_api_key"} {
			if failure["code"] == known || failure["type"] == known {
				errorKind = known
			}
		}
	}
	t.Logf("LIVE_SHAPE status=%s error=%s items=%d messages=%d text_parts=%d text_bytes=%d wrapped_match=%t contains_expected=%t nested_response=%t", status, errorKind, len(items), messages, textParts, len(text), strings.Trim(text, "`\" \r\n\t") == expected, strings.Contains(text, expected), response["response"] != nil)
}
