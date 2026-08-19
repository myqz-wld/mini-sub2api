package protocolv1

import (
	"encoding/json"
	"os"
	"testing"
)

func TestReadinessFixture(t *testing.T) {
	data, err := os.ReadFile("../fixtures/readiness.json")
	if err != nil {
		t.Fatal(err)
	}
	var got Readiness
	if err := json.Unmarshal(data, &got); err != nil {
		t.Fatal(err)
	}
	want := Readiness{
		ProtocolVersion: Version,
		Port:            42123,
		PID:             12345,
		Build: BuildIdentity{
			Name:    "mini-sub2api-core-codex",
			Version: "0.1.0",
			Commit:  "0123456789abcdef0123456789abcdef01234567",
		},
	}
	if got != want {
		t.Fatalf("readiness mismatch: got %#v want %#v", got, want)
	}
}

func TestErrorFixture(t *testing.T) {
	data, err := os.ReadFile("../fixtures/error.json")
	if err != nil {
		t.Fatal(err)
	}
	var got ErrorEnvelope
	if err := json.Unmarshal(data, &got); err != nil {
		t.Fatal(err)
	}
	want := ErrorEnvelope{Error: CoreError{
		Code:      "credential_requires_login",
		Message:   "The selected credential requires sign-in.",
		Retryable: false,
		RequestID: "req_01JEXAMPLE",
	}}
	if got != want {
		t.Fatalf("error mismatch: got %#v want %#v", got, want)
	}
}
