//go:build nativeparity && liveparity

package integration

import (
	"bufio"
	"bytes"
	"crypto/subtle"
	"crypto/tls"
	"encoding/binary"
	"io"
	"net"
	"net/http"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// A deliberately explicit live-only relay. It preserves JSON/zstd and raw WS frames, header
// order/casing and HTTP response bytes. Only destination/path/Host are replaced. The caller must
// already hold the same live access credential; this is not an unauthenticated credential relay.
// Its upstream TLS is Go TLS, so this fixture never claims end-to-end native TLS equivalence.
type liveWireRelay struct {
	endpoint    string
	tap         *nativeTap
	requests    atomic.Int32
	mu          sync.Mutex
	connections map[net.Conn]bool
	closed      bool
	wg          sync.WaitGroup
	httpOnly    bool
	dial        func() (net.Conn, error)
}

func newLiveWireRelay(t *testing.T, httpOnly bool) *liveWireRelay {
	t.Helper()
	auth := readLiveAccessCredentials(t)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal("live relay listener")
	}
	r := &liveWireRelay{endpoint: "http://" + listener.Addr().String(), tap: &nativeTap{}, connections: map[net.Conn]bool{}, httpOnly: httpOnly}
	r.dial = func() (net.Conn, error) {
		return tls.DialWithDialer(&net.Dialer{Timeout: 10 * time.Second}, "tcp", "chatgpt.com:443", &tls.Config{ServerName: "chatgpt.com", MinVersion: tls.VersionTLS12, NextProtos: []string{"http/1.1"}})
	}
	tapped := &nativeTapListener{Listener: listener, tap: r.tap}
	r.wg.Add(1)
	go func() {
		defer r.wg.Done()
		for {
			connection, err := tapped.Accept()
			if err != nil {
				return
			}
			r.mu.Lock()
			if r.closed {
				r.mu.Unlock()
				_ = connection.Close()
				return
			}
			r.connections[connection] = true
			r.wg.Add(1)
			r.mu.Unlock()
			go func() {
				defer r.wg.Done()
				defer connection.Close()
				defer func() { r.mu.Lock(); delete(r.connections, connection); r.mu.Unlock() }()
				r.serve(connection, auth)
			}()
		}
	}()
	t.Cleanup(func() {
		r.mu.Lock()
		r.closed = true
		_ = listener.Close()
		for connection := range r.connections {
			_ = connection.Close()
		}
		r.mu.Unlock()
		r.wg.Wait()
	})
	return r
}

func (r *liveWireRelay) serve(client net.Conn, auth liveAccessCredentials) {
	_ = client.SetDeadline(time.Now().Add(3 * time.Minute))
	source := bufio.NewReader(client)
	for {
		var head bytes.Buffer
		for {
			line, err := source.ReadString('\n')
			if err != nil {
				return
			}
			head.WriteString(line)
			if head.Len() > 64*1024 {
				return
			}
			if line == "\r\n" {
				break
			}
		}
		request, err := http.ReadRequest(bufio.NewReader(bytes.NewReader(head.Bytes())))
		if err != nil || (request.URL.Path != "/v1/responses" && request.URL.Path != "/backend-api/codex/responses") || request.URL.RawQuery != "" {
			_, _ = io.WriteString(client, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
			return
		}
		if subtle.ConstantTimeCompare([]byte(request.Header.Get("Authorization")), []byte("Bearer "+auth.AccessToken)) != 1 || request.Header.Get("Chatgpt-Account-Id") != auth.AccountID {
			_, _ = io.WriteString(client, "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
			return
		}
		ws := request.Method == http.MethodGet && strings.EqualFold(request.Header.Get("Upgrade"), "websocket")
		if r.httpOnly && ws {
			_, _ = io.WriteString(client, "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
			return
		}
		if (!ws && request.Method != http.MethodPost) || len(request.TransferEncoding) != 0 || request.ContentLength < 0 || request.ContentLength > nativeCaptureLimit {
			return
		}
		if !ws && r.requests.Add(1) > 12 {
			return
		}
		remote, err := r.dial()
		if err != nil {
			return
		}
		_ = remote.SetDeadline(time.Now().Add(2 * time.Minute))
		// Tie this upstream connection to its owned downstream, including fixture cancellation.
		r.mu.Lock()
		if r.closed {
			r.mu.Unlock()
			_ = remote.Close()
			return
		}
		r.connections[remote] = true
		r.mu.Unlock()
		forward := rewriteLiveRelayHeader(head.String())
		_, err = io.WriteString(remote, forward)
		if err == nil && !ws {
			_, err = io.CopyN(remote, source, request.ContentLength)
		}
		if err != nil {
			_ = remote.Close()
			r.remove(remote)
			return
		}
		upstream := bufio.NewReader(io.TeeReader(remote, client))
		response, err := http.ReadResponse(upstream, request)
		if err == nil && ws && response.StatusCode == http.StatusSwitchingProtocols {
			done := make(chan struct{})
			go func() { r.copyFrames(remote, source); _ = remote.Close(); close(done) }()
			_, _ = io.Copy(io.Discard, upstream)
			_ = client.Close()
			_ = remote.Close()
			<-done
			r.remove(remote)
			return
		}
		if err == nil {
			_, err = io.Copy(io.Discard, response.Body)
			_ = response.Body.Close()
		}
		_ = remote.Close()
		r.remove(remote)
		if err != nil || ws || request.Close || response.Close {
			return
		}
	}
}

func (r *liveWireRelay) remove(connection net.Conn) {
	r.mu.Lock()
	delete(r.connections, connection)
	r.mu.Unlock()
}

func rewriteLiveRelayHeader(raw string) string {
	lines := strings.Split(raw, "\r\n")
	request := strings.SplitN(lines[0], " ", 3)
	request[1] = "/backend-api/codex/responses"
	lines[0] = strings.Join(request, " ")
	for i, line := range lines[1:] {
		name, _, found := strings.Cut(line, ":")
		if !found {
			continue
		}
		value := ""
		switch strings.ToLower(name) {
		case "host":
			value = "chatgpt.com"
		default:
			continue
		}
		lines[i+1] = name + ": " + value
	}
	return strings.Join(lines, "\r\n")
}

func (r *liveWireRelay) copyFrames(destination io.Writer, source *bufio.Reader) {
	for {
		head := make([]byte, 2)
		if _, err := io.ReadFull(source, head); err != nil {
			return
		}
		length := uint64(head[1] & 127)
		extra := 0
		if length == 126 {
			extra = 2
		} else if length == 127 {
			extra = 8
		}
		if extra > 0 {
			more := make([]byte, extra)
			if _, err := io.ReadFull(source, more); err != nil {
				return
			}
			head = append(head, more...)
			if extra == 2 {
				length = uint64(binary.BigEndian.Uint16(more))
			} else {
				length = binary.BigEndian.Uint64(more)
			}
		}
		if head[1]&128 == 0 || length > nativeCaptureLimit {
			return
		}
		mask := make([]byte, 4)
		if _, err := io.ReadFull(source, mask); err != nil {
			return
		}
		head = append(head, mask...)
		if head[0]&15 == 1 && r.requests.Add(1) > 12 {
			return
		}
		if _, err := destination.Write(head); err != nil {
			return
		}
		if _, err := io.CopyN(destination, source, int64(length)); err != nil {
			return
		}
	}
}

func (r *liveWireRelay) report(t *testing.T) {
	t.Helper()
	t.Logf("LIVE_RELAY forwarded_requests=%d", r.requests.Load())
}
