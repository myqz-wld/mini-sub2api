package storage

import (
	"crypto/sha256"
	"encoding/base64"
	"strings"
	"testing"
)

func TestPseudonymScopeIsStatelessStableAndKeyScoped(t *testing.T) {
	firstHash := sha256.Sum256([]byte("ms2a_first-test-secret"))
	secondHash := sha256.Sum256([]byte("ms2a_second-test-secret"))
	first := pseudonymScope(firstHash)

	if first != pseudonymScope(firstHash) {
		t.Fatal("same downstream key hash produced a different pseudonym scope")
	}
	if first == pseudonymScope(secondHash) {
		t.Fatal("different downstream key hashes produced the same pseudonym scope")
	}
	if !strings.HasPrefix(first, "psn_") || len(first) != 47 {
		t.Fatalf("scope shape = %q", first)
	}
	decoded, err := base64.RawURLEncoding.DecodeString(strings.TrimPrefix(first, "psn_"))
	if err != nil || len(decoded) != sha256.Size {
		t.Fatalf("scope payload is not one SHA-256 digest: %q, %v", first, err)
	}
}
