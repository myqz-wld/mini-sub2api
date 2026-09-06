//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"reflect"
	"slices"
	"strings"
	"testing"
)

func assertNativeWireParity(t *testing.T, caller, sent nativePacket, wire nativeWire, subscription bool) {
	t.Helper()
	if caller.method != sent.method {
		t.Fatal("gateway changed native transport")
	}
	for _, name := range []string{"User-Agent", "Originator", "Accept", "Content-Type", "Content-Encoding", "OpenAI-Beta", "X-OpenAI-Internal-Codex-Responses-Lite"} {
		if caller.headers.Get(name) != sent.headers.Get(name) {
			t.Errorf("native header value differs: %s", name)
		}
	}
	// The gateway intentionally pins a complete identity triplet; native app-server uses UA
	// and Originator without a separate Version header on these requests.
	if subscription {
		if sent.headers.Get("Version") != "0.153.4" {
			t.Fatal("pinned Version header")
		}
	} else if caller.headers.Get("Version") != sent.headers.Get("Version") {
		t.Fatal("API-key Version header changed")
	}
	if sent.headers.Get("Authorization") == caller.headers.Get("Authorization") {
		t.Fatal("distribution key crossed upstream boundary")
	}
	if sent.method == http.MethodGet {
		for _, name := range []string{"Upgrade", "Connection", "Sec-WebSocket-Version", "Sec-WebSocket-Extensions"} {
			if caller.headers.Get(name) != sent.headers.Get(name) {
				t.Errorf("native WS negotiation differs: %s", name)
			}
		}
		key, err := base64.StdEncoding.DecodeString(sent.headers.Get("Sec-WebSocket-Key"))
		if err != nil || len(key) != 16 || sent.headers.Get("Sec-WebSocket-Key") == caller.headers.Get("Sec-WebSocket-Key") {
			t.Fatal("upstream handshake nonce not fresh/valid")
		}
		if !bytes.Equal(sent.payload, wire.body) {
			t.Fatal("raw WS deflate/frame decoding differs from server library")
		}
		if len(sent.payload) > 4096 && caller.compressed && !sent.compressed {
			t.Fatal("gateway omitted negotiated compression on large native frame")
		}
		if wire.value["type"] != "response.create" {
			t.Fatal("native upstream WS message type")
		}
	} else {
		if len(sent.payload) < 4 || !bytes.Equal(sent.payload[:4], []byte{0x28, 0xb5, 0x2f, 0xfd}) {
			t.Fatal("native HTTP zstd magic")
		}
		if !bytes.Equal(sent.payload, wire.encodedBody) {
			t.Fatal("HTTP raw framing lost encoded bytes")
		}
	}
	// Persist only names and ordering, never arbitrary header values. Both raw and common order
	// are observable facts; optional gateway identity headers can legitimately add positions.
	if !reflect.DeepEqual(caller.headerNames, sent.headerNames) {
		added := []string{}
		removed := []string{}
		for _, name := range sent.headerNames {
			if !slices.Contains(caller.headerNames, name) {
				added = append(added, name)
			}
		}
		for _, name := range caller.headerNames {
			if !slices.Contains(sent.headerNames, name) {
				removed = append(removed, name)
			}
		}
		t.Logf("wire header order: native=%v gateway=%v added=%v removed=%v", caller.headerNames, sent.headerNames, added, removed)
	}
	if !subscription {
		for _, name := range []string{"Session-Id", "Thread-Id", "X-Client-Request-Id", "X-Codex-Turn-Metadata", "X-Codex-Window-Id"} {
			if !reflect.DeepEqual(caller.headers.Values(name), sent.headers.Values(name)) {
				t.Errorf("API-key native identity header changed: %s", name)
			}
		}
		return
	}
	common := func(names, other []string) []string {
		out := []string{}
		for _, name := range names {
			if slices.Contains(other, name) {
				out = append(out, name)
			}
		}
		return out
	}
	if !reflect.DeepEqual(common(caller.headerNames, sent.headerNames), common(sent.headerNames, caller.headerNames)) {
		t.Fatal("Subscription common native header order changed")
	}
	for i, name := range caller.headerNames {
		if j := slices.Index(sent.headerNames, name); j >= 0 && caller.headerSpellings[i] != sent.headerSpellings[j] {
			t.Errorf("Subscription header spelling differs: %s", name)
		}
	}
	metadata, _ := wire.value["client_metadata"].(map[string]any)
	for _, field := range []string{"session_id", "thread_id", "turn_id"} {
		id, _ := metadata[field].(string)
		if field == "turn_id" && wire.value["generate"] == false {
			if id != "" {
				t.Fatal("prewarm invented an active turn")
			}
			continue
		}
		if !isUUIDVersion(id, '7') {
			t.Errorf("projected native identity is not UUIDv7: %s", field)
		}
	}
	if wire.method == http.MethodPost && sent.headers.Get("Session-Id") != metadata["session_id"] {
		t.Fatal("HTTP session header/body diverged")
	}
	var turn map[string]any
	raw, _ := metadata["x-codex-turn-metadata"].(string)
	if json.Unmarshal([]byte(raw), &turn) != nil {
		t.Fatal("native turn metadata JSON")
	}
	for _, field := range []string{"session_id", "thread_id", "turn_id"} {
		if turn[field] != metadata[field] {
			t.Errorf("native metadata identity relationship differs: %s", field)
		}
	}
	if window, ok := metadata["x-codex-window-id"].(string); ok && !strings.HasPrefix(window, metadata["thread_id"].(string)+":") {
		t.Fatal("native window/thread relationship")
	}
}
