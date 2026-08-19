package adapter

import (
	"bytes"
	"context"
	"fmt"
	"net"
	"net/http"
	"time"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

var forwardedRequestHeaders = map[string]bool{
	"Accept":                   true,
	"Content-Type":             true,
	"User-Agent":               true,
	"Openai-Beta":              true,
	"X-Client-Request-Id":      true,
	"X-Codex-Beta-Features":    true,
	"X-Codex-Turn-State":       true,
	"X-Codex-Turn-Metadata":    true,
	"X-Codex-Parent-Thread-Id": true,
	"X-Codex-Window-Id":        true,
	"X-Codex-Installation-Id":  true,
	"Session_id":               true,
	"Conversation_id":          true,
}

var internalHTTPClient = &http.Client{
	Transport: &http.Transport{
		Proxy:                 nil,
		DialContext:           (&net.Dialer{Timeout: 5 * time.Second}).DialContext,
		ForceAttemptHTTP2:     false,
		MaxIdleConns:          16,
		MaxIdleConnsPerHost:   16,
		IdleConnTimeout:       90 * time.Second,
		ResponseHeaderTimeout: 310 * time.Second,
	},
	CheckRedirect: func(*http.Request, []*http.Request) error {
		return http.ErrUseLastResponse
	},
}

func (s *Supervisor) Forward(
	ctx context.Context,
	accountRef, requestID string,
	headers http.Header,
	body []byte,
) (*http.Response, error) {
	core, err := s.snapshot()
	if err != nil {
		return nil, err
	}
	request, err := http.NewRequestWithContext(
		ctx, http.MethodPost, core.baseURL+"/internal/v1/responses", bytes.NewReader(body),
	)
	if err != nil {
		return nil, fmt.Errorf("create internal core request: %w", err)
	}
	for name, values := range headers {
		canonical := http.CanonicalHeaderKey(name)
		if !forwardedRequestHeaders[canonical] {
			continue
		}
		for _, value := range values {
			request.Header.Add(canonical, value)
		}
	}
	request.Header.Set("Authorization", "Bearer "+core.token)
	request.Header.Set(protocolv1.VersionHeader, protocolv1.Version)
	request.Header.Set(protocolv1.AccountRefHeader, accountRef)
	request.Header.Set(protocolv1.RequestIDHeader, requestID)
	response, err := internalHTTPClient.Do(request)
	if err != nil {
		return nil, fmt.Errorf("call Codex core: %w", err)
	}
	return response, nil
}
