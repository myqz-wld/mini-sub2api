//go:build nativeparity

package integration

import (
	"bytes"
	"compress/flate"
	"encoding/binary"
	"testing"
)

func maskedNativeFrame(first byte, data []byte) []byte {
	result := []byte{first, 0x80}
	switch {
	case len(data) < 126:
		result[1] |= byte(len(data))
	case len(data) < 65536:
		result[1] |= 126
		result = binary.BigEndian.AppendUint16(result, uint16(len(data)))
	default:
		result[1] |= 127
		result = binary.BigEndian.AppendUint64(result, uint64(len(data)))
	}
	key := []byte{1, 2, 3, 4}
	result = append(result, key...)
	for i, b := range data {
		result = append(result, b^key[i%4])
	}
	return result
}
func TestNativeWireParserBoundaries(t *testing.T) {
	payload := bytes.Repeat([]byte("synthetic-frame-"), 5000)
	for _, length := range []int{0, 125, 126, 65535, 65536} {
		frames := maskedNativeFrame(1, payload[:length/2])
		frames = append(frames, maskedNativeFrame(0x89, []byte("ping"))...)
		frames = append(frames, maskedNativeFrame(0x80, payload[length/2:length])...)
		messages, err := decodeNativeFrames(frames)
		if err != nil || len(messages) != 1 || !bytes.Equal(messages[0].payload, payload[:length]) {
			t.Fatal("fragment/extended length/control decoding")
		}
	}
	var compressed bytes.Buffer
	encoder, _ := flate.NewWriter(&compressed, flate.DefaultCompression)
	var frames []byte
	for i := 0; i < 2; i++ {
		start := compressed.Len()
		_, _ = encoder.Write(payload)
		_ = encoder.Flush()
		block := compressed.Bytes()[start : compressed.Len()-4]
		frames = append(frames, maskedNativeFrame(0xc1, block)...)
	}
	_ = encoder.Close()
	messages, err := decodeNativeFrames(frames)
	if err != nil || len(messages) != 2 {
		t.Fatal("context takeover decode")
	}
	for _, message := range messages {
		if !message.compressed || !bytes.Equal(message.payload, payload) {
			t.Fatal("context takeover payload mismatch")
		}
	}
	for _, frame := range [][]byte{
		{0x81, 0},
		maskedNativeFrame(0x89, bytes.Repeat([]byte("x"), 126)),
		maskedNativeFrame(0x09, []byte("fragmented control")),
		maskedNativeFrame(0x82, []byte("unsupported binary")),
		maskedNativeFrame(0x01, []byte("unfinished text")),
		{0x81, 0xff, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff},
	} {
		if _, err := decodeNativeFrames(frame); err == nil {
			t.Fatal("malformed frame accepted by capture decoder")
		}
	}
}
func TestNativeHelloParserRejectsMalformedInput(t *testing.T) {
	for _, input := range [][]byte{nil, {22, 3, 3, 0, 0}, {23, 3, 3, 0, 4, 1, 0, 0, 0}, {22, 3, 3, 0, 4, 1, 0, 0, 0}} {
		if _, err := readNativeHello(bytes.NewReader(input)); err == nil {
			t.Fatal("malformed ClientHello accepted")
		}
	}
}
