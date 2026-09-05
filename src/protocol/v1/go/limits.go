package protocolv1

import (
	"bytes"
	_ "embed"
	"encoding/json"
	"fmt"
	"io"
	"os"
)

//go:embed limits.json
var inferenceDefaults []byte

const LimitsEnv = "MINI_SUB2API_LIMITS"

// InferenceLimits are operator-owned process settings. The child Core inherits the same environment.
type InferenceLimits struct {
	RequestBytes   uint64 `json:"requestBytes"`
	OutputBytes    uint64 `json:"outputBytes"`
	OutputItems    uint64 `json:"outputItems"`
	GlobalBytes    uint64 `json:"globalBytes"`
	KeyBytes       uint64 `json:"keyBytes"`
	SessionBytes   uint64 `json:"sessionBytes"`
	SessionRecords uint64 `json:"sessionRecords"`
}

func ParseInferenceLimits(override string) (InferenceLimits, error) {
	var result InferenceLimits
	if err := json.Unmarshal(inferenceDefaults, &result); err != nil {
		return result, err
	}
	if override != "" {
		var fields map[string]json.RawMessage
		if json.Unmarshal([]byte(override), &fields) != nil || fields == nil {
			return result, fmt.Errorf("invalid inference limits")
		}
		for _, raw := range fields {
			if bytes.Equal(bytes.TrimSpace(raw), []byte("null")) {
				return result, fmt.Errorf("invalid inference limits")
			}
		}
		decoder := json.NewDecoder(bytes.NewBufferString(override))
		decoder.DisallowUnknownFields()
		if err := decoder.Decode(&result); err != nil {
			return result, fmt.Errorf("invalid inference limits")
		}
		var extra any
		if decoder.Decode(&extra) != io.EOF {
			return result, fmt.Errorf("invalid inference limits")
		}
	}
	for _, value := range []uint64{result.RequestBytes, result.OutputBytes, result.OutputItems, result.GlobalBytes, result.KeyBytes, result.SessionBytes, result.SessionRecords} {
		if value == 0 || value > 1<<40 {
			return result, fmt.Errorf("inference limits must be between 1 and 2^40")
		}
	}
	if result.SessionBytes > result.KeyBytes || result.KeyBytes > result.GlobalBytes {
		return result, fmt.Errorf("inference byte budgets must satisfy session <= key <= global")
	}
	return result, nil
}

func LoadInferenceLimits() (InferenceLimits, error) {
	return ParseInferenceLimits(os.Getenv(LimitsEnv))
}

func MustInferenceLimits() InferenceLimits {
	limits, err := LoadInferenceLimits()
	if err != nil {
		panic(err)
	}
	return limits
}
