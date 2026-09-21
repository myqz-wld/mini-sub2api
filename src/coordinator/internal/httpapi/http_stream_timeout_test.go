package httpapi

import (
	"context"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func shortHTTPTimeouts() httpStreamTimeouts {
	return httpStreamTimeouts{idle: 100 * time.Millisecond, terminalTail: 70 * time.Millisecond, write: 100 * time.Millisecond}
}

// Closing either end wakes the other; every test joins its producer.
func endlessHTTPBody(t *testing.T, prefix, repeated string) (io.ReadCloser, func()) {
	t.Helper()
	reader, writer := io.Pipe()
	done := make(chan struct{})
	go func() {
		defer close(done)
		defer writer.Close()
		if prefix != "" {
			if _, err := io.WriteString(writer, prefix); err != nil {
				return
			}
		}
		for {
			time.Sleep(5 * time.Millisecond)
			if _, err := io.WriteString(writer, repeated); err != nil {
				return
			}
		}
	}()
	abort := sync.OnceFunc(func() { _ = reader.Close() })
	t.Cleanup(func() { abort(); <-done })
	return reader, abort
}

func TestHTTPStreamTimeoutsDoNotDependOnUpstreamEOF(t *testing.T) {
	completed := "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"total_tokens\":7}}}\n\n"
	for _, test := range []struct {
		name, prefix, repeated string
		outcome                streamOutcome
		reason                 streamStopReason
	}{
		{"heartbeat", "", ": heartbeat\n\n", streamUpstreamError, streamStopIdle},
		{"partial", "", "data: {", streamUpstreamError, streamStopIdle},
		{"done_only", "", "data: [DONE]\n\n", streamUpstreamError, streamStopIdle},
		{"completed_open", completed, ": heartbeat\n\n", streamComplete, streamStopTail},
		{"completed_repeated", completed, completed, streamComplete, streamStopTail},
		{"failed_footer", "data: {\"type\":\"error\"}\n\n", "data: {\"type\":\"response.failed\",\"response\":{\"usage\":{\"total_tokens\":7}}}\n\n", streamResponseFailed, streamStopTail},
		{"late_error", completed, "data: {\"type\":\"error\"}\n\n", streamResponseFailed, streamStopTail},
		{"partial_tail", completed, "data: {", streamUpstreamError, streamStopPartialTail},
	} {
		t.Run(test.name, func(t *testing.T) {
			body, abort := endlessHTTPBody(t, test.prefix, test.repeated)
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			defer cancel()
			writer := httptest.NewRecorder()
			started := time.Now()
			_, outcome, reason := streamBody(writer, body, "text/event-stream", ctx, shortHTTPTimeouts(), abort)
			if outcome != test.outcome || reason != test.reason {
				t.Fatalf("stream outcome/reason = %v/%q, want %v/%q", outcome, reason, test.outcome, test.reason)
			}
			if elapsed := time.Since(started); elapsed < 50*time.Millisecond || elapsed > time.Second {
				t.Fatalf("stream lifetime = %v", elapsed)
			}
			wire := writer.Body.String()
			if !strings.HasPrefix(wire, test.prefix) || strings.TrimPrefix(wire, test.prefix) == "" {
				t.Fatal("passthrough prefix or following bytes were lost")
			}
		})
	}
}

func TestHTTPStreamProgressAndNaturalEOF(t *testing.T) {
	reader, writer := io.Pipe()
	done := make(chan struct{})
	defer reader.Close()
	const event = "data: {\"type\":\"response.metadata\"}\n\n"
	const terminal = "data: {\"type\":\"response.completed\"}\n\n"
	go func() {
		defer close(done)
		defer writer.Close()
		for i := 0; i < 6; i++ {
			if _, err := io.WriteString(writer, event); err != nil {
				return
			}
			time.Sleep(40 * time.Millisecond)
		}
		_, _ = io.WriteString(writer, terminal)
	}()
	var aborted atomic.Bool
	abort := func() { aborted.Store(true); _ = reader.Close() }
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	recorder := httptest.NewRecorder()
	_, outcome, reason := streamBody(recorder, reader, "text/event-stream", ctx, shortHTTPTimeouts(), abort)
	<-done
	if outcome != streamComplete || reason != streamStopNone || aborted.Load() {
		t.Fatalf("valid progressing stream aborted: %v/%q", outcome, reason)
	}
	if recorder.Body.String() != strings.Repeat(event, 6)+terminal {
		t.Fatal("stream bytes changed")
	}
}

func TestHTTPStreamClientCancellationClosesUpstream(t *testing.T) {
	body, abort := endlessHTTPBody(t, "", ": heartbeat\n\n")
	ctx, cancel := context.WithCancel(context.Background())
	timer := time.AfterFunc(20*time.Millisecond, cancel)
	defer timer.Stop()
	defer cancel()
	_, outcome, reason := streamBody(httptest.NewRecorder(), body, "text/event-stream", ctx, shortHTTPTimeouts(), abort)
	if outcome != streamClientDisconnected || reason != streamStopCanceled {
		t.Fatalf("cancellation outcome = %v/%q", outcome, reason)
	}
}

// net.Pipe provides real deadline-capable blocking writes without filling a host TCP buffer.
type stalledHTTPWriter struct {
	conn   net.Conn
	header http.Header
}

func (w *stalledHTTPWriter) Header() http.Header         { return w.header }
func (w *stalledHTTPWriter) WriteHeader(int)             {}
func (w *stalledHTTPWriter) Write(p []byte) (int, error) { return w.conn.Write(p) }
func (w *stalledHTTPWriter) SetWriteDeadline(deadline time.Time) error {
	return w.conn.SetWriteDeadline(deadline)
}

func TestHTTPStreamWriteDeadlineReleasesUpstream(t *testing.T) {
	server, client := net.Pipe()
	defer server.Close()
	defer client.Close()
	writer := &stalledHTTPWriter{conn: server, header: make(http.Header)}
	var aborted atomic.Bool
	timeouts := shortHTTPTimeouts()
	timeouts.idle = time.Second
	started := time.Now()
	_, outcome, reason := streamBody(writer, strings.NewReader("data: {}\n\n"), "text/event-stream", context.Background(), timeouts, func() { aborted.Store(true) })
	if outcome != streamClientDisconnected || reason != streamStopWriteTimeout || !aborted.Load() {
		t.Fatalf("blocked write outcome = %v/%q, closed = %t", outcome, reason, aborted.Load())
	}
	if elapsed := time.Since(started); elapsed > time.Second {
		t.Fatalf("write deadline took %v", elapsed)
	}
}
