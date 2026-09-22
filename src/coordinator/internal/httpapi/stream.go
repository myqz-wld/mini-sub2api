package httpapi

import (
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
	"mini-sub2api/src/coordinator/internal/usage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

type streamOutcome int

const (
	streamComplete streamOutcome = iota
	streamResponseFailed
	streamUpstreamError
	streamClientDisconnected
)

func streamBody(
	writer http.ResponseWriter,
	body io.Reader,
	contentType string,
	ctx context.Context,
	timeouts httpStreamTimeouts,
	abort func(),
) (tokens *storage.TokenUsage, outcome streamOutcome, reason streamStopReason, diagnostics httpStreamDiagnostics) {
	observer := usage.NewObserver(contentType)
	diagnostics = newHTTPStreamDiagnostics()
	guard := newHTTPStreamGuard(ctx, timeouts, abort)
	defer guard.stop()
	watchOutput := observer.IsStreaming()
	if watchOutput {
		guard.enableOutputDeadline()
	}
	controller := http.NewResponseController(writer)
	buffer := make([]byte, 32*1024)
	for {
		count, readErr := body.Read(buffer)
		if count > 0 {
			chunk := buffer[:count]
			observer.Observe(chunk)
			if observer.IsStreaming() && !watchOutput {
				watchOutput = true
				guard.enableOutputDeadline()
			}
			progress := observer.Progress()
			guard.observe(!observer.IsStreaming() || progress.Events != diagnostics.progress.Events,
				progress.Outputs != diagnostics.progress.Outputs, progress.Classes[protocolv1.SSETerminal] > 0)
			diagnostics.observe(count, progress)
			// net/http clears the deadline after finishing the response. Leave it in force
			// through final trailer/connection flushing, including after a write timeout.
			if err := controller.SetWriteDeadline(time.Now().Add(timeouts.write)); err != nil && !errors.Is(err, http.ErrNotSupported) {
				guard.stop()
				abort()
				return observer.Usage(), streamClientDisconnected, writeStopReason(err), diagnostics
			}
			written, writeErr := writer.Write(chunk)
			if writeErr == nil && written != len(chunk) {
				writeErr = io.ErrShortWrite
			}
			if writeErr == nil {
				if err := controller.Flush(); err != nil && !errors.Is(err, http.ErrNotSupported) {
					writeErr = err
				}
			}
			if writeErr != nil {
				guard.stop()
				abort()
				return observer.Usage(), streamClientDisconnected, writeStopReason(writeErr), diagnostics
			}
		}
		if readErr == nil {
			continue
		}
		reason := guard.stop()
		partialTail := observer.HasPendingSSEData()
		observedUsage := observer.Usage()
		// Natural EOF may finish the final event. Forced closure must not report an
		// unfinished fragment as output that arrived before the timeout/cancellation.
		if reason == streamStopNone && readErr == io.EOF {
			diagnostics.observe(0, observer.Progress())
		}
		if reason == streamStopIdle || reason == streamStopFirstOutput || reason == streamStopOutputIdle {
			return observedUsage, streamUpstreamError, reason, diagnostics
		}
		if reason == streamStopCanceled || (ctx.Err() != nil && reason == streamStopNone) {
			return observedUsage, streamClientDisconnected, streamStopCanceled, diagnostics
		}
		if reason == streamStopTail {
			if partialTail {
				return observedUsage, streamUpstreamError, streamStopPartialTail, diagnostics
			}
			return observedUsage, observedStreamOutcome(observer), reason, diagnostics
		}
		if readErr != io.EOF {
			return observedUsage, streamUpstreamError, reason, diagnostics
		}
		return observedUsage, observedStreamOutcome(observer), reason, diagnostics
	}
}

func observedStreamOutcome(observer *usage.Observer) streamOutcome {
	if observer.TerminalStatus() == usage.TerminalUpstreamError {
		return streamResponseFailed
	}
	if observer.IsStreaming() && observer.TerminalStatus() != usage.TerminalCompleted {
		return streamUpstreamError
	}
	return streamComplete
}

func writeStopReason(err error) streamStopReason {
	var timeout net.Error
	if errors.As(err, &timeout) && timeout.Timeout() {
		return streamStopWriteTimeout
	}
	return streamStopCanceled
}
