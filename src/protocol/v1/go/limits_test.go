package protocolv1

import (
	"encoding/json"
	"os"
	"testing"
)

func TestSharedInferenceLimits(t *testing.T) {
	raw, err := os.ReadFile("../fixtures/limits.json")
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct {
		Override     string
		Valid        bool
		RequestBytes uint64
	}
	if err := json.Unmarshal(raw, &cases); err != nil {
		t.Fatal(err)
	}
	for _, c := range cases {
		v, err := ParseInferenceLimits(c.Override)
		if (err == nil) != c.Valid {
			t.Fatal("limit validity mismatch")
		}
		if c.Valid && v.RequestBytes != c.RequestBytes {
			t.Fatal("limit value mismatch")
		}
	}
}
