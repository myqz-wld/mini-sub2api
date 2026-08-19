package httpapi

import (
	"context"
	"io"
	"net/http"

	"mini-sub2api/src/coordinator/internal/storage"
	"mini-sub2api/src/coordinator/internal/usage"
)

type streamOutcome int

const (
	streamComplete streamOutcome = iota
	streamUpstreamError
	streamClientDisconnected
)

func streamBody(
	writer http.ResponseWriter,
	body io.Reader,
	contentType string,
	ctx context.Context,
) (*storage.TokenUsage, streamOutcome) {
	observer := usage.NewObserver(contentType)
	buffer := make([]byte, 32*1024)
	for {
		count, readErr := body.Read(buffer)
		if count > 0 {
			chunk := buffer[:count]
			observer.Observe(chunk)
			written, writeErr := writer.Write(chunk)
			if writeErr != nil || written != len(chunk) {
				return observer.Usage(), streamClientDisconnected
			}
			if flushErr := http.NewResponseController(writer).Flush(); flushErr != nil &&
				flushErr != http.ErrNotSupported {
				return observer.Usage(), streamClientDisconnected
			}
		}
		if readErr == io.EOF {
			return observer.Usage(), streamComplete
		}
		if readErr != nil {
			select {
			case <-ctx.Done():
				return observer.Usage(), streamClientDisconnected
			default:
				return observer.Usage(), streamUpstreamError
			}
		}
	}
}
