package httpapi

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

const maxCoreErrorBytes = 64 * 1024

var knownCoreErrors = map[string]bool{
	"invalid_internal_auth":     true,
	"unsupported_protocol":      true,
	"invalid_request":           true,
	"unknown_account":           true,
	"credential_disabled":       true,
	"credential_requires_login": true,
	"credential_busy":           true,
	"upstream_connect_failed":   true,
	"upstream_auth_failed":      true,
	"internal_error":            true,
}

func detectCoreError(response *http.Response, requestID string) (protocolv1.CoreError, bool) {
	if response.StatusCode < 400 {
		return protocolv1.CoreError{}, false
	}
	data, err := io.ReadAll(io.LimitReader(response.Body, maxCoreErrorBytes+1))
	if err != nil {
		response.Body = replayBody(data, response.Body)
		return protocolv1.CoreError{}, false
	}
	if len(data) <= maxCoreErrorBytes {
		var envelope protocolv1.ErrorEnvelope
		if json.Unmarshal(data, &envelope) == nil && knownCoreErrors[envelope.Error.Code] &&
			envelope.Error.RequestID == requestID {
			return envelope.Error, true
		}
	}
	response.Body = replayBody(data, response.Body)
	return protocolv1.CoreError{}, false
}

type replayReadCloser struct {
	io.Reader
	closer io.Closer
}

func (r *replayReadCloser) Close() error {
	return r.closer.Close()
}

func replayBody(prefix []byte, original io.ReadCloser) io.ReadCloser {
	return &replayReadCloser{
		Reader: io.MultiReader(bytes.NewReader(prefix), original),
		closer: original,
	}
}

func writeOpenAIError(writer http.ResponseWriter, status int, code, message, requestID string) {
	writer.Header().Set("Content-Type", "application/json")
	if requestID != "" {
		writer.Header().Set("X-Mini-Sub2Api-Request-Id", requestID)
	}
	writer.WriteHeader(status)
	_ = json.NewEncoder(writer).Encode(map[string]any{
		"error": map[string]any{
			"message": message,
			"type":    "mini_sub2api_error",
			"code":    code,
		},
	})
}
