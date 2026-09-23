//go:build nativeparity

package integration

import (
	"bufio"
	"bytes"
	"compress/flate"
	"encoding/binary"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
)

const nativeCaptureLimit = 64 * 1024 * 1024

type nativeTap struct {
	mu          sync.Mutex
	connections []*nativeTapConn
	overflow    bool
}
type nativeTapConn struct {
	net.Conn
	mu       sync.Mutex
	received []byte
	overflow bool
}
type nativeTapListener struct {
	net.Listener
	tap *nativeTap
}
type nativePacket struct {
	method          string
	headers         http.Header
	headerNames     []string
	headerSpellings []string
	payload         []byte
	compressed      bool
	connection      int
}

func (l *nativeTapListener) Accept() (net.Conn, error) {
	connection, err := l.Listener.Accept()
	if err != nil {
		return nil, err
	}
	wrapped := &nativeTapConn{Conn: connection}
	l.tap.mu.Lock()
	if len(l.tap.connections) >= 64 {
		l.tap.overflow = true
		l.tap.mu.Unlock()
		_ = connection.Close()
		return nil, fmt.Errorf("native connection capture capacity")
	}
	l.tap.connections = append(l.tap.connections, wrapped)
	l.tap.mu.Unlock()
	return wrapped, nil
}
func (c *nativeTapConn) Read(buffer []byte) (int, error) {
	n, err := c.Conn.Read(buffer)
	c.mu.Lock()
	if len(c.received)+n > nativeCaptureLimit {
		c.overflow = true
	} else {
		c.received = append(c.received, buffer[:n]...)
	}
	c.mu.Unlock()
	return n, err
}
func newNativeTappedServer(t *testing.T, handler http.Handler) (*httptest.Server, *nativeTap) {
	t.Helper()
	tap := &nativeTap{}
	server := httptest.NewUnstartedServer(handler)
	server.Listener = &nativeTapListener{Listener: server.Listener, tap: tap}
	server.Start()
	t.Cleanup(server.Close)
	assertLoopbackURL(t, server.URL)
	return server, tap
}
func (tap *nativeTap) packets(t *testing.T) []nativePacket {
	t.Helper()
	tap.mu.Lock()
	connections := append([]*nativeTapConn(nil), tap.connections...)
	overflow := tap.overflow
	tap.mu.Unlock()
	if overflow {
		t.Fatal("native tap connection capacity exceeded")
	}
	var packets []nativePacket
	for number, connection := range connections {
		connection.mu.Lock()
		raw := append([]byte(nil), connection.received...)
		overflow := connection.overflow
		connection.mu.Unlock()
		if overflow {
			t.Fatal("native wire capture capacity exceeded")
		}
		source := bytes.NewReader(raw)
		reader := bufio.NewReader(source)
		for {
			offset := len(raw) - source.Len() - reader.Buffered()
			headerEnd := bytes.Index(raw[offset:], []byte("\r\n\r\n"))
			if headerEnd < 0 {
				break
			}
			names := wireHeaderNames(raw[offset : offset+headerEnd])
			spellings := append([]string(nil), names...)
			for i := range names {
				names[i] = strings.ToLower(names[i])
			}
			request, err := http.ReadRequest(reader)
			if err != nil {
				t.Fatal("native wire HTTP framing invalid")
			}
			if strings.EqualFold(request.Header.Get("Upgrade"), "websocket") {
				frames, err := io.ReadAll(reader)
				if err != nil {
					t.Fatal("native wire frame read")
				}
				messages, err := decodeNativeFrames(frames)
				if err != nil {
					t.Fatal("native wire WebSocket framing invalid")
				}
				for _, message := range messages {
					message.method = http.MethodGet
					message.headers = request.Header.Clone()
					message.headerNames = names
					message.headerSpellings = spellings
					message.connection = number + 1
					packets = append(packets, message)
				}
				break
			}
			body, err := io.ReadAll(io.LimitReader(request.Body, nativeCaptureLimit+1))
			_ = request.Body.Close()
			if err != nil || len(body) > nativeCaptureLimit {
				t.Fatal("native HTTP capture body bound")
			}
			if request.URL.Path == "/v1/responses" || request.URL.Path == "/backend-api/codex/responses" {
				packets = append(packets, nativePacket{method: request.Method, headers: request.Header.Clone(), headerNames: names, headerSpellings: spellings, payload: body, connection: number + 1})
			}
		}
	}
	return packets
}
func wireHeaderNames(header []byte) []string {
	lines := strings.Split(string(header), "\r\n")
	var names []string
	for _, line := range lines[1:] {
		if name, _, ok := strings.Cut(line, ":"); ok {
			names = append(names, name)
		}
	}
	return names
}
func decodeNativeFrames(frames []byte) ([]nativePacket, error) {
	var messages []nativePacket
	var payload []byte
	var dictionary []byte
	compressed := false
	fragmented := false
	for len(frames) > 0 {
		if len(frames) < 2 {
			break
		}
		first, second := frames[0], frames[1]
		length := uint64(second & 127)
		offset := 2
		if length == 126 {
			if len(frames) < 4 {
				break
			}
			length = uint64(binary.BigEndian.Uint16(frames[2:4]))
			offset = 4
		} else if length == 127 {
			if len(frames) < 10 {
				break
			}
			length = binary.BigEndian.Uint64(frames[2:10])
			offset = 10
		}
		if length > nativeCaptureLimit {
			return nil, fmt.Errorf("frame bound")
		}
		masked := second&128 != 0
		if !masked || first&0x30 != 0 {
			return nil, fmt.Errorf("client mask/reserved bits")
		}
		var mask []byte
		if masked {
			if len(frames) < offset+4 {
				break
			}
			mask = frames[offset : offset+4]
			offset += 4
		}
		if uint64(len(frames)-offset) < length {
			break
		}
		data := append([]byte(nil), frames[offset:offset+int(length)]...)
		frames = frames[offset+int(length):]
		if masked {
			for i := range data {
				data[i] ^= mask[i%4]
			}
		}
		opcode := first & 15
		if opcode >= 8 {
			if opcode > 10 || first&128 == 0 || first&64 != 0 || length > 125 {
				return nil, fmt.Errorf("invalid control frame")
			}
			continue
		}
		if opcode == 1 {
			if fragmented {
				return nil, fmt.Errorf("overlapping data message")
			}
			payload = nil
			compressed = first&64 != 0
			fragmented = true
		} else if opcode != 0 || !fragmented {
			return nil, fmt.Errorf("unsupported data frame")
		}
		if opcode == 0 && first&64 != 0 {
			return nil, fmt.Errorf("compressed continuation bit")
		}
		if len(payload)+len(data) > nativeCaptureLimit {
			return nil, fmt.Errorf("message bound")
		}
		payload = append(payload, data...)
		if first&128 == 0 {
			continue
		}
		fragmented = false
		if compressed {
			// RFC 7692 removes the sync-flush tail. Add it and a final empty block for a bounded reader.
			encoded := append(append([]byte(nil), payload...), 0, 0, 255, 255, 1, 0, 0, 255, 255)
			decoder := flate.NewReaderDict(bytes.NewReader(encoded), dictionary)
			decoded, err := io.ReadAll(io.LimitReader(decoder, nativeCaptureLimit+1))
			_ = decoder.Close()
			if err != nil || len(decoded) > nativeCaptureLimit {
				return nil, fmt.Errorf("deflate bound or framing")
			}
			payload = decoded
			dictionary = append(dictionary, payload...)
			if len(dictionary) > 32768 {
				dictionary = append([]byte(nil), dictionary[len(dictionary)-32768:]...)
			}
		}
		messages = append(messages, nativePacket{payload: append([]byte(nil), payload...), compressed: compressed})
		payload = nil
	}
	if fragmented {
		return nil, fmt.Errorf("incomplete data message")
	}
	return messages, nil
}
