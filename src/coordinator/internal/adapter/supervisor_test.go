package adapter

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"strings"
	"testing"
	"time"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestMain(m *testing.M) {
	if os.Getenv("MINI_SUB2API_FAKE_CORE") == "1" {
		os.Exit(runFakeCore())
	}
	os.Exit(m.Run())
}

func TestSupervisorStartsAndForwardsWithoutSecretInProcessMetadata(t *testing.T) {
	t.Setenv("MINI_SUB2API_FAKE_CORE", "1")
	supervisor, err := Start(context.Background(), Config{
		Binary: os.Args[0], StateDir: t.TempDir(),
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = supervisor.Close() })
	readiness, ok := supervisor.Readiness()
	if !ok || readiness.ProtocolVersion != protocolv1.Version {
		t.Fatalf("readiness = %#v, %v", readiness, ok)
	}
	headers := http.Header{
		"Authorization":      []string{"Bearer downstream-secret"},
		"Content-Type":       []string{"application/json"},
		"X-Codex-Turn-State": []string{"turn-test"},
		"X-Forwarded-For":    []string{"203.0.113.1"},
	}
	response, err := supervisor.Forward(
		context.Background(), "acct_test", "req_test", headers, []byte(`{"stream":true}`),
	)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	body, err := io.ReadAll(response.Body)
	if err != nil {
		t.Fatal(err)
	}
	if string(body) != "firstsecond" {
		t.Fatalf("body = %q", body)
	}
	if response.Header.Get(protocolv1.CoreTTFBHeader) != "3" {
		t.Fatalf("TTFB header = %q", response.Header.Get(protocolv1.CoreTTFBHeader))
	}
}

func TestSupervisorRestartsAfterCoreExit(t *testing.T) {
	t.Setenv("MINI_SUB2API_FAKE_CORE", "1")
	t.Setenv("MINI_SUB2API_FAKE_EXIT_ONCE_FILE", t.TempDir()+"/exited")
	supervisor, err := Start(context.Background(), Config{
		Binary: os.Args[0], StateDir: t.TempDir(),
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = supervisor.Close() })
	deadline := time.Now().Add(5 * time.Second)
	for {
		response, err := supervisor.Forward(
			context.Background(), "acct_restart", "req_restart", nil, []byte(`{}`),
		)
		if err == nil {
			response.Body.Close()
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("core did not restart: %v", err)
		}
		time.Sleep(25 * time.Millisecond)
	}
}

func TestReadinessRejectsWrongProtocolOrPID(t *testing.T) {
	input := bytes.NewBufferString(`{"protocolVersion":"2","port":1234,"pid":7,"build":{"name":"x","version":"1","commit":"c"}}` + "\n")
	result := readReadiness(bufio.NewReader(input), 7)
	if result.err == nil {
		t.Fatal("wrong protocol accepted")
	}
}

func TestReadinessRejectsOversizedLineWithoutGrowingBuffer(t *testing.T) {
	input := bytes.NewBuffer(append(bytes.Repeat([]byte("x"), 5000), '\n'))
	result := readReadiness(bufio.NewReaderSize(input, 4097), 7)
	if result.err == nil || !strings.Contains(result.err.Error(), "exceeds 4096") {
		t.Fatalf("oversized readiness error = %v", result.err)
	}
}

func runFakeCore() int {
	token, err := bufio.NewReader(os.Stdin).ReadString('\n')
	if err != nil {
		return 20
	}
	token = strings.TrimSpace(token)
	if len(token) < 32 {
		return 21
	}
	for _, value := range append(os.Args, os.Environ()...) {
		if strings.Contains(value, token) {
			return 22
		}
	}
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return 23
	}
	address := listener.Addr().(*net.TCPAddr)
	if !address.IP.IsLoopback() {
		return 24
	}
	readiness := protocolv1.Readiness{
		ProtocolVersion: protocolv1.Version,
		Port:            uint16(address.Port),
		PID:             os.Getpid(),
		Build: protocolv1.BuildIdentity{
			Name: "fake-core", Version: "0.1.0", Commit: "test",
		},
	}
	if err := json.NewEncoder(os.Stdout).Encode(readiness); err != nil {
		return 25
	}
	if sentinel := os.Getenv("MINI_SUB2API_FAKE_EXIT_ONCE_FILE"); sentinel != "" {
		if _, err := os.Stat(sentinel); os.IsNotExist(err) {
			if err := os.WriteFile(sentinel, []byte("exited"), 0o600); err != nil {
				return 26
			}
			listener.Close()
			return 0
		}
	}
	handler := http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		expectedAuth := "Bearer " + token
		if request.URL.Path != "/internal/v1/responses" ||
			request.Header.Get("Authorization") != expectedAuth ||
			request.Header.Get(protocolv1.VersionHeader) != protocolv1.Version ||
			request.Header.Get("X-Forwarded-For") != "" {
			http.Error(writer, "invalid internal request", http.StatusUnauthorized)
			return
		}
		body, err := io.ReadAll(request.Body)
		if err != nil || len(body) == 0 {
			http.Error(writer, "invalid body", http.StatusBadRequest)
			return
		}
		writer.Header().Set(protocolv1.CoreTTFBHeader, "3")
		writer.WriteHeader(http.StatusOK)
		_, _ = io.WriteString(writer, "first")
		if flusher, ok := writer.(http.Flusher); ok {
			flusher.Flush()
		}
		time.Sleep(10 * time.Millisecond)
		_, _ = io.WriteString(writer, "second")
	})
	server := &http.Server{Handler: handler, ReadHeaderTimeout: 2 * time.Second}
	if err := server.Serve(listener); err != nil && err != http.ErrServerClosed {
		fmt.Fprintln(os.Stderr, "fake core serve failed")
		return 27
	}
	return 0
}
