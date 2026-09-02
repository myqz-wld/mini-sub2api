package usage

import (
	"bytes"
	"encoding/json"
	"mime"
	"strings"

	"mini-sub2api/src/coordinator/internal/storage"
)

const maxObservedEventBytes = 8 * 1024 * 1024

type Observer struct {
	streaming       bool
	detectStreaming bool
	disabled        bool
	buffer          []byte
	usage           *storage.TokenUsage
	terminal        TerminalStatus
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
		streaming:       strings.EqualFold(mediaType, "text/event-stream"),
		detectStreaming: mediaType == "",
	}
}

func (o *Observer) Observe(chunk []byte) {
	if o.disabled || len(chunk) == 0 {
		return
	}
	if len(o.buffer)+len(chunk) > maxObservedEventBytes {
		o.buffer = nil
		o.disabled = true
		return
	}
	o.buffer = append(o.buffer, chunk...)
	if !o.streaming && o.detectStreaming && looksLikeSSE(o.buffer) {
		o.streaming = true
	}
	if o.streaming {
		o.consumeSSEEvents()
	}
}

func looksLikeSSE(buffer []byte) bool {
	trimmed := bytes.TrimLeft(buffer, " \t\r\n")
	return bytes.HasPrefix(trimmed, []byte("event:")) || bytes.HasPrefix(trimmed, []byte("data:"))
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
		o.buffer = nil
	} else if o.streaming && !o.disabled && len(o.buffer) > 0 {
		data := eventData(o.buffer)
		if len(data) != 0 && !bytes.Equal(bytes.TrimSpace(data), []byte("[DONE]")) {
			o.acceptJSON(data)
		}
		o.buffer = nil
	}
}

func (o *Observer) consumeSSEEvents() {
	for {
		index, delimiterLength := nextEventBoundary(o.buffer)
		if index < 0 {
			return
		}
		event := o.buffer[:index]
		o.buffer = o.buffer[index+delimiterLength:]
		data := eventData(event)
		if len(data) != 0 && !bytes.Equal(bytes.TrimSpace(data), []byte("[DONE]")) {
			o.acceptJSON(data)
		}
	}
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

func nextEventBoundary(buffer []byte) (int, int) {
	lf := bytes.Index(buffer, []byte("\n\n"))
	crlf := bytes.Index(buffer, []byte("\r\n\r\n"))
	switch {
	case lf < 0:
		if crlf < 0 {
			return -1, 0
		}
		return crlf, 4
	case crlf < 0 || lf < crlf:
		return lf, 2
	default:
		return crlf, 4
	}
}

func eventData(event []byte) []byte {
	lines := bytes.Split(bytes.ReplaceAll(event, []byte("\r\n"), []byte("\n")), []byte("\n"))
	var data []byte
	for _, line := range lines {
		if !bytes.HasPrefix(line, []byte("data:")) {
			continue
		}
		value := bytes.TrimPrefix(line, []byte("data:"))
		value = bytes.TrimPrefix(value, []byte(" "))
		if len(data) > 0 {
			data = append(data, '\n')
		}
		data = append(data, value...)
	}
	return data
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
