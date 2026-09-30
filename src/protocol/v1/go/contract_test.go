package protocolv1

import (
	"encoding/json"
	"os"
	"reflect"
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

func TestUsageWindowErrorFixture(t *testing.T) {
	data, err := os.ReadFile("../fixtures/usage_window_error.json")
	if err != nil {
		t.Fatal(err)
	}
	var got ErrorEnvelope
	if err := json.Unmarshal(data, &got); err != nil {
		t.Fatal(err)
	}
	if got.Error.Code != "usage_limit_reached" || got.Error.LimitWindowMinutes == nil || *got.Error.LimitWindowMinutes != 300 || !got.Error.FailureMetadata.Valid() {
		t.Fatal("usage-window fixture contract mismatch")
	}
	var original, encodedValue any
	if err := json.Unmarshal(data, &original); err != nil {
		t.Fatal(err)
	}
	encoded, err := json.Marshal(got)
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(encoded, &encodedValue); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(original, encodedValue) {
		t.Fatal("usage-window fixture did not round trip")
	}
	for _, minutes := range []uint16{0, 65535} {
		got.Error.LimitWindowMinutes = &minutes
		encoded, err := json.Marshal(got)
		if err != nil {
			t.Fatal(err)
		}
		var decoded ErrorEnvelope
		if err := json.Unmarshal(encoded, &decoded); err != nil {
			t.Fatal(err)
		}
		if decoded.Error.LimitWindowMinutes == nil || *decoded.Error.LimitWindowMinutes != minutes {
			t.Fatal("usage-window bound changed")
		}
	}
	for _, invalid := range []string{"-1", "65536", "1.5", `"300"`} {
		var decoded ErrorEnvelope
		if json.Unmarshal([]byte(`{"error":{"limitWindowMinutes":`+invalid+`}}`), &decoded) == nil {
			t.Fatal("accepted invalid typed usage window")
		}
	}
}

func TestProviderRequestIDControlShape(t *testing.T) {
	control := ProviderRequestIDControl{
		Type: ProviderRequestIDEventType, ProviderRequestID: "provider-visible-ascii",
	}
	encoded, err := json.Marshal(control)
	if err != nil {
		t.Fatal(err)
	}
	if string(encoded) != `{"type":"mini_sub2api.provider_request_id","providerRequestId":"provider-visible-ascii"}` {
		t.Fatalf("control JSON = %s", encoded)
	}
}

func TestResponseTerminalHeaderContract(t *testing.T) {
	if ResponseTerminalHeader != "X-Mini-Sub2Api-Response-Terminal" {
		t.Fatalf("terminal header = %q", ResponseTerminalHeader)
	}
	got := []string{
		ResponseTerminalCompleted, ResponseTerminalFailed, ResponseTerminalIncomplete,
	}
	want := []string{"completed", "failed", "incomplete"}
	for index := range want {
		if got[index] != want[index] {
			t.Fatalf("terminal value %d = %q, want %q", index, got[index], want[index])
		}
	}
}
