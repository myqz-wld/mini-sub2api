package protocolv1

import (
	"bytes"
	"encoding/json"
)

// SSE classes are a fixed vocabulary. Never put provider-controlled event names in diagnostics.
type SSEClass uint8

const (
	SSEStatus SSEClass = iota
	SSEText
	SSEReasoning
	SSETool
	SSEMedia
	SSEItem
	SSETerminal
	SSEEmpty
	SSEOther
	SSEInvalid
	SSEClassCount
)

func (c SSEClass) String() string {
	names := [...]string{"status", "text", "reasoning", "tool", "media", "item", "terminal", "empty", "other", "invalid"}
	if c >= SSEClassCount {
		return "other"
	}
	return names[c]
}

func (c SSEClass) IsOutput() bool { return c >= SSEText && c <= SSEItem }

// ClassifySSEData observes output progress, not response validity or completion proof.
// Scalar payloads become booleans instead of copied strings/trees. Unknown events never
// renew output deadlines. Field matching follows encoding/json's case-insensitive keys.
func ClassifySSEData(data []byte) SSEClass {
	var event progressEvent
	if json.Unmarshal(data, &event) != nil || bytes.Equal(bytes.TrimSpace(data), []byte("null")) {
		return SSEInvalid
	}
	withPayload := func(class SSEClass, present nonemptyText) SSEClass {
		if present {
			return class
		}
		return SSEEmpty
	}
	switch string(event.Type) {
	case "response.completed", "response.failed", "response.incomplete", "error":
		return SSETerminal
	case "response.created", "response.in_progress", "response.queued", "response.metadata",
		"codex.response.metadata", "responsesapi.websocket_timing", "ping", "heartbeat", "keepalive",
		"response.output_item.added", "response.web_search_call.in_progress", "response.web_search_call.searching",
		"response.file_search_call.in_progress", "response.file_search_call.searching",
		"response.code_interpreter_call.in_progress", "response.code_interpreter_call.interpreting",
		"response.image_generation_call.in_progress", "response.image_generation_call.generating":
		return SSEStatus
	case "response.output_text.delta", "response.refusal.delta":
		return withPayload(SSEText, event.Delta)
	case "response.output_text.done":
		return withPayload(SSEText, event.Text)
	case "response.refusal.done":
		return withPayload(SSEText, event.Refusal)
	case "response.reasoning_text.delta", "response.reasoning_summary_text.delta", "response.reasoning.delta":
		return withPayload(SSEReasoning, event.Delta)
	case "response.reasoning_text.done", "response.reasoning_summary_text.done":
		return withPayload(SSEReasoning, event.Text)
	case "response.function_call_arguments.delta", "response.custom_tool_call_input.delta", "response.code_interpreter_call_code.delta":
		return withPayload(SSETool, event.Delta)
	case "response.function_call_arguments.done":
		return withPayload(SSETool, event.Arguments)
	case "response.custom_tool_call_input.done":
		return withPayload(SSETool, event.Input)
	case "response.code_interpreter_call_code.done":
		return withPayload(SSETool, event.Code)
	case "response.audio.delta", "response.output_audio.delta":
		return withPayload(SSEMedia, event.Delta)
	case "response.image_generation_call.partial_image":
		return withPayload(SSEMedia, event.PartialImage)
	case "response.content_part.added", "response.content_part.done":
		return withPayload(SSEText, event.Part.Text || event.Part.Refusal)
	case "response.reasoning_summary_part.added", "response.reasoning_summary_part.done":
		return withPayload(SSEReasoning, event.Part.Text)
	case "response.output_item.done":
		if event.Item.Type == "" || event.Item.unfinished() {
			return SSEEmpty
		}
		switch string(event.Item.Type) {
		case "reasoning":
			return SSEReasoning
		case "function_call", "custom_tool_call", "local_shell_call", "shell_call", "code_interpreter_call", "web_search_call", "file_search_call", "tool_search_call":
			return SSETool
		case "image_generation_call":
			return SSEMedia
		default:
			return SSEItem
		}
	default:
		return SSEOther
	}
}

type progressEvent struct {
	Type         shortText    `json:"type"`
	Delta        nonemptyText `json:"delta"`
	Text         nonemptyText `json:"text"`
	Refusal      nonemptyText `json:"refusal"`
	Arguments    nonemptyText `json:"arguments"`
	Input        nonemptyText `json:"input"`
	Code         nonemptyText `json:"code"`
	PartialImage nonemptyText `json:"partial_image_b64"`
	Item         progressItem `json:"item"`
	Part         progressPart `json:"part"`
}

type nonemptyText bool

func (p *nonemptyText) UnmarshalJSON(data []byte) error {
	*p = len(data) > 2 && data[0] == '"'
	return nil
}

type shortText string

func (s *shortText) UnmarshalJSON(data []byte) error {
	*s = ""
	if len(data) < 2 || len(data) > 1024 || data[0] != '"' {
		return nil
	}
	var value string
	if err := json.Unmarshal(data, &value); err != nil {
		return err
	}
	if len(value) <= 128 {
		*s = shortText(value)
	}
	return nil
}

type progressItem struct {
	Type   shortText `json:"type"`
	Status shortText `json:"status"`
}

func (item *progressItem) UnmarshalJSON(data []byte) error {
	*item = progressItem{}
	if len(data) == 0 || data[0] != '{' {
		return nil
	}
	type plain progressItem
	return json.Unmarshal(data, (*plain)(item))
}

func (item progressItem) unfinished() bool {
	switch string(item.Status) {
	case "in_progress", "incomplete", "queued", "searching", "generating", "interpreting":
		return true
	default:
		return false
	}
}

type progressPart struct {
	Text    nonemptyText `json:"text"`
	Refusal nonemptyText `json:"refusal"`
}

func (part *progressPart) UnmarshalJSON(data []byte) error {
	*part = progressPart{}
	if len(data) == 0 || data[0] != '{' {
		return nil
	}
	type plain progressPart
	return json.Unmarshal(data, (*plain)(part))
}
