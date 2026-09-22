package httpapi

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"strings"
	"sync"
	"time"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

var maxRequestBytes = int(protocolv1.MustInferenceLimits().RequestBytes)

type Core interface {
	Forward(
		context.Context,
		string,
		string,
		string,
		http.Header,
		[]byte,
	) (*http.Response, error)
}

type Handler struct {
	requestLimit int64
	store        *storage.Store
	core         Core
	clock        func() time.Time
	logger       *log.Logger
	websockets   *websocketManager
	wsTimeouts   websocketTimeouts
	httpTimeouts httpStreamTimeouts
}

func NewHandler(store *storage.Store, core Core, logger *log.Logger) *Handler {
	if logger == nil {
		logger = log.New(io.Discard, "", 0)
	}
	return &Handler{
		store: store, core: core, clock: time.Now, logger: logger,
		requestLimit: int64(protocolv1.MustInferenceLimits().RequestBytes),
		websockets:   newWebSocketManager(maxWebSocketsPerKey),
		wsTimeouts:   defaultWebSocketTimeouts(),
		httpTimeouts: defaultHTTPStreamTimeouts(),
	}
}

func (h *Handler) ServeHTTP(writer http.ResponseWriter, request *http.Request) {
	requestID, err := newRequestID()
	if err != nil {
		writeOpenAIError(writer, http.StatusInternalServerError, "internal_error", "The service could not create a request identifier.", "")
		return
	}
	writer.Header().Set("X-Mini-Sub2Api-Request-Id", requestID)
	if request.URL.Path != "/v1/responses" || request.URL.RawQuery != "" {
		writeOpenAIError(writer, http.StatusNotFound, "not_found", "The requested endpoint does not exist.", requestID)
		return
	}
	switch request.Method {
	case http.MethodPost:
		h.serveHTTPResponses(writer, request, requestID)
	case http.MethodGet:
		h.serveWebSocket(writer, request, requestID)
	default:
		writeOpenAIError(writer, http.StatusNotFound, "not_found", "The requested endpoint does not exist.", requestID)
	}
}

func (h *Handler) serveHTTPResponses(writer http.ResponseWriter, request *http.Request, requestID string) {
	secret, ok := bearerToken(request.Header)
	if !ok {
		writeOpenAIError(writer, http.StatusUnauthorized, "invalid_api_key", "The API key is invalid or unavailable.", requestID)
		return
	}
	started := h.clock().UTC()
	route, err := h.store.AuthenticateAndStart(request.Context(), secret, requestID)
	secret = ""
	if err != nil {
		if errors.Is(err, storage.ErrUnauthorized) {
			writeOpenAIError(writer, http.StatusUnauthorized, "invalid_api_key", "The API key is invalid or unavailable.", requestID)
			return
		}
		writeOpenAIError(writer, http.StatusInternalServerError, "internal_error", "The request could not be authenticated.", requestID)
		return
	}
	if route.Adapter != "codex" {
		h.finish(requestID, started, storage.RequestUpstreamErr, http.StatusBadGateway, nil, nil, nil, nil)
		writeOpenAIError(writer, http.StatusBadGateway, "adapter_unavailable", "The selected adapter is unavailable.", requestID)
		return
	}
	request.Body = http.MaxBytesReader(writer, request.Body, h.requestLimit)
	body, err := io.ReadAll(request.Body)
	if err != nil {
		status := http.StatusBadRequest
		code := "invalid_request"
		message := "The request body is invalid."
		var tooLarge *http.MaxBytesError
		if errors.As(err, &tooLarge) {
			status = http.StatusRequestEntityTooLarge
			code = "request_too_large"
			message = fmt.Sprintf("The request body exceeds the %d-byte limit.", h.requestLimit)
		}
		terminal := storage.RequestUpstreamErr
		if request.Context().Err() != nil {
			terminal = storage.RequestDisconnected
		}
		h.finish(requestID, started, terminal, status, nil, nil, nil, nil)
		writeOpenAIError(writer, status, code, message, requestID)
		return
	}
	forwardContext, cancelForward := context.WithCancel(request.Context())
	defer cancelForward()
	response, err := h.core.Forward(
		forwardContext, route.AccountRef, route.PseudonymScope, requestID,
		allowedRequestHeaders(request.Header), body,
	)
	body = nil
	if err != nil {
		status := http.StatusBadGateway
		code := "upstream_unavailable"
		terminal := storage.RequestUpstreamErr
		if errors.Is(err, adapter.ErrUnavailable) {
			status = http.StatusServiceUnavailable
			code = "adapter_unavailable"
		}
		if request.Context().Err() != nil {
			terminal = storage.RequestDisconnected
		}
		h.finish(requestID, started, terminal, status, nil, nil, nil, nil)
		writeOpenAIError(writer, status, code, "The upstream service is unavailable.", requestID)
		return
	}
	upstreamBody := response.Body
	closeUpstream := sync.OnceFunc(func() {
		cancelForward()
		_ = upstreamBody.Close()
	})
	defer closeUpstream()
	providerRequestID := providerRequestIDFromHeaders(response.Header)
	// Error-envelope inspection happens before streamBody and must also be bounded.
	// It reads at most 64 KiB; even a trickling JSON error cannot hold it indefinitely.
	var coreError protocolv1.CoreError
	var isCoreError bool
	var inspectionReason streamStopReason
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		inspection := newHTTPStreamGuard(request.Context(), h.httpTimeouts, closeUpstream)
		coreError, isCoreError = detectCoreError(response, requestID)
		inspectionReason = inspection.stop()
	}
	if inspectionReason != streamStopNone {
		terminal := storage.RequestUpstreamErr
		if inspectionReason == streamStopCanceled {
			terminal = storage.RequestDisconnected
		}
		h.logger.Printf("request %s HTTP error inspection ended: %s", requestID, inspectionReason)
		ttfb := copyResponseHeaders(writer.Header(), response.Header, requestID)
		h.finish(requestID, started, terminal, http.StatusBadGateway, ttfb, nil, nil, providerRequestID)
		writeOpenAIError(writer, http.StatusBadGateway, "upstream_unavailable", "The upstream service is unavailable.", requestID)
		return
	}
	if isCoreError {
		ttfb := copyResponseHeaders(writer.Header(), response.Header, requestID)
		if coreError.Code == "credential_requires_login" {
			_ = h.store.MarkCredentialRequiresLogin(context.Background(), route.CredentialID)
		}
		h.finish(requestID, started, storage.RequestUpstreamErr, response.StatusCode, ttfb, nil, nil, providerRequestID)
		writeOpenAIErrorWithFailure(
			writer, response.StatusCode, coreError.Code, coreError.Message, requestID,
			coreError.FailureMetadata,
		)
		return
	}
	ttfb := copyResponseHeaders(writer.Header(), response.Header, requestID)
	writer.Header().Set("X-Mini-Sub2Api-Request-Id", requestID)
	declareFailureTrailers(writer.Header())
	writer.WriteHeader(response.StatusCode)
	usage, streamResult, stopReason, diagnostics := streamBody(
		writer, response.Body, response.Header.Get("Content-Type"), request.Context(), h.httpTimeouts, closeUpstream,
	)
	if streamResult == streamComplete && responseTerminalFailed(response.Header) {
		streamResult = streamResponseFailed
	}
	if failure, ok := failureFromTrailers(response.Trailer); ok {
		publishFailureTrailers(writer.Header(), failure)
		streamResult = streamUpstreamError
	} else if streamResult == streamUpstreamError {
		publishFailureTrailers(writer.Header(), protocolv1.FailureMetadata{
			RetryAdvice: protocolv1.RetryNever, Phase: protocolv1.PhaseUpstreamStream,
			DeliveryState: protocolv1.DeliveryDelivered,
		})
	}
	terminal := storage.RequestCompleted
	if response.StatusCode >= 400 || streamResult == streamResponseFailed || streamResult == streamUpstreamError {
		terminal = storage.RequestUpstreamErr
	}
	if streamResult == streamClientDisconnected {
		terminal = storage.RequestDisconnected
	}
	reason := string(stopReason)
	if reason == "" {
		reason = string(terminal)
	}
	h.logger.Printf("request %s HTTP stream ended: %s outcome=%s %s", requestID, reason, terminal, diagnostics)
	h.finish(requestID, started, terminal, response.StatusCode, ttfb, usage, nil, providerRequestID)
}

func responseTerminalFailed(header http.Header) bool {
	switch header.Get(protocolv1.ResponseTerminalHeader) {
	case protocolv1.ResponseTerminalFailed, protocolv1.ResponseTerminalIncomplete:
		return true
	default:
		return false
	}
}

func (h *Handler) finish(
	requestID string,
	started time.Time,
	status string,
	httpStatus int,
	ttfb *time.Duration,
	usage *storage.TokenUsage,
	completedAt *time.Time,
	providerRequestID *string,
) {
	completed := h.clock().UTC()
	if completedAt != nil {
		completed = *completedAt
	}
	code := httpStatus
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	err := h.store.FinalizeRequest(ctx, requestID, storage.RequestResult{
		CompletedAt:       completed,
		Status:            status,
		HTTPStatus:        &code,
		TTFB:              ttfb,
		Duration:          completed.Sub(started),
		Usage:             usage,
		ProviderRequestID: providerRequestID,
	})
	if err != nil {
		h.logger.Printf("request %s history finalization failed: %v", requestID, err)
	}
}

func newRequestID() (string, error) {
	value := make([]byte, 16)
	if _, err := rand.Read(value); err != nil {
		return "", fmt.Errorf("generate request id: %w", err)
	}
	return "req_" + base64.RawURLEncoding.EncodeToString(value), nil
}

func bearerToken(header http.Header) (string, bool) {
	values := header.Values("Authorization")
	if len(values) != 1 {
		return "", false
	}
	parts := strings.SplitN(strings.TrimSpace(values[0]), " ", 2)
	if len(parts) != 2 || !strings.EqualFold(parts[0], "Bearer") || parts[1] == "" {
		return "", false
	}
	return parts[1], true
}
