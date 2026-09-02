package httpapi

import (
	"fmt"
	"net/http"
	"strconv"
	"time"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

var allowedHeaders = map[string]bool{
	"Accept":                                 true,
	"Content-Encoding":                       true,
	"Content-Type":                           true,
	"Originator":                             true,
	"Session-Id":                             true,
	"Thread-Id":                              true,
	"User-Agent":                             true,
	"Version":                                true,
	"Openai-Beta":                            true,
	"Openai-Organization":                    true,
	"Openai-Project":                         true,
	"X-Client-Request-Id":                    true,
	"X-Codex-Beta-Features":                  true,
	"X-Codex-Inference-Call-Id":              true,
	"X-Codex-Turn-State":                     true,
	"X-Codex-Turn-Metadata":                  true,
	"X-Codex-Parent-Thread-Id":               true,
	"X-Openai-Subagent":                      true,
	"X-Codex-Window-Id":                      true,
	"X-Codex-Installation-Id":                true,
	"X-Openai-Internal-Codex-Responses-Lite": true,
	"X-Openai-Internal-Codex-Residency":      true,
	"X-Openai-Memgen-Request":                true,
	"X-Oai-Attestation":                      true,
	"X-Responsesapi-Include-Timing-Metrics":  true,
	"X-Stainless-Arch":                       true,
	"X-Stainless-Lang":                       true,
	"X-Stainless-Os":                         true,
	"X-Stainless-Package-Version":            true,
	"X-Stainless-Retry-Count":                true,
	"X-Stainless-Runtime":                    true,
	"X-Stainless-Runtime-Version":            true,
	"X-Stainless-Timeout":                    true,
	"Session_id":                             true,
	"Conversation_id":                        true,
}

var publicResponseHeaders = map[string]bool{
	"Content-Type":                   true,
	"Content-Encoding":               true,
	"Cache-Control":                  true,
	"Retry-After":                    true,
	"Retry-After-Ms":                 true,
	"Server-Timing":                  true,
	"Openai-Model":                   true,
	"Openai-Processing-Ms":           true,
	"Openai-Version":                 true,
	"X-Models-Etag":                  true,
	"X-Reasoning-Included":           true,
	"X-Codex-Turn-State":             true,
	"X-Ratelimit-Limit-Requests":     true,
	"X-Ratelimit-Remaining-Requests": true,
	"X-Ratelimit-Reset-Requests":     true,
	"X-Ratelimit-Limit-Tokens":       true,
	"X-Ratelimit-Remaining-Tokens":   true,
	"X-Ratelimit-Reset-Tokens":       true,
	"X-Request-Id":                   true,
	"Openai-Request-Id":              true,
	"Request-Id":                     true,
}

func allowedRequestHeaders(source http.Header) http.Header {
	result := make(http.Header)
	for name, values := range source {
		canonical := http.CanonicalHeaderKey(name)
		if !allowedHeaders[canonical] {
			continue
		}
		for _, value := range values {
			result.Add(canonical, value)
		}
	}
	return result
}

func copyResponseHeaders(destination, source http.Header, gatewayRequestID string) *time.Duration {
	for name, values := range source {
		canonical := http.CanonicalHeaderKey(name)
		if !publicResponseHeaders[canonical] {
			continue
		}
		if isProviderRequestIDHeader(canonical) {
			if gatewayRequestID == "" {
				continue
			}
			for range values {
				destination.Add(canonical, gatewayRequestID)
			}
			continue
		}
		for _, value := range values {
			destination.Add(canonical, value)
		}
	}
	return mergeCoreTTFB(destination, source)
}

func isProviderRequestIDHeader(name string) bool {
	return name == "X-Request-Id" || name == "Openai-Request-Id" || name == "Request-Id"
}

func providerRequestIDFromHeaders(source http.Header) *string {
	value := source.Get(protocolv1.ProviderRequestIDHeader)
	if !validProviderRequestID(value) {
		return nil
	}
	return &value
}

func validProviderRequestID(value string) bool {
	if len(value) == 0 || len(value) > 512 {
		return false
	}
	for index := 0; index < len(value); index++ {
		if value[index] < 0x21 || value[index] > 0x7e {
			return false
		}
	}
	return true
}

func mergeCoreTTFB(destination, source http.Header) *time.Duration {
	rawTTFB := source.Get(protocolv1.CoreTTFBHeader)
	milliseconds, err := strconv.ParseInt(rawTTFB, 10, 64)
	if err != nil || milliseconds < 0 || milliseconds > int64((24*time.Hour)/time.Millisecond) {
		return nil
	}
	timing := fmt.Sprintf("upstream_ttfb;dur=%d", milliseconds)
	if existing := destination.Get("Server-Timing"); existing != "" {
		destination.Set("Server-Timing", existing+", "+timing)
	} else {
		destination.Set("Server-Timing", timing)
	}
	value := time.Duration(milliseconds) * time.Millisecond
	return &value
}
