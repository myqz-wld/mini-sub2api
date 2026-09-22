package diagnostics

import (
	"bytes"
	"fmt"
	"testing"
)

func TestBodyCorrelationIsScopedEphemeralAndFailsClosed(t *testing.T) {
	a := newCorrelator(bytes.NewReader(bytes.Repeat([]byte{1}, 32)))
	b := newCorrelator(bytes.NewReader(bytes.Repeat([]byte{2}, 32)))
	body := []byte("synthetic-private-body")
	tag := a.BodyTag("key-a", body)
	if tag != a.BodyTag("key-a", body) || len(tag) != 24 {
		t.Fatal("same request not correlated")
	}
	if tag == a.BodyTag("key-b", body) || tag == b.BodyTag("key-a", body) || tag == a.BodyTag("key-a", []byte("different")) {
		t.Fatal("correlation escaped request/key/instance scope")
	}
	if failed := newCorrelator(bytes.NewReader(nil)); failed.BodyTag("key-a", body) != "unavailable" || failed.Instance != "unavailable" {
		t.Fatal("entropy failure must not fall back to a predictable hash")
	}
}

func BenchmarkBodyTag(b *testing.B) {
	for _, size := range []int{1024, 1 << 20, 32 << 20} {
		b.Run(fmt.Sprint(size), func(b *testing.B) {
			body := bytes.Repeat([]byte{'x'}, size)
			correlator := NewCorrelator()
			b.SetBytes(int64(size))
			b.ReportAllocs()
			b.ResetTimer()
			for b.Loop() {
				_ = correlator.BodyTag("synthetic-key", body)
			}
		})
	}
}
