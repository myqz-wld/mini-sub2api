//go:build nativeparity

package integration

import (
	"bytes"
	"context"
	"encoding/binary"
	"fmt"
	"io"
	"net"
	"net/http"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
)

// Only public negotiation capabilities survive parsing. Never retain random/session/key bytes.
type nativeTLSShape struct {
	Version                                           uint16
	Ciphers, Extensions, Groups, Signatures, Versions []uint16
	KeyShares                                         []string
	ALPN                                              []string
}
type nativeTLSProbe struct {
	endpoint string
	shapes   chan nativeTLSShape
}

func newNativeTLSProbe(t *testing.T) nativeTLSProbe {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal("TLS capture listener")
	}
	probe := nativeTLSProbe{endpoint: "https://" + listener.Addr().String(), shapes: make(chan nativeTLSShape, 16)}
	done := make(chan struct{})
	go func() {
		defer close(done)
		defer close(probe.shapes)
		for {
			connection, err := listener.Accept()
			if err != nil {
				return
			}
			_ = connection.SetDeadline(time.Now().Add(2 * time.Second))
			shape, err := readNativeHello(connection)
			_ = connection.Close() // Deliberately never complete TLS or receive auth/application data.
			if err == nil {
				select {
				case probe.shapes <- shape:
				default:
				}
			}
		}
	}()
	t.Cleanup(func() { _ = listener.Close(); <-done })
	return probe
}
func (p nativeTLSProbe) receive(t *testing.T) nativeTLSShape {
	t.Helper()
	select {
	case shape, ok := <-p.shapes:
		if !ok {
			t.Fatal("TLS probe closed without ClientHello")
		}
		return shape
	case <-time.After(8 * time.Second):
		t.Fatal("ClientHello capture deadline")
	}
	return nativeTLSShape{}
}

type helloCursor struct {
	data []byte
	bad  bool
}

func (c *helloCursor) take(n int) []byte {
	if n < 0 || n > len(c.data) {
		c.bad = true
		return nil
	}
	out := c.data[:n]
	c.data = c.data[n:]
	return out
}
func (c *helloCursor) number(n int) int {
	v := 0
	for _, b := range c.take(n) {
		v = v*256 + int(b)
	}
	return v
}
func (c *helloCursor) vector(n int) []byte { return c.take(c.number(n)) }
func helloNumbers(data []byte) []uint16 {
	var out []uint16
	for len(data) >= 2 {
		n := binary.BigEndian.Uint16(data[:2])
		data = data[2:]
		if n&0x0f0f != 0x0a0a {
			out = append(out, n)
		}
	}
	return out
}
func readNativeHello(r io.Reader) (nativeTLSShape, error) {
	var shape nativeTLSShape
	var payload []byte
	for len(payload) < 4 || len(payload) < 4+int(payload[1])<<16+int(payload[2])<<8+int(payload[3]) {
		var header [5]byte
		if _, err := io.ReadFull(r, header[:]); err != nil {
			return shape, fmt.Errorf("TLS header")
		}
		size := int(binary.BigEndian.Uint16(header[3:]))
		if header[0] != 22 || size == 0 || len(payload)+size > 65536 {
			return shape, fmt.Errorf("TLS bound/type")
		}
		part := make([]byte, size)
		if _, err := io.ReadFull(r, part); err != nil {
			return shape, fmt.Errorf("TLS record")
		}
		payload = append(payload, part...)
	}
	c := helloCursor{data: payload}
	if c.number(1) != 1 {
		return shape, fmt.Errorf("not ClientHello")
	}
	length := c.number(3)
	c.data = c.take(length)
	shape.Version = uint16(c.number(2))
	c.take(32)
	c.vector(1)
	shape.Ciphers = helloNumbers(c.vector(2))
	c.vector(1)
	extensions := helloCursor{data: c.vector(2)}
	for len(extensions.data) > 0 && !extensions.bad {
		kind := uint16(extensions.number(2))
		value := helloCursor{data: extensions.vector(2)}
		if kind&0x0f0f == 0x0a0a {
			continue
		}
		shape.Extensions = append(shape.Extensions, kind)
		switch kind {
		case 10:
			shape.Groups = helloNumbers(value.vector(2))
		case 13:
			shape.Signatures = helloNumbers(value.vector(2))
		case 43:
			shape.Versions = helloNumbers(value.vector(1))
		case 16:
			names := helloCursor{data: value.vector(2)}
			for len(names.data) > 0 && !names.bad {
				shape.ALPN = append(shape.ALPN, string(names.vector(1)))
			}
			value.bad = value.bad || names.bad
		case 51:
			keys := helloCursor{data: value.vector(2)}
			for len(keys.data) > 0 && !keys.bad {
				group := uint16(keys.number(2))
				key := keys.vector(2)
				if group&0x0f0f != 0x0a0a {
					shape.KeyShares = append(shape.KeyShares, fmt.Sprintf("%d:%d", group, len(key)))
				}
			}
			value.bad = value.bad || keys.bad
		}
		if value.bad {
			return shape, fmt.Errorf("malformed extension")
		}
	}
	if c.bad || extensions.bad {
		return shape, fmt.Errorf("malformed ClientHello")
	}
	return shape, nil
}
func TestNativeTLSClientHelloParity(t *testing.T) {
	for _, ws := range []bool{false, true} {
		t.Run(fmt.Sprintf("ws=%t", ws), func(t *testing.T) {
			metadata := newNativeCapture(t)
			var baseline nativeTLSShape
			for attempt := 0; attempt < 2; attempt++ {
				native := newNativeTLSProbe(t)
				options := nativeOptions{endpoint: native.endpoint, metadataEndpoint: metadata.server.URL, model: "gpt-5.4", ws: ws, base: "synthetic"}
				client := startNativeClient(t, options)
				thread := client.thread(options)
				client.call("turn/start", map[string]any{"threadId": thread, "input": []any{map[string]any{"type": "text", "text": "synthetic TLS probe"}}})
				shape := native.receive(t)
				slices.Sort(shape.Extensions) // rustls permutes extensions; compare their membership separately.
				if attempt == 0 {
					baseline = shape
				} else if !reflect.DeepEqual(baseline, shape) {
					t.Fatal("native negotiation capabilities vary across fresh connections")
				}
			}
			for _, subscription := range []bool{false, true} {
				t.Run(fmt.Sprintf("subscription=%t", subscription), func(t *testing.T) {
					upstream := newNativeTLSProbe(t)
					gateway := newNativeGateway(t, upstream.endpoint, subscription)
					ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
					defer cancel()
					body := []byte(`{"model":"gpt-5.4","input":"synthetic TLS probe","stream":true}`)
					if ws {
						headers := http.Header{"Authorization": []string{"Bearer " + gateway.secret}}
						connection, _, err := websocket.Dial(ctx, strings.Replace(gateway.server.URL, "http", "ws", 1)+"/v1/responses", &websocket.DialOptions{HTTPHeader: headers})
						if err != nil && subscription {
							t.Fatal("public TLS-probe websocket")
						}
						if connection != nil {
							defer connection.CloseNow()
						}
						if connection != nil && connection.Write(ctx, websocket.MessageText, append([]byte(`{"type":"response.create",`), body[1:]...)) != nil {
							t.Fatal("TLS-probe frame")
						}
					} else {
						request, _ := http.NewRequestWithContext(ctx, http.MethodPost, gateway.server.URL+"/v1/responses", bytes.NewReader(body))
						request.Header.Set("Authorization", "Bearer "+gateway.secret)
						request.Header.Set("Content-Type", "application/json")
						done := make(chan struct{})
						go func() {
							defer close(done)
							response, err := http.DefaultClient.Do(request)
							if err == nil {
								_ = response.Body.Close()
							}
						}()
						defer func() { cancel(); <-done }()
					}
					shape := upstream.receive(t)
					slices.Sort(shape.Extensions)
					if !reflect.DeepEqual(baseline, shape) {
						t.Errorf("ClientHello capability mismatch: native=%+v gateway=%+v", baseline, shape)
					}
				})
			}
			t.Logf("native ClientHello (public capabilities only): %+v", baseline)
		})
	}
}
