package protocolv1

const (
	VersionHeader              = "X-Mini-Sub2Api-Protocol-Version"
	AccountRefHeader           = "X-Mini-Sub2Api-Account-Ref"
	PseudonymScopeHeader       = "X-Mini-Sub2Api-Pseudonym-Scope"
	RequestIDHeader            = "X-Mini-Sub2Api-Request-Id"
	CoreTTFBHeader             = "X-Mini-Sub2Api-Core-TTFB-Ms"
	ProviderRequestIDHeader    = "X-Mini-Sub2Api-Provider-Request-Id"
	ProviderRequestIDEventType = "mini_sub2api.provider_request_id"
	ResponseTerminalHeader     = "X-Mini-Sub2Api-Response-Terminal"
	ResponseTerminalCompleted  = "completed"
	ResponseTerminalFailed     = "failed"
	ResponseTerminalIncomplete = "incomplete"
	FailurePhaseTrailer        = "X-Mini-Sub2Api-Failure-Phase"
	DeliveryStateTrailer       = "X-Mini-Sub2Api-Delivery-State"
	RetryAdviceTrailer         = "X-Mini-Sub2Api-Retry-Advice"
	FailureCloseCode           = 4500
	Version                    = "1"
)

type RetryAdvice string

const (
	RetrySafe      RetryAdvice = "safe"
	RetryAmbiguous RetryAdvice = "ambiguous"
	RetryNever     RetryAdvice = "never"
)

type FailurePhase string

const (
	PhaseInternal         FailurePhase = "internal"
	PhaseRequest          FailurePhase = "request"
	PhaseCredential       FailurePhase = "credential"
	PhaseUpstreamConnect  FailurePhase = "upstream_connect"
	PhaseUpstreamRequest  FailurePhase = "upstream_request"
	PhaseUpstreamResponse FailurePhase = "upstream_response"
	PhaseUpstreamStream   FailurePhase = "upstream_stream"
	PhaseWebSocketRelay   FailurePhase = "websocket_relay"
)

type DeliveryState string

const (
	DeliveryNotDelivered      DeliveryState = "not_delivered"
	DeliveryPossiblyDelivered DeliveryState = "possibly_delivered"
	DeliveryDelivered         DeliveryState = "delivered"
)

type FailureMetadata struct {
	RetryAdvice   RetryAdvice   `json:"retryAdvice"`
	Phase         FailurePhase  `json:"phase"`
	DeliveryState DeliveryState `json:"deliveryState"`
}

func (metadata FailureMetadata) Valid() bool {
	switch metadata.RetryAdvice {
	case RetrySafe, RetryAmbiguous, RetryNever:
	default:
		return false
	}
	switch metadata.Phase {
	case PhaseInternal, PhaseRequest, PhaseCredential, PhaseUpstreamConnect,
		PhaseUpstreamRequest, PhaseUpstreamResponse, PhaseUpstreamStream, PhaseWebSocketRelay:
	default:
		return false
	}
	switch metadata.DeliveryState {
	case DeliveryNotDelivered, DeliveryPossiblyDelivered, DeliveryDelivered:
	default:
		return false
	}
	switch metadata.RetryAdvice {
	case RetrySafe:
		return metadata.DeliveryState == DeliveryNotDelivered
	case RetryAmbiguous:
		return metadata.DeliveryState == DeliveryPossiblyDelivered
	case RetryNever:
		return metadata.DeliveryState != DeliveryPossiblyDelivered
	default:
		return false
	}
}

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
	RequestID string `json:"requestId"`
	FailureMetadata
}

type ProviderRequestIDControl struct {
	Type              string `json:"type"`
	ProviderRequestID string `json:"providerRequestId"`
}
