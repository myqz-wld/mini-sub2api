package httpapi

import (
	"context"
	"errors"
	"net/http"

	"github.com/coder/websocket"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

type websocketCore interface {
	DialWebSocket(
		context.Context,
		string,
		string,
		string,
		http.Header,
	) (*websocket.Conn, *http.Response, error)
}

func (h *Handler) serveWebSocket(
	writer http.ResponseWriter,
	request *http.Request,
	connectionID string,
) {
	secret, ok := bearerToken(request.Header)
	if !ok {
		writeOpenAIError(writer, http.StatusUnauthorized, "invalid_api_key", "The API key is invalid or unavailable.", connectionID)
		return
	}
	route, err := h.store.AuthenticateConnection(request.Context(), secret)
	secret = ""
	if err != nil {
		if errors.Is(err, storage.ErrUnauthorized) {
			writeOpenAIError(writer, http.StatusUnauthorized, "invalid_api_key", "The API key is invalid or unavailable.", connectionID)
			return
		}
		writeOpenAIError(writer, http.StatusInternalServerError, "internal_error", "The request could not be authenticated.", connectionID)
		return
	}
	status, code, message := validateWebSocketHandshake(request)
	if status != 0 {
		writeInvalidWebSocketHandshake(writer, status, code, message, connectionID)
		return
	}
	core, ok := h.core.(websocketCore)
	if !ok || route.Adapter != "codex" {
		writeOpenAIError(writer, http.StatusBadGateway, "adapter_unavailable", "The selected adapter is unavailable.", connectionID)
		return
	}
	if !h.websockets.acquire(route.APIKeyID) {
		writeOpenAIError(writer, http.StatusTooManyRequests, "websocket_connection_limit", "The API key has too many active WebSocket connections.", connectionID)
		return
	}
	defer h.websockets.release(route.APIKeyID)

	coreSocket, coreResponse, err := core.DialWebSocket(
		request.Context(), route.AccountRef, route.PseudonymScope, connectionID,
		allowedRequestHeaders(request.Header),
	)
	if err != nil {
		h.writeWebSocketDialError(writer, route, connectionID, coreResponse, err)
		return
	}
	defer coreSocket.CloseNow()
	if coreResponse == nil {
		writeOpenAIError(writer, http.StatusBadGateway, "upstream_unavailable", "The upstream service is unavailable.", connectionID)
		return
	}
	copyWebSocketUpgradeHeaders(writer.Header(), coreResponse.Header, connectionID)
	providerRequestID := providerRequestIDFromHeaders(coreResponse.Header)
	writer.Header().Set("X-Mini-Sub2Api-Request-Id", connectionID)
	publicSocket, err := websocket.Accept(writer, request, &websocket.AcceptOptions{
		CompressionMode: websocket.CompressionNoContextTakeover,
	})
	if err != nil {
		return
	}
	defer publicSocket.CloseNow()
	publicSocket.SetReadLimit(maxRequestBytes)

	session := newWebSocketSession(h, route, publicSocket, coreSocket, providerRequestID)
	if !h.websockets.register(session) {
		session.stop()
		return
	}
	defer h.websockets.unregister(session)
	session.run()
}

func (h *Handler) writeWebSocketDialError(
	writer http.ResponseWriter,
	route storage.Route,
	connectionID string,
	response *http.Response,
	err error,
) {
	if response != nil && response.StatusCode != http.StatusSwitchingProtocols && response.Body != nil {
		if coreError, ok := detectCoreError(response, connectionID); ok {
			copyResponseHeaders(writer.Header(), response.Header, connectionID)
			if coreError.Code == "credential_requires_login" {
				_ = h.store.MarkCredentialRequiresLogin(context.Background(), route.CredentialID)
			}
			writeOpenAIErrorWithFailure(
				writer, response.StatusCode, coreError.Code, coreError.Message, connectionID,
				coreError.FailureMetadata,
			)
			return
		}
		writeWebSocketHandshakeRejection(writer, response, connectionID)
		return
	}
	status := http.StatusBadGateway
	if errors.Is(err, adapter.ErrUnavailable) {
		status = http.StatusServiceUnavailable
	}
	failure := publicFailure("adapter_unavailable")
	failure.Phase = protocolv1.PhaseUpstreamConnect
	writeOpenAIErrorWithFailure(
		writer, status, "upstream_unavailable", "The upstream service is unavailable.", connectionID,
		failure,
	)
}
