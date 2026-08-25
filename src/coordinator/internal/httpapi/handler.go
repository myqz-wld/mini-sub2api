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
	"time"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/storage"
)

const maxRequestBytes = 16 * 1024 * 1024

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
	store      *storage.Store
	core       Core
	clock      func() time.Time
	logger     *log.Logger
	websockets *websocketManager
	wsTimeouts websocketTimeouts
}

func NewHandler(store *storage.Store, core Core, logger *log.Logger) *Handler {
	if logger == nil {
		logger = log.New(io.Discard, "", 0)
	}
	return &Handler{
		store: store, core: core, clock: time.Now, logger: logger,
		websockets: newWebSocketManager(maxWebSocketsPerKey),
		wsTimeouts: defaultWebSocketTimeouts(),
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
		h.finish(requestID, started, storage.RequestUpstreamErr, http.StatusBadGateway, nil, nil, nil)
		writeOpenAIError(writer, http.StatusBadGateway, "adapter_unavailable", "The selected adapter is unavailable.", requestID)
		return
	}
	request.Body = http.MaxBytesReader(writer, request.Body, maxRequestBytes)
	body, err := io.ReadAll(request.Body)
	if err != nil {
		status := http.StatusBadRequest
		code := "invalid_request"
		message := "The request body is invalid."
		var tooLarge *http.MaxBytesError
		if errors.As(err, &tooLarge) {
			status = http.StatusRequestEntityTooLarge
			code = "request_too_large"
			message = "The request body exceeds the 16 MiB limit."
		}
		terminal := storage.RequestUpstreamErr
		if request.Context().Err() != nil {
			terminal = storage.RequestDisconnected
		}
		h.finish(requestID, started, terminal, status, nil, nil, nil)
		writeOpenAIError(writer, status, code, message, requestID)
		return
	}
	response, err := h.core.Forward(
		request.Context(), route.AccountRef, route.PseudonymScope, requestID,
		allowedRequestHeaders(request.Header), body,
	)
	body = nil
	if err != nil {
		status := http.StatusBadGateway
		terminal := storage.RequestUpstreamErr
		if errors.Is(err, adapter.ErrUnavailable) {
			status = http.StatusServiceUnavailable
		}
		if request.Context().Err() != nil {
			terminal = storage.RequestDisconnected
		}
		h.finish(requestID, started, terminal, status, nil, nil, nil)
		writeOpenAIError(writer, status, "upstream_unavailable", "The upstream service is unavailable.", requestID)
		return
	}
	defer response.Body.Close()
	if coreError, ok := detectCoreError(response, requestID); ok {
		if coreError.Code == "credential_requires_login" {
			_ = h.store.MarkCredentialRequiresLogin(context.Background(), route.CredentialID)
		}
		h.finish(requestID, started, storage.RequestUpstreamErr, response.StatusCode, nil, nil, nil)
		writeOpenAIErrorWithFailure(
			writer, response.StatusCode, coreError.Code, coreError.Message, requestID,
			coreError.FailureMetadata,
		)
		return
	}
	ttfb := copyResponseHeaders(writer.Header(), response.Header)
	writer.Header().Set("X-Mini-Sub2Api-Request-Id", requestID)
	declareFailureTrailers(writer.Header())
	writer.WriteHeader(response.StatusCode)
	usage, streamResult := streamBody(writer, response.Body, response.Header.Get("Content-Type"), request.Context())
	if failure, ok := failureFromTrailers(response.Trailer); ok {
		publishFailureTrailers(writer.Header(), failure)
		streamResult = streamUpstreamError
	}
	terminal := storage.RequestCompleted
	if response.StatusCode >= 400 || streamResult == streamUpstreamError {
		terminal = storage.RequestUpstreamErr
	}
	if streamResult == streamClientDisconnected {
		terminal = storage.RequestDisconnected
	}
	h.finish(requestID, started, terminal, response.StatusCode, ttfb, usage, nil)
}

func (h *Handler) finish(
	requestID string,
	started time.Time,
	status string,
	httpStatus int,
	ttfb *time.Duration,
	usage *storage.TokenUsage,
	completedAt *time.Time,
) {
	completed := h.clock().UTC()
	if completedAt != nil {
		completed = *completedAt
	}
	code := httpStatus
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	err := h.store.FinalizeRequest(ctx, requestID, storage.RequestResult{
		CompletedAt: completed,
		Status:      status,
		HTTPStatus:  &code,
		TTFB:        ttfb,
		Duration:    completed.Sub(started),
		Usage:       usage,
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
