package protocolv1

const (
	VersionHeader    = "X-Mini-Sub2Api-Protocol-Version"
	AccountRefHeader = "X-Mini-Sub2Api-Account-Ref"
	RequestIDHeader  = "X-Mini-Sub2Api-Request-Id"
	CoreTTFBHeader   = "X-Mini-Sub2Api-Core-TTFB-Ms"
	Version          = "1"
)

type BuildIdentity struct {
	Name    string `json:"name"`
	Version string `json:"version"`
	Commit  string `json:"commit"`
}

type Capabilities struct {
	ResponsesWebSocket bool `json:"responsesWebSocket"`
}

type Readiness struct {
	ProtocolVersion string        `json:"protocolVersion"`
	Port            uint16        `json:"port"`
	PID             int           `json:"pid"`
	Build           BuildIdentity `json:"build"`
	Capabilities    Capabilities  `json:"capabilities"`
}

type ErrorEnvelope struct {
	Error CoreError `json:"error"`
}

type CoreError struct {
	Code      string `json:"code"`
	Message   string `json:"message"`
	Retryable bool   `json:"retryable"`
	RequestID string `json:"requestId"`
}
