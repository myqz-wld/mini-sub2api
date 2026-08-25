package storage

import "time"

const (
	CredentialEnabled         = "enabled"
	CredentialDisabled        = "disabled"
	CredentialRequiresLogin   = "requires_login"
	CredentialDeleted         = "deleted"
	KeyActive                 = "active"
	KeyRevoked                = "revoked"
	RequestInProgress         = "in_progress"
	RequestCompleted          = "completed"
	RequestUpstreamErr        = "upstream_error"
	RequestDisconnected       = "client_disconnected"
	TransportHTTP             = "http"
	TransportWebSocket        = "websocket"
	OperationInference        = "inference"
	OperationWebSocketPrewarm = "websocket_prewarm"
)

type Credential struct {
	ID                string     `json:"id"`
	Name              string     `json:"name"`
	Adapter           string     `json:"adapter"`
	AuthKind          string     `json:"authKind"`
	AccountRef        string     `json:"accountRef"`
	UpstreamAccountID *string    `json:"upstreamAccountId,omitempty"`
	Status            string     `json:"status"`
	CreatedAt         time.Time  `json:"createdAt"`
	UpdatedAt         time.Time  `json:"updatedAt"`
	DeletedAt         *time.Time `json:"deletedAt,omitempty"`
}

type APIKey struct {
	ID           string     `json:"id"`
	Name         string     `json:"name"`
	Prefix       string     `json:"prefix"`
	CredentialID string     `json:"credentialId"`
	Status       string     `json:"status"`
	CreatedAt    time.Time  `json:"createdAt"`
	RevokedAt    *time.Time `json:"revokedAt,omitempty"`
}

type CreatedAPIKey struct {
	APIKey
	Secret string `json:"secret"`
}

type Route struct {
	APIKeyID       string
	CredentialID   string
	Adapter        string
	AuthKind       string
	AccountRef     string
	PseudonymScope string
}

type TokenUsage struct {
	InputTokens           int64 `json:"inputTokens"`
	CachedInputTokens     int64 `json:"cachedInputTokens"`
	CacheWriteInputTokens int64 `json:"cacheWriteInputTokens"`
	OutputTokens          int64 `json:"outputTokens"`
	ReasoningOutputTokens int64 `json:"reasoningOutputTokens"`
	TotalTokens           int64 `json:"totalTokens"`
}

type RequestResult struct {
	CompletedAt time.Time
	Status      string
	HTTPStatus  *int
	TTFB        *time.Duration
	Duration    time.Duration
	Usage       *TokenUsage
}

type RequestRecord struct {
	RequestID            string      `json:"requestId"`
	APIKeyID             string      `json:"apiKeyId"`
	CredentialID         string      `json:"credentialId"`
	Transport            string      `json:"transport"`
	OperationKind        string      `json:"operationKind"`
	StartedAt            time.Time   `json:"startedAt"`
	CompletedAt          *time.Time  `json:"completedAt,omitempty"`
	Status               string      `json:"status"`
	HTTPStatus           *int        `json:"httpStatus,omitempty"`
	TTFBMilliseconds     *int64      `json:"ttfbMs,omitempty"`
	DurationMilliseconds *int64      `json:"durationMs,omitempty"`
	Usage                *TokenUsage `json:"usage,omitempty"`
}

type DailyUsage struct {
	Day                   string      `json:"day"`
	APIKeyID              string      `json:"apiKeyId"`
	RequestCount          int64       `json:"requestCount"`
	CompletedCount        int64       `json:"completedCount"`
	ErrorCount            int64       `json:"errorCount"`
	DisconnectedCount     int64       `json:"disconnectedCount"`
	DurationMilliseconds  int64       `json:"durationMs"`
	UsageObservationCount int64       `json:"usageObservationCount"`
	Usage                 *TokenUsage `json:"usage,omitempty"`
}
