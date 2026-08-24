package httpapi

import (
	"fmt"
	"net/http"
	"strconv"
	"strings"
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

func copyResponseHeaders(destination, source http.Header) *time.Duration {
	connectionHeaders := nominatedConnectionHeaders(source)
	for name, values := range source {
		if !safeResponseHeader(name) || connectionHeaders[strings.ToLower(name)] {
			continue
		}
		for _, value := range values {
			destination.Add(name, value)
		}
	}
	return mergeCoreTTFB(destination, source)
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

func nominatedConnectionHeaders(source http.Header) map[string]bool {
	result := make(map[string]bool)
	for _, value := range source.Values("Connection") {
		for _, name := range strings.Split(value, ",") {
			name = strings.ToLower(strings.TrimSpace(name))
			if name != "" {
				result[name] = true
			}
		}
	}
	return result
}

func safeResponseHeader(name string) bool {
	canonical := http.CanonicalHeaderKey(name)
	if strings.HasPrefix(strings.ToLower(canonical), "x-mini-sub2api-") {
		return false
	}
	switch canonical {
	case "Connection", "Content-Length", "Keep-Alive", "Proxy-Authenticate",
		"Proxy-Authorization", "Set-Cookie", "Te", "Trailer", "Transfer-Encoding", "Upgrade":
		return false
	default:
		return true
	}
}
