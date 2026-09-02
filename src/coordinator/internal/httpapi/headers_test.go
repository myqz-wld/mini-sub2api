package httpapi

import (
	"net/http"
	"strings"
	"testing"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestResponseHeaderBoundaryIsExactAndDefaultDeny(t *testing.T) {
	source := make(http.Header)
	source.Set("Cache-Control", "no-store")
	source.Set("X-Request-Id", "provider-raw")
	source.Set("X-Codex-Turn-State", "opaque")
	source.Set("X-Codex-Installation-Id", "must-not-cross")
	source.Set("X-Unrecognized-Provider-Extension", "must-not-cross")
	source.Set(protocolv1.ProviderRequestIDHeader, "provider-private")
	source.Set(protocolv1.CoreTTFBHeader, "6")
	destination := make(http.Header)
	ttfb := copyResponseHeaders(destination, source, "req_gateway")
	if destination.Get("Cache-Control") != "no-store" ||
		destination.Get("X-Request-Id") != "req_gateway" ||
		destination.Get("X-Codex-Turn-State") != "opaque" ||
		destination.Get("Server-Timing") != "upstream_ttfb;dur=6" ||
		ttfb == nil || ttfb.Milliseconds() != 6 {
		t.Fatalf("safe response headers = %#v / %v", destination, ttfb)
	}
	for _, name := range []string{
		"X-Codex-Installation-Id",
		"X-Unrecognized-Provider-Extension",
		protocolv1.ProviderRequestIDHeader,
		protocolv1.CoreTTFBHeader,
	} {
		if destination.Get(name) != "" {
			t.Fatalf("private header %s crossed: %#v", name, destination)
		}
	}
}

func TestProviderRequestIDValidationMatchesThePrivateProtocolBound(t *testing.T) {
	for _, value := range []string{"", "contains space", "contains\tcontrol", strings.Repeat("x", 513)} {
		if validProviderRequestID(value) {
			t.Fatalf("invalid provider request ID accepted: %q", value)
		}
	}
	maximum := strings.Repeat("x", 512)
	if !validProviderRequestID(maximum) {
		t.Fatal("512-byte visible ASCII provider request ID was rejected")
	}
	headers := make(http.Header)
	headers.Set(protocolv1.ProviderRequestIDHeader, maximum)
	got := providerRequestIDFromHeaders(headers)
	if got == nil || *got != maximum {
		t.Fatalf("provider request ID = %#v", got)
	}
}
