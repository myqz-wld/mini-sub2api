package diagnostics

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"io"
)

// Correlator retains only a random process-local key. It never retains request bytes,
// and equal bodies from different downstream keys receive different identifiers.
type Correlator struct {
	key      [32]byte
	enabled  bool
	Instance string
}

func NewCorrelator() *Correlator { return newCorrelator(rand.Reader) }

func newCorrelator(random io.Reader) *Correlator {
	c := &Correlator{Instance: "unavailable"}
	if _, err := io.ReadFull(random, c.key[:]); err != nil {
		return c
	}
	c.enabled = true
	digest := hmac.New(sha256.New, c.key[:])
	_, _ = digest.Write([]byte("diagnostic-instance-v1"))
	c.Instance = hex.EncodeToString(digest.Sum(nil)[:8])
	return c
}

func (c *Correlator) BodyTag(scope string, body []byte) string {
	if c == nil || !c.enabled {
		return "unavailable"
	}
	digest := hmac.New(sha256.New, c.key[:])
	_, _ = digest.Write([]byte("http-body-v1"))
	var length [8]byte
	binary.BigEndian.PutUint64(length[:], uint64(len(scope)))
	_, _ = digest.Write(length[:])
	_, _ = io.WriteString(digest, scope)
	_, _ = digest.Write(body)
	return hex.EncodeToString(digest.Sum(nil)[:12])
}
