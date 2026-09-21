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
) (*storage.TokenUsage, streamOutcome, streamStopReason) {
	observer := usage.NewObserver(contentType)
	guard := newHTTPStreamGuard(ctx, timeouts, abort)
	defer guard.stop()
	controller := http.NewResponseController(writer)
	buffer := make([]byte, 32*1024)
	var events uint64
	for {
		count, readErr := body.Read(buffer)
		if count > 0 {
			chunk := buffer[:count]
			observer.Observe(chunk)
			observed, terminal := observer.StreamProgress()
			guard.observe(!observer.IsStreaming() || observed != events, terminal != usage.TerminalUnknown)
			events = observed
			// net/http clears the deadline after finishing the response. Leave it in force
			// through final trailer/connection flushing, including after a write timeout.
			if err := controller.SetWriteDeadline(time.Now().Add(timeouts.write)); err != nil && !errors.Is(err, http.ErrNotSupported) {
				guard.stop()
				abort()
				return observer.Usage(), streamClientDisconnected, writeStopReason(err)
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
				return observer.Usage(), streamClientDisconnected, writeStopReason(writeErr)
			}
		}
		if readErr == nil {
			continue
		}
		reason := guard.stop()
		partialTail := observer.HasPendingSSEData()
		observedUsage := observer.Usage()
		if reason == streamStopIdle {
			return observedUsage, streamUpstreamError, reason
		}
		if reason == streamStopCanceled || (ctx.Err() != nil && reason == streamStopNone) {
			return observedUsage, streamClientDisconnected, streamStopCanceled
		}
		if reason == streamStopTail {
			if partialTail {
				return observedUsage, streamUpstreamError, streamStopPartialTail
			}
			return observedUsage, observedStreamOutcome(observer), reason
		}
		if readErr != io.EOF {
			return observedUsage, streamUpstreamError, reason
		}
		return observedUsage, observedStreamOutcome(observer), reason
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
