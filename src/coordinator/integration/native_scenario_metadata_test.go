//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"reflect"
	"sort"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"
)

func nativeScenarioPacketValue(t *testing.T, packet nativePacket) map[string]any {
	t.Helper()
	body := packet.payload
	if packet.headers.Get("Content-Encoding") == "zstd" {
		decoder, err := zstd.NewReader(nil, zstd.WithDecoderMaxMemory(nativeCaptureLimit), zstd.WithDecoderConcurrency(1))
		if err != nil {
			t.Fatal("scenario decoder initialization")
		}
		defer decoder.Close()
		body, err = decoder.DecodeAll(body, nil)
		if err != nil {
			t.Fatal("scenario compressed packet decode")
		}
	}
	var value map[string]any
	if json.Unmarshal(body, &value) != nil {
		t.Fatal("scenario packet JSON decode")
	}
	return value
}

type nativeMetadataRelations struct {
	forward map[string]string
	reverse map[string]string
	starts  map[string][2]any
}

func (r *nativeMetadataRelations) identity(t *testing.T, path string, before, after any) {
	t.Helper()
	b, ok := before.(string)
	a, valid := after.(string)
	if !ok || !valid || (b == "") != (a == "") {
		t.Fatalf("metadata identity shape changed: %s", path)
	}
	if b == "" {
		return
	}
	if r.forward == nil {
		r.forward, r.reverse = map[string]string{}, map[string]string{}
	}
	if previous, found := r.forward[b]; found && previous != a {
		t.Fatalf("metadata identity mapping changed across carriers or requests: %s", path)
	}
	if original, found := r.reverse[a]; found && original != b {
		t.Fatalf("distinct native identities collapsed: %s", path)
	}
	r.forward[b], r.reverse[a] = a, b
}

func (r *nativeMetadataRelations) compare(t *testing.T, path string, before, after map[string]any) []string {
	t.Helper()
	var differences []string
	if turn, ok := before["turn_id"].(string); ok && turn != "" {
		if start, present := before["turn_started_at_unix_ms"]; present {
			b, bok := start.(float64)
			a, aok := after["turn_started_at_unix_ms"].(float64)
			if !bok || !aok || b <= 0 || a <= 0 {
				t.Fatal("native or projected turn start is not a positive timestamp")
			}
			if r.starts == nil {
				r.starts = map[string][2]any{}
			}
			pair := [2]any{b, a}
			if saved, seen := r.starts[turn]; seen && saved != pair {
				t.Fatal("native or projected turn start changed during one turn")
			}
			r.starts[turn] = pair
		}
	}
	for field, value := range before {
		target, present := after[field]
		location := path + "." + field
		if !present {
			differences = append(differences, "removed:"+location)
			continue
		}
		switch field {
		case "context_window_id":
			b, _ := value.(string)
			a, _ := target.(string)
			if !isUUIDVersion(b, '7') || !isUUIDVersion(a, '7') {
				t.Fatal("native or projected context window is not UUIDv7")
			}
			r.identity(t, location, value, target)
		case "session_id", "thread_id", "turn_id", "parent_turn_id", "root_turn_id", "parent_thread_id", "forked_from_thread_id", "installation_id", "x-codex-installation-id", "x-codex-parent-thread-id":
			r.identity(t, location, value, target)
		case "window_id", "x-codex-window-id":
			b, bok := value.(string)
			a, aok := target.(string)
			bi, ai := strings.LastIndexByte(b, ':'), strings.LastIndexByte(a, ':')
			if !bok || !aok || bi < 1 || ai < 1 || b[bi:] != a[ai:] {
				t.Fatalf("metadata window sequence changed: %s", location)
			}
			r.identity(t, location, b[:bi], a[:ai])
		case "x-codex-turn-metadata":
			var b, a map[string]any
			bs, bok := value.(string)
			as, aok := target.(string)
			if !bok || !aok || json.Unmarshal([]byte(bs), &b) != nil || json.Unmarshal([]byte(as), &a) != nil {
				t.Fatal("scenario turn metadata JSON shape")
			}
			differences = append(differences, r.compare(t, location, b, a)...)
		default:
			if !reflect.DeepEqual(value, target) {
				differences = append(differences, "changed:"+location)
			}
		}
	}
	for field := range after {
		if _, found := before[field]; !found {
			differences = append(differences, "added:"+path+"."+field)
		}
	}
	sort.Strings(differences)
	return differences
}

func TestNativeScenarioMetadataRootLifecycle(t *testing.T) {
	for _, subscription := range []bool{false, true} {
		for _, ws := range []bool{false, true} {
			for _, model := range []string{"gpt-5.4", "gpt-5.6-sol"} {
				t.Run(fmt.Sprintf("subscription=%t/ws=%t/%s", subscription, ws, model), func(t *testing.T) {
					capture := newNativeCapture(t)
					gateway := newNativeGateway(t, capture.server.URL, subscription)
					options := nativeOptions{endpoint: gateway.server.URL, bearer: gateway.secret, model: model, ws: ws}
					client := startNativeClient(t, options)
					thread := client.thread(options)
					client.turn(thread, "synthetic metadata first turn")
					client.turn(thread, "synthetic metadata second turn")
					packets, wires := gateway.tap.packets(t), capture.snapshot()
					if len(packets) != len(wires) || len(businessWires(wires)) != 3 {
						t.Fatal("metadata scenario inference sequence changed")
					}
					var relations nativeMetadataRelations
					seenDifferences := map[string]bool{}
					for index, wire := range wires {
						before := nativeScenarioPacketValue(t, packets[index])
						if !subscription && !bytes.Equal(packets[index].payload, wire.encodedBody) {
							t.Fatal("API-key metadata scenario application payload changed")
						}
						b, bok := before["client_metadata"].(map[string]any)
						a, aok := wire.value["client_metadata"].(map[string]any)
						if !bok || !aok {
							t.Fatal("root native metadata absent")
						}
						for _, difference := range relations.compare(t, "client_metadata", b, a) {
							seenDifferences[difference] = true
						}
						input, _ := before["input"].([]any)
						emitted, _ := wire.value["input"].([]any)
						if len(input) != len(emitted) {
							t.Fatal("native historical metadata input sequence changed")
						}
						for i, raw := range input {
							item, _ := raw.(map[string]any)
							projected, _ := emitted[i].(map[string]any)
							b, present := item["internal_chat_message_metadata_passthrough"].(map[string]any)
							if !present {
								continue
							}
							a, ok := projected["internal_chat_message_metadata_passthrough"].(map[string]any)
							if !ok {
								t.Fatal("native historical item metadata removed")
							}
							for _, difference := range relations.compare(t, "input.item_metadata", b, a) {
								seenDifferences[difference] = true
							}
						}
					}
					var differences []string
					for difference := range seenDifferences {
						// These measured normalization differences remain explicit. Unexpected paths
						// fail rather than being swallowed by a metadata-wide exclusion.
						allowed := subscription && nativeRootMetadataDifference(difference)
						if !allowed {
							t.Errorf("unclassified native metadata difference: %s", difference)
						}
						differences = append(differences, difference)
					}
					sort.Strings(differences)
					t.Logf("metadata field differences=%v; consistent identity relations=%d", differences, len(relations.forward))
				})
			}
		}
	}
}

func nativeRootMetadataDifference(path string) bool {
	switch path {
	case "added:client_metadata.x-codex-turn-state", "removed:client_metadata.x-codex-turn-state",
		"changed:client_metadata.x-codex-turn-metadata.turn_started_at_unix_ms",
		"added:input.item_metadata.create_time", "added:input.item_metadata.turn_id":
		return true
	}
	return false
}
