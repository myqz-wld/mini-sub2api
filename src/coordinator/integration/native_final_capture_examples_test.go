//go:build nativeparity

package integration

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"
)

// Explicitly requested documentation excerpts only. Raw byte and UUID assertions run separately.
// The default test environment writes no request/response artifacts.
func saveFinalCapture(t *testing.T, packets []nativePacket, wires []nativeWire) {
	t.Helper()
	if os.Getenv("MINI_SUB2API_CAPTURE_EXCERPTS") != "1" {
		return
	}
	redactor := &captureExcerptRedactor{aliases: map[string]string{}}
	var caller, upstream []any
	for _, packet := range packets {
		body := packet.payload
		if packet.headers.Get("Content-Encoding") == "zstd" {
			decoder, err := zstd.NewReader(nil, zstd.WithDecoderMaxMemory(nativeCaptureLimit))
			if err != nil {
				t.Fatal("excerpt decoder unavailable")
			}
			body, err = decoder.DecodeAll(body, nil)
			decoder.Close()
			if err != nil {
				t.Fatal("excerpt decompression failed")
			}
		}
		var value any
		if json.Unmarshal(body, &value) != nil {
			t.Fatal("excerpt caller JSON invalid")
		}
		caller = append(caller, map[string]any{"method": packet.method, "connection": packet.connection, "header_order": packet.headerSpellings, "headers": redactor.headers(packet.headers), "message": redactor.value("", value), "wire_payload_bytes": len(packet.payload), "frame_compressed": packet.compressed})
	}
	for _, wire := range wires {
		upstream = append(upstream, map[string]any{"method": wire.method, "connection": wire.connection, "headers": redactor.headers(wire.headers), "message": redactor.value("", wire.value), "wire_payload_bytes": len(wire.encodedBody)})
	}
	source := "actual client/gateway capture with loopback mock upstream"
	if strings.HasPrefix(t.Name(), "TestLive") {
		source = "actual client request to loopback gateway with real Subscription response; upstream TLS payload not tapped"
	}
	document := map[string]any{"test": t.Name(), "source": source, "representation": "decoded and sanitized excerpts; JSON key order is not original wire order; marked long fields omitted", "caller": caller, "upstream": upstream, "raw_payload_saved": false}
	encoded, err := json.MarshalIndent(document, "", "  ")
	if err != nil || len(encoded) > 2*1024*1024 {
		t.Fatal("capture excerpt encoding or size bound")
	}
	if capturePrivatePath.Match(encoded) || captureJWT.Match(encoded) || captureKey.Match(encoded) {
		t.Fatal("capture excerpt contains unsanitized private material")
	}
	path := filepath.Join(nativeRepository(), ".ref/plans/plan17/captures")
	if os.MkdirAll(path, 0700) != nil {
		t.Fatal("create bounded capture excerpt directory")
	}
	digest := sha256.Sum256([]byte(t.Name()))
	if os.WriteFile(filepath.Join(path, hex.EncodeToString(digest[:8])+".json"), append(encoded, '\n'), 0600) != nil {
		t.Fatal("write sanitized capture excerpt")
	}
}

var capturePrivatePath = regexp.MustCompile(`/(?:Users|home|private|var|tmp)/[^\s"'<>` + "`" + `,;\}\]]*`)
var captureJWT = regexp.MustCompile(`eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+`)
var captureKey = regexp.MustCompile(`\b(?:sk-|msk_|msk-|ms2a_)[A-Za-z0-9_-]{12,}`)
var captureUUID = regexp.MustCompile(`[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}`)

type captureExcerptRedactor struct{ aliases map[string]string }

func (r *captureExcerptRedactor) alias(kind, value string) string {
	key := kind + ":" + value
	if alias, exists := r.aliases[key]; exists {
		return alias
	}
	alias := fmt.Sprintf("<%s-%d>", kind, len(r.aliases)+1)
	r.aliases[key] = alias
	return alias
}

func (r *captureExcerptRedactor) text(value string) string {
	value = captureJWT.ReplaceAllString(value, "<credential>")
	value = captureKey.ReplaceAllString(value, "<credential>")
	value = capturePrivatePath.ReplaceAllStringFunc(value, func(path string) string { return r.alias("client-path", path) })
	return captureUUID.ReplaceAllStringFunc(value, func(id string) string { return r.alias("uuid", id) })
}

func (r *captureExcerptRedactor) headers(headers http.Header) map[string]any {
	out := map[string]any{}
	for key, values := range headers {
		var encoded []any
		for _, value := range values {
			switch strings.ToLower(key) {
			case "authorization", "proxy-authorization", "cookie", "set-cookie", "chatgpt-account-id":
				encoded = append(encoded, "<credential>")
			case "x-codex-turn-state":
				encoded = append(encoded, r.alias("routing-state", value))
			case "sec-websocket-key", "sec-websocket-accept":
				encoded = append(encoded, "<websocket-handshake-nonce>")
			case "host":
				encoded = append(encoded, "<loopback-endpoint>")
			default:
				encoded = append(encoded, r.value(strings.ToLower(key), value))
			}
		}
		out[key] = encoded
	}
	return out
}

func (r *captureExcerptRedactor) value(key string, raw any) any {
	switch value := raw.(type) {
	case map[string]any:
		out := map[string]any{}
		for key, value := range value {
			out[r.text(key)] = r.value(key, value)
		}
		return out
	case []any:
		out := make([]any, len(value))
		for i, value := range value {
			out[i] = r.value(key, value)
		}
		return out
	case string:
		if key == "x-codex-turn-metadata" {
			var metadata any
			if json.Unmarshal([]byte(value), &metadata) == nil {
				encoded, _ := json.Marshal(r.value("", metadata))
				return string(encoded)
			}
		}
		if key == "encrypted_content" {
			return fmt.Sprintf("<encrypted-content omitted: %d bytes>", len(value))
		}
		if key == "x-codex-turn-state" {
			return r.alias("routing-state", value)
		}
		if key == "id" || key == "previous_response_id" || key == "call_id" || key == "stream_id" {
			return r.alias("id", value)
		}
		if (key == "instructions" || key == "description" || key == "text") && len(value) > 1200 {
			digest := sha256.Sum256([]byte(value))
			return fmt.Sprintf("<long field omitted: %d bytes, sha256=%x>", len(value), digest)
		}
		return r.text(value)
	default:
		return raw
	}
}

func TestFinalCaptureExcerptPrivacy(t *testing.T) {
	r := &captureExcerptRedactor{aliases: map[string]string{}}
	value := map[string]any{"client_metadata": map[string]any{"x-codex-turn-metadata": `{"session_id":"01900000-0000-7000-8000-000000000001","workspaces":{"/Users/synthetic/private-work":{}}}`}, "instructions": "never store eyJfYWxn.X2JvZHk.X3NpZw or sk-syntheticsecret0123456789 or ms2a_syntheticsecret0123456789", "input": []any{map[string]any{"id": "msg_synthetic", "call_id": "call_synthetic", "encrypted_content": "private opaque value", "content": []any{map[string]any{"text": "CWD /private/var/folders/synthetic/project\nTimezone: Etc/UTC"}}}}, "previous_response_id": "msg_synthetic"}
	projected := r.value("", value).(map[string]any)
	encoded, _ := json.Marshal(projected)
	for _, forbidden := range []string{"private opaque value", "syntheticsecret", "eyJfYWxn", "/Users/", "/private/", "01900000-0000"} {
		if strings.Contains(string(encoded), forbidden) {
			t.Fatal("private field survived excerpt sanitization")
		}
	}
	input := projected["input"].([]any)
	if input[0].(map[string]any)["id"] != projected["previous_response_id"] {
		t.Fatal("excerpt ID correlation unstable")
	}
	headers := r.headers(http.Header{"Authorization": {"Bearer unstructured-sensitive-value"}, "Chatgpt-Account-Id": {"private-account"}})
	encoded, _ = json.Marshal(headers)
	if strings.Contains(string(encoded), "sensitive-value") || strings.Contains(string(encoded), "private-account") {
		t.Fatal("credential header survived excerpt sanitization")
	}
}
