// Package diagnostics observes fixed metadata without formatting untrusted errors.
package diagnostics

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"io"
	"net"
	"net/http"
	"syscall"
)

type ErrorInfo struct {
	Kind, IO, Operation        string
	OSCode, StorageCode, Depth int
	Truncated                  bool
}

func Error(err error) ErrorInfo {
	out := ErrorInfo{Kind: "none", IO: "none", Operation: "none"}
	if err == nil {
		return out
	}
	out.Kind = "other"
	var pending [16]error
	pending[0] = err
	count := 1
	for count > 0 && out.Depth < 16 {
		count--
		current := pending[count]
		pending[count] = nil
		if current == nil {
			continue
		}
		out.Depth++
		switch current {
		case context.Canceled:
			out.Kind = "canceled"
		case context.DeadlineExceeded:
			out.Kind = "deadline"
		case io.EOF:
			out.Kind = "eof"
		case io.ErrUnexpectedEOF:
			out.Kind = "incomplete_body"
			out.IO = "unexpected_eof"
		case io.ErrClosedPipe:
			out.Kind = "io"
			out.IO = "closed_pipe"
		case net.ErrClosed:
			out.Kind = "io"
			out.IO = "closed"
		}
		switch value := current.(type) {
		case *net.OpError:
			switch value.Op {
			case "dial", "read", "write", "accept":
				out.Operation = value.Op
			}
		case *net.DNSError:
			out.Kind = "dns"
			if value.IsTimeout {
				out.Kind = "dns_timeout"
			} else if value.IsNotFound {
				out.Kind = "dns_not_found"
			}
		case syscall.Errno:
			out.Kind = "io"
			out.OSCode = int(value)
			switch value {
			case syscall.ECONNRESET:
				out.IO = "connection_reset"
			case syscall.ECONNABORTED:
				out.IO = "connection_aborted"
			case syscall.ECONNREFUSED:
				out.IO = "connection_refused"
			case syscall.EPIPE:
				out.IO = "broken_pipe"
			case syscall.ETIMEDOUT:
				out.IO = "timeout"
			case syscall.ENETUNREACH:
				out.IO = "network_unreachable"
			case syscall.EHOSTUNREACH:
				out.IO = "host_unreachable"
			default:
				out.IO = "other"
			}
		case *tls.CertificateVerificationError, x509.CertificateInvalidError, x509.UnknownAuthorityError, x509.HostnameError:
			out.Kind = "tls_certificate"
		case tls.RecordHeaderError:
			out.Kind = "tls_record_header"
		case *http.MaxBytesError:
			out.Kind = "request_too_large"
		case interface{ Code() int }:
			out.Kind = "storage"
			out.StorageCode = value.Code()
		case interface{ Timeout() bool }:
			if value.Timeout() && out.Kind != "deadline" {
				out.Kind = "timeout"
			}
		}
		switch value := current.(type) {
		case interface{ Unwrap() error }:
			if next := value.Unwrap(); next != nil {
				if count == len(pending) {
					out.Truncated = true
				} else {
					pending[count] = next
					count++
				}
			}
		case interface{ Unwrap() []error }:
			for index, next := range value.Unwrap() {
				if index == len(pending) {
					out.Truncated = true
					break
				}
				if next == nil {
					continue
				}
				if count == len(pending) {
					out.Truncated = true
					break
				}
				pending[count] = next
				count++
			}
		}
	}
	out.Truncated = out.Truncated || count > 0
	return out
}

func (e ErrorInfo) String() string {
	return fmt.Sprintf("error_kind=%s io_kind=%s io_operation=%s os_error=%d storage_code=%d source_depth=%d source_truncated=%t", e.Kind, e.IO, e.Operation, e.OSCode, e.StorageCode, e.Depth, e.Truncated)
}
