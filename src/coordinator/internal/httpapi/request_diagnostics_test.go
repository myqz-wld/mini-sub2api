package httpapi

import (
	"bytes"
	"context"
	"io"
	"log"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"syscall"
	"testing"
	"testing/synctest"
	"time"
)

func TestRequestLogsCorrelateAttemptsAndClassifyFailureWithoutPayloads(t *testing.T) {
	store, _, key := setupHTTPTest(t)
	core := &fakeCore{response: func(context.Context, string) (*http.Response, error) {
		return nil, &url.Error{Op: "POST", URL: "https://synthetic-private.example/token", Err: &net.OpError{Op: "read", Err: syscall.ECONNRESET}}
	}}
	var logs bytes.Buffer
	handler := NewHandler(store, core, log.New(&logs, "", 0))
	var ids []string
	for range 2 {
		request := httptest.NewRequest(http.MethodPost, "/v1/responses", strings.NewReader(`{"model":"synthetic-private","input":"synthetic-private"}`))
		request.Header.Set("Authorization", "Bearer "+key.Secret)
		writer := httptest.NewRecorder()
		handler.ServeHTTP(writer, request)
		if writer.Code != http.StatusBadGateway {
			t.Fatal(writer.Code)
		}
		ids = append(ids, writer.Header().Get("X-Mini-Sub2Api-Request-Id"))
	}
	if ids[0] == ids[1] || ids[0] == "" {
		t.Fatal("attempt identifiers not distinct")
	}
	text := logs.String()
	for _, id := range ids {
		if !strings.Contains(text, "event=request_finished request_id="+id) {
			t.Fatal("missing completion")
		}
	}
	var tags []string
	for field := range strings.FieldsSeq(text) {
		if strings.HasPrefix(field, "body_tag=") {
			tags = append(tags, field)
		}
	}
	if len(tags) != 2 || tags[0] != tags[1] || strings.Contains(tags[0], "unavailable") {
		t.Fatal("same body correlation lost")
	}
	for _, field := range []string{"phase=core_forward", "io_kind=connection_reset", "client_canceled=false", "history_saved=true"} {
		if !strings.Contains(text, field) {
			t.Fatalf("missing %s", field)
		}
	}
	if strings.Contains(text, "synthetic-private") || strings.Contains(text, key.Secret) || strings.Contains(text, key.ID) {
		t.Fatal("logs disclose request data, endpoint or downstream identity")
	}
	if len(strings.Split(strings.TrimSpace(text), "\n")) != 8 {
		t.Fatal("unexpected per-request log growth")
	}
}

type diagnosticPacedBody struct{ remaining int }

func (b *diagnosticPacedBody) Read(buffer []byte) (int, error) {
	if b.remaining == 0 {
		return 0, io.EOF
	}
	time.Sleep(20 * time.Second)
	b.remaining--
	frame := "data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic-private\"}\n\n"
	if b.remaining == 0 {
		frame = "data: {\"type\":\"response.completed\"}\n\n"
	}
	return copy(buffer, frame), nil
}

func TestProgressReportsAreMinuteBoundedAndByteTransparent(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		started := time.Now()
		var reports []time.Duration
		writer := httptest.NewRecorder()
		_, outcome, reason, details := streamBodyObserved(writer, &diagnosticPacedBody{remaining: 10},
			"text/event-stream", context.Background(), defaultHTTPStreamTimeouts(), func() {},
			func(snapshot httpStreamDiagnostics) {
				reports = append(reports, time.Since(started))
				if strings.Contains(snapshot.String(), "synthetic-private") {
					t.Fatal("payload logged")
				}
			})
		if outcome != streamComplete || reason != streamStopNone {
			t.Fatalf("outcome %v/%s", outcome, reason)
		}
		if len(reports) != 3 {
			t.Fatalf("progress reports = %v", reports)
		}
		for index, elapsed := range reports {
			if elapsed != time.Duration(index+1)*time.Minute {
				t.Fatal(reports)
			}
		}
		if details.progress.Outputs != 9 || details.bytesWritten != uint64(writer.Body.Len()) || strings.Count(writer.Body.String(), "synthetic-private") != 9 {
			t.Fatal("diagnostics changed forwarding or counts")
		}
	})
}

type delayedFailureWriter struct{ *httptest.ResponseRecorder }

func (w delayedFailureWriter) Write([]byte) (int, error) {
	time.Sleep(3 * time.Second)
	return 2, &net.OpError{Op: "write", Err: syscall.EPIPE}
}

func TestFailedWriteRetainsBlockedTimeAndTypedCause(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		_, outcome, _, details := streamBody(delayedFailureWriter{httptest.NewRecorder()}, strings.NewReader("synthetic-private"),
			"application/json", context.Background(), defaultHTTPStreamTimeouts(), func() {})
		if outcome != streamClientDisconnected || details.failurePhase != "downstream_write" || details.failure.IO != "broken_pipe" || details.writeTime != 3*time.Second || details.bytesWritten != 2 {
			t.Fatalf("incorrect write observation: %s", details)
		}
	})
}

func TestMissingTerminalIsDistinctFromTruncatedBody(t *testing.T) {
	for _, truncated := range []bool{false, true} {
		var body io.Reader = strings.NewReader("data: {\"type\":\"response.in_progress\"}\n\n")
		if truncated {
			body = io.MultiReader(body, diagnosticErrorReader{})
		}
		_, outcome, reason, details := streamBody(httptest.NewRecorder(), body, "text/event-stream",
			context.Background(), defaultHTTPStreamTimeouts(), func() {})
		if outcome != streamUpstreamError {
			t.Fatal("missing terminal accepted")
		}
		if truncated {
			if details.failurePhase != "core_body" || details.failure.Kind != "incomplete_body" {
				t.Fatal(details)
			}
		} else if reason != streamStopMissingTerminal || details.failure.Kind != "none" {
			t.Fatal(details)
		}
	}
}

type diagnosticErrorReader struct{}

func (diagnosticErrorReader) Read([]byte) (int, error) { return 0, io.ErrUnexpectedEOF }
