package protocolv1

import (
	"encoding/json"
	"os"
	"testing"
)

func TestSSEProgressFixture(t *testing.T) {
	data, err := os.ReadFile("../fixtures/sse_progress.json")
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct {
		Name   string
		Data   string
		Class  string
		Output bool
	}
	if err := json.Unmarshal(data, &cases); err != nil {
		t.Fatal(err)
	}
	for _, tc := range cases {
		t.Run(tc.Name, func(t *testing.T) {
			got := ClassifySSEData([]byte(tc.Data))
			if got.String() != tc.Class || got.IsOutput() != tc.Output {
				t.Fatalf("class=%s output=%t; want class=%s output=%t", got, got.IsOutput(), tc.Class, tc.Output)
			}
		})
	}
}
