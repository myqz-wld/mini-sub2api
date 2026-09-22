package diagnostics

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/url"
	"strings"
	"syscall"
	"testing"
)

func TestNestedTransportErrorsDoNotDiscloseMessagesOrEndpoints(t *testing.T) {
	err := fmt.Errorf("synthetic-private-token: %w", &url.Error{Op: "synthetic-private", URL: "https://synthetic-private.example/token", Err: &net.OpError{Op: "read", Err: syscall.ECONNRESET}})
	got := Error(err)
	if got.IO != "connection_reset" || got.Operation != "read" || got.OSCode == 0 {
		t.Fatal(got)
	}
	if strings.Contains(got.String(), "synthetic-private") {
		t.Fatal(got)
	}
	for _, tc := range []struct {
		err  error
		kind string
	}{{context.Canceled, "canceled"}, {context.DeadlineExceeded, "deadline"}, {io.ErrUnexpectedEOF, "incomplete_body"}, {io.EOF, "eof"}} {
		if got := Error(fmt.Errorf("synthetic-private: %w", tc.err)); got.Kind != tc.kind {
			t.Fatal(got)
		}
	}
}

type cycle struct{}

func (cycle) Error() string   { panic("must not format errors") }
func (c cycle) Unwrap() error { return c }

func TestErrorTraversalBoundsCyclesAndJoins(t *testing.T) {
	got := Error(cycle{})
	if got.Depth != 16 || !got.Truncated {
		t.Fatal(got)
	}
	got = Error(errors.Join(io.ErrUnexpectedEOF, context.Canceled))
	if got.Depth != 3 || got.Truncated {
		t.Fatal(got)
	}
}
