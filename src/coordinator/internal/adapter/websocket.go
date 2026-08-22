package adapter

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"time"

	"github.com/coder/websocket"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

var internalWebSocketHTTPClient = &http.Client{
	Transport: &http.Transport{
		Proxy:                 nil,
		DialContext:           (&net.Dialer{Timeout: 5 * time.Second}).DialContext,
		ForceAttemptHTTP2:     false,
		ResponseHeaderTimeout: 30 * time.Second,
	},
	CheckRedirect: func(*http.Request, []*http.Request) error {
		return http.ErrUseLastResponse
	},
}

func (s *Supervisor) DialWebSocket(
	ctx context.Context,
	accountRef, requestID string,
	headers http.Header,
) (*websocket.Conn, *http.Response, error) {
	core, err := s.snapshot()
	if err != nil {
		return nil, nil, err
	}
	if !core.readiness.Capabilities.ResponsesWebSocket {
		return nil, nil, ErrUnavailable
	}
	requestHeaders := make(http.Header)
	copyForwardedHeaders(requestHeaders, headers)
	requestHeaders.Set("Authorization", "Bearer "+core.token)
	requestHeaders.Set(protocolv1.VersionHeader, protocolv1.Version)
	requestHeaders.Set(protocolv1.AccountRefHeader, accountRef)
	requestHeaders.Set(protocolv1.RequestIDHeader, requestID)
	connection, response, err := websocket.Dial(
		ctx,
		core.baseURL+"/internal/v1/responses/ws",
		&websocket.DialOptions{
			HTTPClient:      internalWebSocketHTTPClient,
			HTTPHeader:      requestHeaders,
			CompressionMode: websocket.CompressionDisabled,
		},
	)
	if err != nil {
		return nil, response, fmt.Errorf("call Codex core WebSocket: %w", err)
	}
	connection.SetReadLimit(16 * 1024 * 1024)
	return connection, response, nil
}
