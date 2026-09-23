//go:build nativeparity && liveparity

package integration

import (
	"bufio"
	"errors"
	"io"
	"net"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

// The dial callback can only fail locally: even a rejected-request regression cannot contact
// a provider. No live opt-in or real credential is needed for these fixture security checks.
func TestLiveRelayRejectsBeforeDial(t *testing.T) {
	for _, test := range []struct{ name, path, token, account string }{
		{"missing-token", "/v1/responses", "", "synthetic-account"},
		{"wrong-token", "/v1/responses", "wrong", "synthetic-account"},
		{"wrong-account", "/v1/responses", "synthetic-token", "wrong"},
		{"wrong-path", "/arbitrary", "synthetic-token", "synthetic-account"},
		{"query", "/v1/responses?target=example.invalid", "synthetic-token", "synthetic-account"},
	} {
		t.Run(test.name, func(t *testing.T) {
			var dials atomic.Int32
			relay := &liveWireRelay{dial: func() (net.Conn, error) {
				dials.Add(1)
				return nil, errors.New("synthetic denied dial")
			}}
			server, client := net.Pipe()
			defer client.Close()
			_ = client.SetDeadline(time.Now().Add(3 * time.Second))
			done := make(chan struct{})
			go func() {
				defer close(done)
				defer server.Close()
				relay.serve(server, liveAccessCredentials{AccessToken: "synthetic-token", AccountID: "synthetic-account"})
			}()
			_, err := io.WriteString(client, "POST "+test.path+" HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer "+test.token+"\r\nChatgpt-Account-Id: "+test.account+"\r\nContent-Length: 0\r\n\r\n")
			if err != nil {
				t.Fatal("synthetic relay write failed")
			}
			response, err := io.ReadAll(client)
			<-done
			if err != nil || (!strings.HasPrefix(string(response), "HTTP/1.1 403 ") && !strings.HasPrefix(string(response), "HTTP/1.1 404 ")) || dials.Load() != 0 {
				t.Fatal("rejected relay request reached its dial boundary")
			}
		})
	}
}

func TestLiveRelayPreservesWireBytes(t *testing.T) {
	raw := "POST /v1/responses HTTP/1.1\r\nhOsT: localhost\r\nX-Synthetic:  two spaces\r\nAuthorization: Bearer synthetic-token\r\nContent-Length: 0\r\n\r\n"
	want := strings.Replace(strings.Replace(raw, "/v1/responses", "/backend-api/codex/responses", 1), "hOsT: localhost", "hOsT: chatgpt.com", 1)
	if rewriteLiveRelayHeader(raw) != want {
		t.Fatal("relay changed header bytes outside Host/path")
	}
	// A masked text frame and a masked control frame must retain exact payload/mask bytes.
	frames := string([]byte{0x81, 0x82, 1, 2, 3, 4, 'x', 'y', 0x89, 0x80, 4, 3, 2, 1})
	var out strings.Builder
	relay := &liveWireRelay{}
	relay.copyFrames(&out, bufio.NewReader(strings.NewReader(frames)))
	if out.String() != frames || relay.requests.Load() != 1 {
		t.Fatal("relay changed masked frame bytes or request accounting")
	}
	out.Reset()
	relay.requests.Store(12)
	relay.copyFrames(&out, bufio.NewReader(strings.NewReader(frames)))
	if out.Len() != 0 {
		t.Fatal("relay forwarded a request beyond its bound")
	}
}
