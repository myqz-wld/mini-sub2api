//go:build nativeparity

package integration

import (
	"encoding/json"
	"fmt"
	"reflect"
	"strings"
	"testing"
)

// One bijection per typed identity domain, retained across all requests. This checks
// correlation and stability without removing whole metadata objects from comparison.
type transportProjection struct {
	identities map[string]map[string]string
	known      map[string]int
	starts     map[string]float64
	turn       string
}

func (p *transportProjection) observed(name string) {
	if p.known == nil {
		p.known = map[string]int{}
	}
	p.known[name]++
}

func (p *transportProjection) identity(t *testing.T, before, after any, domain, path string) {
	t.Helper()
	original, ok := before.(string)
	emitted, valid := after.(string)
	if !ok || !valid || (original == "") != (emitted == "") {
		t.Errorf("typed identity presence/shape differs: %s", path)
		return
	}
	if original == "" {
		return
	}
	if p.identities[domain] == nil {
		p.identities[domain] = map[string]string{}
	}
	mapping := p.identities[domain]
	if known, exists := mapping[original]; exists {
		if known != emitted {
			t.Errorf("typed identity mapping changed: %s", path)
		}
		return
	}
	for input, output := range mapping {
		if input != original && output == emitted {
			t.Errorf("distinct typed identities collapsed: %s", path)
		}
	}
	mapping[original] = emitted
}

func (p *transportProjection) metadata(t *testing.T, before, after map[string]any, path string) {
	t.Helper()
	for field, expected := range before {
		actual, present := after[field]
		fieldPath := path + "." + field
		if !present {
			t.Errorf("metadata field disappeared: %s", fieldPath)
			continue
		}
		switch field {
		case "turn_started_at_unix_ms":
			// Preserve the native timestamp exactly throughout its tool loop.
			original, originalOK := expected.(float64)
			emitted, emittedOK := actual.(float64)
			if !originalOK || !emittedOK || original <= 0 || emitted <= 0 {
				t.Errorf("turn start timestamp shape differs: %s", fieldPath)
				continue
			}
			if p.starts == nil {
				p.starts = map[string]float64{}
			}
			if known, exists := p.starts[p.turn]; exists && known != emitted {
				t.Errorf("projected turn start changed within its tool loop: %s", fieldPath)
			}
			p.starts[p.turn] = emitted
			if original != emitted {
				t.Errorf("caller turn start timestamp changed: %s", fieldPath)
			}
		case "session_id", "thread_id", "parent_thread_id", "forked_from_thread_id", "x-codex-parent-thread-id":
			p.identity(t, expected, actual, "thread", fieldPath)
		case "turn_id", "parent_turn_id", "root_turn_id":
			p.identity(t, expected, actual, "turn", fieldPath)
		case "installation_id", "x-codex-installation-id":
			p.identity(t, expected, actual, "installation", fieldPath)
		case "context_window_id", "first_window_id", "previous_window_id":
			p.identity(t, expected, actual, "context-window", fieldPath)
		case "window_id", "x-codex-window-id":
			left, leftOK := expected.(string)
			right, rightOK := actual.(string)
			leftThread, leftNumber, leftFound := strings.Cut(left, ":")
			rightThread, rightNumber, rightFound := strings.Cut(right, ":")
			if !leftOK || !rightOK || !leftFound || !rightFound || leftNumber != rightNumber {
				t.Errorf("window counter/shape differs: %s", fieldPath)
			} else {
				p.identity(t, leftThread, rightThread, "thread", fieldPath)
			}
		case "x-codex-turn-metadata":
			var left, right map[string]any
			a, _ := expected.(string)
			b, _ := actual.(string)
			if json.Unmarshal([]byte(a), &left) != nil || json.Unmarshal([]byte(b), &right) != nil {
				t.Errorf("nested metadata JSON differs: %s", fieldPath)
			} else {
				p.metadata(t, left, right, fieldPath)
			}
		default:
			if !reflect.DeepEqual(expected, actual) {
				t.Errorf("metadata value differs: %s", fieldPath)
			}
		}
	}
	for field := range after {
		if _, exists := before[field]; !exists {
			t.Errorf("metadata field added: %s.%s", path, field)
		}
	}
}

func (p *transportProjection) compare(t *testing.T, before, after map[string]any, path string) {
	t.Helper()
	p.turn, _ = transportMetadata(t, after)["turn_id"].(string)
	for field, expected := range before {
		actual, present := after[field]
		fieldPath := path + "." + field
		if !present {
			t.Errorf("request field disappeared: %s", fieldPath)
			continue
		}
		switch field {
		case "client_metadata":
			p.metadata(t, transportMetadata(t, before), transportMetadata(t, after), fieldPath)
		case "previous_response_id":
			p.identity(t, expected, actual, "response", fieldPath)
		case "prompt_cache_key":
			p.identity(t, expected, actual, "thread", fieldPath)
		case "input":
			left, leftOK := expected.([]any)
			right, rightOK := actual.([]any)
			if !leftOK || !rightOK || len(left) != len(right) {
				t.Errorf("ordered input shape differs: %s", fieldPath)
				continue
			}
			for i := range left {
				item, itemOK := left[i].(map[string]any)
				projected, projectedOK := right[i].(map[string]any)
				if !itemOK || !projectedOK {
					t.Errorf("input item shape differs: %s[%d]", fieldPath, i)
					continue
				}
				p.item(t, item, projected, fmt.Sprintf("%s[%d]", fieldPath, i))
			}
		default:
			if !reflect.DeepEqual(expected, actual) {
				t.Errorf("request value differs: %s", fieldPath)
			}
		}
	}
	for field := range after {
		if _, exists := before[field]; !exists {
			t.Errorf("request field added: %s.%s", path, field)
		}
	}
}

func (p *transportProjection) item(t *testing.T, before, after map[string]any, path string) {
	t.Helper()
	for field, expected := range before {
		actual, present := after[field]
		if !present {
			t.Errorf("input item field disappeared: %s.%s", path, field)
			continue
		}
		switch field {
		case "id", "previous_item_id":
			p.identity(t, expected, actual, "item", path+"."+field)
		case "call_id":
			p.identity(t, expected, actual, "call", path+"."+field)
		case "internal_chat_message_metadata_passthrough":
			left, leftOK := expected.(map[string]any)
			right, rightOK := actual.(map[string]any)
			if !leftOK || !rightOK {
				t.Errorf("item metadata shape differs: %s.%s", path, field)
			} else {
				p.metadata(t, left, right, path+"."+field)
			}
		default:
			if !reflect.DeepEqual(expected, actual) {
				t.Errorf("input item value differs: %s.%s", path, field)
			}
		}
	}
	for field := range after {
		if _, exists := before[field]; !exists {
			t.Errorf("input item field added: %s.%s", path, field)
		}
	}
}
