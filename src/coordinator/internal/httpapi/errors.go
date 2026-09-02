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
	"invalid_internal_auth":       true,
	"unsupported_protocol":        true,
	"invalid_request":             true,
	"unknown_account":             true,
	"state_unavailable":           true,
	"credential_disabled":         true,
	"credential_requires_login":   true,
	"credential_busy":             true,
	"upstream_connect_failed":     true,
	"upstream_delivery_unknown":   true,
	"upstream_response_failed":    true,
	"upstream_handshake_rejected": true,
	"upstream_auth_failed":        true,
	"internal_error":              true,
}

func detectCoreError(response *http.Response, requestID string) (protocolv1.CoreError, bool) {
	if response.StatusCode >= 200 && response.StatusCode < 300 {
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
			envelope.Error.RequestID == requestID && envelope.Error.FailureMetadata.Valid() {
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
	writeOpenAIErrorWithFailure(writer, status, code, message, requestID, publicFailure(code))
}

func writeOpenAIErrorWithFailure(
	writer http.ResponseWriter,
	status int,
	code, message, requestID string,
	failure protocolv1.FailureMetadata,
) {
	if !failure.Valid() {
		failure = protocolv1.FailureMetadata{
			RetryAdvice: protocolv1.RetryNever, Phase: protocolv1.PhaseInternal,
			DeliveryState: protocolv1.DeliveryNotDelivered,
		}
	}
	writer.Header().Set("Content-Type", "application/json")
	if requestID != "" {
		writer.Header().Set("X-Mini-Sub2Api-Request-Id", requestID)
	}
	writer.WriteHeader(status)
	_ = json.NewEncoder(writer).Encode(map[string]any{
		"error": map[string]any{
			"message":       message,
			"type":          "mini_sub2api_error",
			"code":          code,
			"retryAdvice":   failure.RetryAdvice,
			"phase":         failure.Phase,
			"deliveryState": failure.DeliveryState,
		},
	})
}

func publicFailure(code string) protocolv1.FailureMetadata {
	failure := protocolv1.FailureMetadata{
		RetryAdvice: protocolv1.RetryNever, Phase: protocolv1.PhaseRequest,
		DeliveryState: protocolv1.DeliveryNotDelivered,
	}
	switch code {
	case "invalid_api_key", "credential_requires_login":
		failure.Phase = protocolv1.PhaseCredential
	case "adapter_unavailable":
		failure.RetryAdvice = protocolv1.RetrySafe
		failure.Phase = protocolv1.PhaseInternal
	case "upstream_unavailable":
		failure.RetryAdvice = protocolv1.RetryAmbiguous
		failure.Phase = protocolv1.PhaseUpstreamRequest
		failure.DeliveryState = protocolv1.DeliveryPossiblyDelivered
	case "internal_error":
		failure.Phase = protocolv1.PhaseInternal
	}
	return failure
}

func declareFailureTrailers(header http.Header) {
	for _, name := range []string{
		protocolv1.FailurePhaseTrailer,
		protocolv1.DeliveryStateTrailer,
		protocolv1.RetryAdviceTrailer,
	} {
		header.Add("Trailer", name)
	}
}

func failureFromTrailers(header http.Header) (protocolv1.FailureMetadata, bool) {
	metadata := protocolv1.FailureMetadata{
		RetryAdvice:   protocolv1.RetryAdvice(header.Get(protocolv1.RetryAdviceTrailer)),
		Phase:         protocolv1.FailurePhase(header.Get(protocolv1.FailurePhaseTrailer)),
		DeliveryState: protocolv1.DeliveryState(header.Get(protocolv1.DeliveryStateTrailer)),
	}
	return metadata, metadata.Valid()
}

func publishFailureTrailers(header http.Header, metadata protocolv1.FailureMetadata) {
	if !metadata.Valid() {
		return
	}
	header.Set(protocolv1.FailurePhaseTrailer, string(metadata.Phase))
	header.Set(protocolv1.DeliveryStateTrailer, string(metadata.DeliveryState))
	header.Set(protocolv1.RetryAdviceTrailer, string(metadata.RetryAdvice))
}
