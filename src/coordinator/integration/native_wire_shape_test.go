//go:build nativeparity

package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"reflect"
	"sort"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"
)

type nativeJSONShape struct {
	kind   string
	scalar any
	keys   []string
	fields map[string]*nativeJSONShape
	items  []*nativeJSONShape
}

func readNativeJSONShape(t *testing.T, data []byte) *nativeJSONShape {
	t.Helper()
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	var read func(string) *nativeJSONShape
	read = func(key string) *nativeJSONShape {
		token, err := decoder.Token()
		if err != nil {
			t.Fatal("invalid captured JSON shape")
		}
		node := &nativeJSONShape{}
		switch value := token.(type) {
		case json.Delim:
			if value == '{' {
				node.kind = "object"
				node.fields = map[string]*nativeJSONShape{}
				for decoder.More() {
					field, err := decoder.Token()
					name, ok := field.(string)
					if err != nil || !ok {
						t.Fatal("invalid captured JSON field")
					}
					if _, exists := node.fields[name]; exists {
						t.Fatal("duplicate captured JSON field")
					}
					node.keys = append(node.keys, name)
					node.fields[name] = read(name)
				}
			} else if value == '[' {
				node.kind = "array"
				for decoder.More() {
					node.items = append(node.items, read(""))
				}
			} else {
				t.Fatal("unexpected captured JSON delimiter")
			}
			if _, err := decoder.Token(); err != nil {
				t.Fatal("unterminated captured JSON container")
			}
		case string:
			node.scalar = value
			node.kind = "string"
			if value == "" {
				node.kind = "empty_string"
			}
			if key == "x-codex-turn-metadata" {
				node = readNativeJSONShape(t, []byte(value))
				node.kind = "serialized_object"
			}
		case bool:
			node.kind = fmt.Sprint(value)
		case nil:
			node.kind = "null"
		case json.Number:
			node.kind = "number"
		default:
			t.Fatal("unsupported captured JSON token")
		}
		return node
	}
	root := read("")
	if _, err := decoder.Token(); err != io.EOF {
		t.Fatal("trailing captured JSON")
	}
	return root
}

func nativePacketJSON(t *testing.T, packet nativePacket) []byte {
	t.Helper()
	if packet.headers.Get("Content-Encoding") != "zstd" {
		return packet.payload
	}
	decoder, err := zstd.NewReader(nil, zstd.WithDecoderMaxMemory(nativeCaptureLimit), zstd.WithDecoderConcurrency(1))
	if err != nil {
		t.Fatal("shape capture decoder")
	}
	defer decoder.Close()
	body, err := decoder.DecodeAll(packet.payload, nil)
	if err != nil {
		t.Fatal("shape capture decompression")
	}
	return body
}

func nativeShapeDifferences(before, after *nativeJSONShape, path string) []string {
	var differences []string
	if before.kind != after.kind {
		return []string{path + ":type:" + before.kind + "->" + after.kind}
	}
	if before.fields != nil {
		var leftCommon, rightCommon []string
		for _, key := range before.keys {
			if next, ok := after.fields[key]; ok {
				leftCommon = append(leftCommon, key)
				differences = append(differences, nativeShapeDifferences(before.fields[key], next, path+"."+safeNativeShapeKey(key))...)
			} else {
				differences = append(differences, path+":missing:"+safeNativeShapeKey(key))
			}
		}
		for _, key := range after.keys {
			if _, ok := before.fields[key]; ok {
				rightCommon = append(rightCommon, key)
			} else {
				differences = append(differences, path+":added:"+safeNativeShapeKey(key))
			}
		}
		if !reflect.DeepEqual(leftCommon, rightCommon) {
			differences = append(differences, path+":order")
		}
	} else if before.kind == "array" {
		if len(before.items) != len(after.items) {
			return []string{path + ":array_length"}
		}
		for index := range before.items {
			differences = append(differences, nativeShapeDifferences(before.items[index], after.items[index], fmt.Sprintf("%s[%d]", path, index))...)
		}
	}
	sort.Strings(differences)
	return differences
}

func safeNativeShapeKey(key string) string {
	if strings.ContainsAny(key, "/\\@") || len(key) > 80 {
		return "<map-key>"
	}
	return key
}

func assertNativeJSONShape(t *testing.T, before, after []byte) {
	t.Helper()
	left, right := readNativeJSONShape(t, before), readNativeJSONShape(t, after)
	for _, difference := range nativeShapeDifferences(left, right, "$") {
		t.Errorf("native wire JSON shape differs: %s", difference)
	}
}

func TestNativeJSONShapeDetectsOrderPresenceAndEmptyValues(t *testing.T) {
	left := readNativeJSONShape(t, []byte(`{"a":null,"b":false,"c":[],"d":{},"metadata":{"x":1,"y":2}}`))
	right := readNativeJSONShape(t, []byte(`{"b":true,"a":{},"c":[null],"e":{},"metadata":{"y":2,"x":1}}`))
	want := []string{"$:added:e", "$:missing:d", "$:order", "$.a:type:null->object", "$.b:type:false->true", "$.c:array_length", "$.metadata:order"}
	sort.Strings(want)
	if got := nativeShapeDifferences(left, right, "$"); !reflect.DeepEqual(got, want) {
		t.Fatalf("shape difference categories: %v", got)
	}
}
