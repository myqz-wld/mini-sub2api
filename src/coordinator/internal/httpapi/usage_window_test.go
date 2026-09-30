package httpapi

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestUsageWindowCoreErrorPublicMapping(t *testing.T) {
	for _, code := range []string{"usage_limit_reached", "usage_not_included", "insufficient_quota"} {
		for _, minutes := range []*uint16{nil, new(uint16(0)), new(uint16(300)), new(uint16(65535))} {
			envelope := protocolv1.ErrorEnvelope{Error: protocolv1.CoreError{
				Code: code, Message: "Synthetic bounded error.", RequestID: "req_synthetic",
				LimitWindowMinutes: minutes,
				FailureMetadata: protocolv1.FailureMetadata{
					RetryAdvice: protocolv1.RetryNever, Phase: protocolv1.PhaseUpstreamResponse,
					DeliveryState: protocolv1.DeliveryDelivered,
				},
			}}
			data, err := json.Marshal(envelope)
			if err != nil {
				t.Fatal(err)
			}
			response := &http.Response{StatusCode: http.StatusTooManyRequests,
				Header: http.Header{"Content-Type": {"application/json"}},
				Body:   io.NopCloser(bytes.NewReader(data)),
			}
			parsed, ok := detectCoreError(response, "req_synthetic")
			if !ok {
				t.Fatal("typed usage-window core envelope was not recognized")
			}
			writer := httptest.NewRecorder()
			writeOpenAIErrorWithUsageWindow(writer, response.StatusCode, parsed.Code, parsed.Message,
				parsed.RequestID, parsed.FailureMetadata, parsed.LimitWindowMinutes)
			var public map[string]map[string]any
			if err := json.Unmarshal(writer.Body.Bytes(), &public); err != nil {
				t.Fatal(err)
			}
			fields := public["error"]
			if fields["type"] != code || fields["code"] != code || writer.Code != http.StatusTooManyRequests {
				t.Fatal("native category or status changed")
			}
			if code == "usage_limit_reached" && minutes != nil {
				if fields["limit_window_minutes"] != float64(*minutes) {
					t.Fatal("usage-window value changed")
				}
			} else if _, exists := fields["limit_window_minutes"]; exists {
				t.Fatal("usage window present without the matching category/value")
			}
			for _, private := range []string{"limitWindowMinutes", "requestId"} {
				if _, exists := fields[private]; exists {
					t.Fatal("internal field reached public envelope")
				}
			}
		}
	}
}
