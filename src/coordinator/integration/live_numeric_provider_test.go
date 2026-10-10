//go:build nativeparity && liveparity

package integration

import (
	"encoding/json"
	"strings"
	"testing"
	"time"
)

// Only static classifications escape the callback; raw provider bodies stay temporary.
type numericProviderObservation struct {
	status                                               int
	bytes                                                int
	effort, integer, invalid, structuredError, completed bool
	parameter, invalidType, explicitTypeMessage          bool
}

func observeNumericProvider(relay *liveWireRelay) <-chan numericProviderObservation {
	observations := make(chan numericProviderObservation, 4)
	relay.mu.Lock()
	defer relay.mu.Unlock()
	relay.observeHTTP = func(status int, body []byte) {
		text := strings.ToLower(string(body))
		observation := numericProviderObservation{status: status, bytes: len(body),
			effort:  strings.Contains(text, "reasoning") && strings.Contains(text, "effort"),
			integer: strings.Contains(text, "integer"), invalid: strings.Contains(text, "invalid"),
			completed: strings.Contains(text, `"response.completed"`)}
		inspect := func(body []byte) {
			var value map[string]any
			if json.Unmarshal(body, &value) != nil {
				return
			}
			response, _ := value["response"].(map[string]any)
			observation.structuredError = observation.structuredError || value["error"] != nil || response["error"] != nil
			for _, raw := range []any{value["error"], response["error"]} {
				failure, _ := raw.(map[string]any)
				message, _ := failure["message"].(string)
				observation.parameter = observation.parameter || failure["param"] == "reasoning.effort"
				observation.invalidType = observation.invalidType || failure["code"] == "invalid_type"
				observation.explicitTypeMessage = observation.explicitTypeMessage || (strings.Contains(strings.ToLower(message), "invalid type for 'reasoning.effort'") && strings.Contains(message, "expected a string"))
			}
		}
		inspect(body)
		for _, line := range strings.Split(string(body), "\n") {
			if data, ok := strings.CutPrefix(line, "data: "); ok {
				inspect([]byte(data))
			}
		}
		select {
		case observations <- observation:
		default:
		}
	}
	return observations
}

func waitNumericProvider(t *testing.T, observed <-chan numericProviderObservation) numericProviderObservation {
	t.Helper()
	select {
	case observation := <-observed:
		return observation
	case <-time.After(5 * time.Second):
		t.Fatal("numeric provider response observation missing")
		return numericProviderObservation{}
	}
}

func (o numericProviderObservation) confirmedEffortRejection() bool {
	return o.structuredError && o.effort && o.integer && o.invalid && ((o.parameter && o.invalidType) || o.explicitTypeMessage) && !o.completed
}
func (o numericProviderObservation) report(t *testing.T) {
	t.Helper()
	t.Logf("NUMERIC_PROVIDER_FACTS http_status=%d response_prefix_bytes=%d effort=%t integer=%t invalid=%t structured_error=%t completed=%t", o.status, o.bytes, o.effort, o.integer, o.invalid, o.structuredError, o.completed)
	t.Logf("NUMERIC_PROVIDER_TYPE parameter_reasoning_effort=%t invalid_type_code=%t explicit_string_type_message=%t", o.parameter, o.invalidType, o.explicitTypeMessage)
}
func assertNumericPacket(t *testing.T, packet nativePacket) {
	t.Helper()
	var value map[string]any
	if json.Unmarshal(nativePacketJSON(t, packet), &value) != nil {
		t.Fatal("numeric native capture JSON")
	}
	reasoning, _ := value["reasoning"].(map[string]any)
	if reasoning["effort"] != float64(8192) {
		t.Fatal("direct native did not send numeric effort 8192")
	}
}
