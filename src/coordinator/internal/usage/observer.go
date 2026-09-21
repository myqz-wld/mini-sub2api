package usage

import (
	"bytes"
	"encoding/json"
	"mime"
	"strings"

	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

type Observer struct {
	maximum         int
	lineLength      uint8
	lineCR          bool
	dropping        bool
	streaming       bool
	detectStreaming bool
	disabled        bool
	buffer          []byte
	usage           *storage.TokenUsage
	terminal        TerminalStatus
	events          uint64
}

type TerminalStatus uint8

const (
	TerminalUnknown TerminalStatus = iota
	TerminalCompleted
	TerminalUpstreamError
)

func NewObserver(contentType string) *Observer {
	mediaType, _, err := mime.ParseMediaType(contentType)
	if err != nil {
		mediaType = strings.TrimSpace(strings.Split(contentType, ";")[0])
	}
	return &Observer{
		maximum:         int(protocolv1.MustInferenceLimits().OutputBytes),
		streaming:       strings.EqualFold(mediaType, "text/event-stream"),
		detectStreaming: mediaType == "",
	}
}

func (o *Observer) Observe(chunk []byte) {
	if o.disabled || len(chunk) == 0 {
		return
	}
	if o.streaming {
		o.observeSSE(chunk)
		return
	}
	if len(chunk) > o.maximum-len(o.buffer) {
		o.buffer = nil
		o.disabled = true
		return
	}
	o.buffer = append(o.buffer, chunk...)
	if o.detectStreaming && looksLikeSSE(o.buffer) {
		o.streaming = true
		pending := o.buffer
		o.buffer = nil
		o.observeSSE(pending)
	}
}

// Scan each byte once, even when a single event approaches the response budget.
// An over-budget event is ignored; its delimiter restores observation of later events.
func (o *Observer) observeSSE(chunk []byte) {
	for len(chunk) > 0 {
		end := o.eventBoundary(chunk)
		size := len(chunk)
		if end >= 0 {
			size = end
		}
		if !o.dropping {
			if size > o.maximum-len(o.buffer) {
				o.buffer = nil
				o.dropping = true
			} else {
				o.buffer = append(o.buffer, chunk[:size]...)
			}
		}
		chunk = chunk[size:]
		if end < 0 {
			return
		}
		if !o.dropping {
			o.acceptEvent(o.buffer)
		}
		o.buffer = nil
		o.dropping = false
	}
}

func (o *Observer) eventBoundary(chunk []byte) int {
	for i, b := range chunk {
		if b == '\n' {
			empty := o.lineLength == 0 || (o.lineLength == 1 && o.lineCR)
			o.lineLength = 0
			o.lineCR = false
			if empty {
				return i + 1
			}
		} else {
			if o.lineLength == 0 {
				o.lineCR = b == '\r'
			}
			o.lineLength = min(o.lineLength+1, 2)
		}
	}
	return -1
}

func (o *Observer) acceptEvent(event []byte) {
	data := eventData(event)
	if len(data) != 0 && !bytes.Equal(bytes.TrimSpace(data), []byte("[DONE]")) {
		if len(bytes.TrimSpace(data)) == 0 {
			return
		}
		o.events++
		o.acceptJSON(data)
	}
}

func (o *Observer) IsStreaming() bool { return o.streaming }

// StreamProgress is a non-destructive snapshot: unlike Usage/TerminalStatus it never
// flushes a partial event. Only complete nonempty data events advance the sequence.
func (o *Observer) StreamProgress() (uint64, TerminalStatus) { return o.events, o.terminal }

// A forced tail close cannot treat an unparsed data fragment as a clean completion.
func (o *Observer) HasPendingSSEData() bool {
	if !o.streaming {
		return false
	}
	if o.dropping {
		return true
	}
	data := bytes.TrimSpace(eventData(o.buffer))
	return len(data) != 0 && !bytes.Equal(data, []byte("[DONE]"))
}

func looksLikeSSE(buffer []byte) bool {
	trimmed := bytes.TrimLeft(buffer, " \t\r\n")
	return bytes.HasPrefix(trimmed, []byte("event:")) || bytes.HasPrefix(trimmed, []byte("data:")) || bytes.HasPrefix(trimmed, []byte(":"))
}

func (o *Observer) Usage() *storage.TokenUsage {
	o.finish()
	if o.usage == nil {
		return nil
	}
	copy := *o.usage
	return &copy
}

func (o *Observer) TerminalStatus() TerminalStatus {
	o.finish()
	return o.terminal
}

func (o *Observer) finish() {
	if !o.streaming && !o.disabled && len(o.buffer) > 0 {
		o.acceptJSON(o.buffer)
	} else if o.streaming && !o.dropping && len(o.buffer) > 0 {
		o.acceptEvent(o.buffer)
	}
	o.buffer = nil
}

func (o *Observer) acceptJSON(data []byte) {
	var envelope responseEnvelope
	if err := json.Unmarshal(data, &envelope); err != nil {
		return
	}
	o.observeTerminal(envelope.Type, envelope.Status)
	if usage, ok := usageFromEnvelope(&envelope); ok {
		o.usage = &usage
	}
}

func (o *Observer) observeTerminal(eventType, responseStatus string) {
	if o.terminal == TerminalUpstreamError {
		return
	}
	switch eventType {
	case "response.completed":
		o.terminal = TerminalCompleted
	case "response.failed", "response.incomplete", "error":
		o.terminal = TerminalUpstreamError
	default:
		switch responseStatus {
		case "completed":
			o.terminal = TerminalCompleted
		case "failed", "incomplete":
			o.terminal = TerminalUpstreamError
		}
	}
}

func eventData(event []byte) []byte {
	var parts [][]byte
	for _, line := range bytes.Split(event, []byte("\n")) {
		line = bytes.TrimSuffix(line, []byte("\r"))
		if value, ok := bytes.CutPrefix(line, []byte("data:")); ok {
			parts = append(parts, bytes.TrimPrefix(value, []byte(" ")))
		}
	}
	if len(parts) == 1 {
		return parts[0]
	}
	return bytes.Join(parts, []byte("\n"))
}

type responseEnvelope struct {
	Type     string         `json:"type"`
	Status   string         `json:"status"`
	Usage    *responseUsage `json:"usage"`
	Response *struct {
		Usage *responseUsage `json:"usage"`
	} `json:"response"`
}

type responseUsage struct {
	InputTokens       int64 `json:"input_tokens"`
	OutputTokens      int64 `json:"output_tokens"`
	TotalTokens       int64 `json:"total_tokens"`
	InputTokenDetails *struct {
		CachedTokens     int64 `json:"cached_tokens"`
		CacheWriteTokens int64 `json:"cache_write_tokens"`
	} `json:"input_tokens_details"`
	OutputTokenDetails *struct {
		ReasoningTokens int64 `json:"reasoning_tokens"`
	} `json:"output_tokens_details"`
}

func parseUsage(data []byte) (storage.TokenUsage, bool) {
	var envelope responseEnvelope
	if err := json.Unmarshal(data, &envelope); err != nil {
		return storage.TokenUsage{}, false
	}
	return usageFromEnvelope(&envelope)
}

func usageFromEnvelope(envelope *responseEnvelope) (storage.TokenUsage, bool) {
	value := envelope.Usage
	if value == nil && envelope.Response != nil {
		value = envelope.Response.Usage
	}
	if value == nil || hasNegativeUsage(value) {
		return storage.TokenUsage{}, false
	}
	result := storage.TokenUsage{
		InputTokens:  value.InputTokens,
		OutputTokens: value.OutputTokens,
		TotalTokens:  value.TotalTokens,
	}
	if value.InputTokenDetails != nil {
		result.CachedInputTokens = value.InputTokenDetails.CachedTokens
		result.CacheWriteInputTokens = value.InputTokenDetails.CacheWriteTokens
	}
	if value.OutputTokenDetails != nil {
		result.ReasoningOutputTokens = value.OutputTokenDetails.ReasoningTokens
	}
	return result, true
}

func hasNegativeUsage(value *responseUsage) bool {
	if value.InputTokens < 0 || value.OutputTokens < 0 || value.TotalTokens < 0 {
		return true
	}
	if value.InputTokenDetails != nil &&
		(value.InputTokenDetails.CachedTokens < 0 || value.InputTokenDetails.CacheWriteTokens < 0) {
		return true
	}
	return value.OutputTokenDetails != nil && value.OutputTokenDetails.ReasoningTokens < 0
}
