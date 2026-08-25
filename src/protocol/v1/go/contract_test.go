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
		Capabilities: Capabilities{ResponsesWebSocket: true},
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
		RequestID: "req_01JEXAMPLE",
		FailureMetadata: FailureMetadata{
			RetryAdvice: RetryNever, Phase: PhaseCredential,
			DeliveryState: DeliveryNotDelivered,
		},
	}}
	if got != want {
		t.Fatalf("error mismatch: got %#v want %#v", got, want)
	}
	if !got.Error.FailureMetadata.Valid() {
		t.Fatal("fixture failure metadata is invalid")
	}
}

func TestRetryAdviceRequiresCoherentDeliveryState(t *testing.T) {
	for _, metadata := range []FailureMetadata{
		{RetryAdvice: RetrySafe, Phase: PhaseUpstreamRequest, DeliveryState: DeliveryPossiblyDelivered},
		{RetryAdvice: RetryAmbiguous, Phase: PhaseUpstreamRequest, DeliveryState: DeliveryDelivered},
		{RetryAdvice: RetryNever, Phase: PhaseUpstreamRequest, DeliveryState: DeliveryPossiblyDelivered},
	} {
		if metadata.Valid() {
			t.Fatalf("accepted inconsistent metadata: %#v", metadata)
		}
	}
}
