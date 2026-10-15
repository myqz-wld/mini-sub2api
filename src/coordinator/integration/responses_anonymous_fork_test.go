package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

type anonymousForkResult struct {
	status int
	body   []byte
	err    error
}

func TestAnonymousHTTPConflictsForkAndSendEachRequestOnce(t *testing.T) {
	for _, model := range []string{"gpt-5.4", "gpt-6-astra"} {
		for _, stream := range []bool{false, true} {
			for _, identical := range []bool{false, true} {
				t.Run(fmt.Sprintf("%s/stream=%t/identical=%t", model, stream, identical), func(t *testing.T) {
					var calls atomic.Int32
					started := make(chan int, 2)
					releaseFirst, releaseSecond := make(chan struct{}), make(chan struct{})
					finishFirst := sync.OnceFunc(func() { close(releaseFirst) })
					finishSecond := sync.OnceFunc(func() { close(releaseSecond) })
					fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
						n := int(calls.Add(1))
						id := fmt.Sprintf("resp_anonymous_fork_%d", n)
						w.Header().Set("Content-Type", "text/event-stream")
						writeAnonymousForkEvent(w, map[string]any{"type": "response.created", "response": map[string]any{"id": id}})
						if n == 2 || n == 3 {
							started <- n
							release := releaseFirst
							if n == 3 {
								release = releaseSecond
							}
							<-release
						}
						var output map[string]any
						if n == 1 {
							output = map[string]any{"type": "function_call", "id": "fc_fork", "call_id": "call_fork", "name": "tool", "arguments": "{}", "status": "completed"}
						} else {
							output = map[string]any{"type": "message", "id": fmt.Sprintf("msg_fork_%d", n), "role": "assistant", "status": "completed", "content": []any{map[string]any{"type": "output_text", "text": fmt.Sprintf("synthetic branch answer %d", n)}}}
						}
						writeAnonymousForkEvent(w, map[string]any{"type": "response.output_item.done", "output_index": 0, "item": output})
						writeAnonymousForkEvent(w, map[string]any{"type": "response.completed", "response": map[string]any{"id": id, "output": []any{output}}})
						return id, ""
					})
					// Release blocked loopback handlers before the fixture closes on failure.
					defer finishFirst()
					defer finishSecond()
					ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
					defer cancel()
					seedInput := map[string]any{"role": "user", "content": "synthetic fork task"}
					seedBody := mustRequestJSON(t, map[string]any{"model": model, "input": []any{seedInput}, "stream": false})
					seed := readAnonymousForkResult(t, sendAnonymousForkRequest(ctx, fixture, seedBody))
					waitForRoutingCapture(t, fixture.captures)
					call := seed["output"].([]any)[0].(map[string]any)
					input := []any{seedInput, call, map[string]any{"type": "function_call_output", "call_id": call["call_id"], "output": "synthetic result"}}
					original := mustRequestJSON(t, map[string]any{"model": model, "input": input, "stream": stream})
					firstResult := make(chan anonymousForkResult, 1)
					go func() { firstResult <- sendAnonymousForkRequest(ctx, fixture, original) }()
					waitAnonymousForkStart(t, ctx, started, 2)
					incoming := original
					if !identical {
						input = append(input, map[string]any{"role": "user", "content": "[Request interrupted by user]"}, map[string]any{"role": "user", "content": "synthetic correction"})
						incoming = mustRequestJSON(t, map[string]any{"model": model, "input": input, "stream": stream})
					}
					secondResult := make(chan anonymousForkResult, 1)
					go func() { secondResult <- sendAnonymousForkRequest(ctx, fixture, incoming) }()
					waitAnonymousForkStart(t, ctx, started, 3)
					if calls.Load() != 3 {
						t.Fatal("overlap triggered an extra upstream request")
					}
					finishSecond()
					second := readAnonymousForkResult(t, <-secondResult)
					waitForRoutingCapture(t, fixture.captures)
					select {
					case <-firstResult:
						t.Fatal("forking or completing the new request ended the original")
					default:
					}
					finishFirst()
					first := readAnonymousForkResult(t, <-firstResult)
					waitForRoutingCapture(t, fixture.captures)
					if first["id"] == second["id"] {
						t.Fatal("independent requests shared a response")
					}
					for index, response := range []map[string]any{first, second} {
						next := mustRequestJSON(t, map[string]any{"model": model, "previous_response_id": response["id"], "input": "synthetic follow-up", "stream": false})
						readAnonymousForkResult(t, sendAnonymousForkRequest(ctx, fixture, next))
						captured := waitForRoutingCapture(t, fixture.captures)
						own, foreign := index+2, 3-index
						if !bytes.Contains(captured.Body, []byte(fmt.Sprintf("synthetic branch answer %d", own))) || bytes.Contains(captured.Body, []byte(fmt.Sprintf("synthetic branch answer %d", foreign))) {
							t.Fatal("response-reference continuation mixed branch histories")
						}
					}
					readAnonymousForkResult(t, sendAnonymousForkRequest(ctx, fixture, original))
					if calls.Load() != 6 {
						t.Fatal("completed identical request was replayed or retried upstream")
					}
				})
			}
		}
	}
}

func writeAnonymousForkEvent(w http.ResponseWriter, event map[string]any) {
	data, _ := json.Marshal(event)
	fmt.Fprintf(w, "data: %s\n\n", data)
	if flusher, ok := w.(http.Flusher); ok {
		flusher.Flush()
	}
}

func sendAnonymousForkRequest(ctx context.Context, fixture *responsesProfileHTTPFixture, body []byte) anonymousForkResult {
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, fixture.public.URL+"/v1/responses", bytes.NewReader(body))
	if err != nil {
		return anonymousForkResult{err: err}
	}
	request.Header.Set("Authorization", "Bearer "+fixture.subscriptionKey)
	request.Header.Set("Content-Type", "application/json")
	response, err := fixture.public.Client().Do(request)
	if err != nil {
		return anonymousForkResult{err: err}
	}
	defer response.Body.Close()
	data, err := io.ReadAll(response.Body)
	return anonymousForkResult{status: response.StatusCode, body: data, err: err}
}

func readAnonymousForkResult(t *testing.T, result anonymousForkResult) map[string]any {
	t.Helper()
	if result.err != nil || result.status != http.StatusOK {
		t.Fatalf("anonymous request failed: status=%d error=%v", result.status, result.err)
	}
	var response map[string]any
	if json.Unmarshal(result.body, &response) == nil {
		return response
	}
	for _, line := range strings.Split(string(result.body), "\n") {
		if !strings.HasPrefix(line, "data: ") {
			continue
		}
		var event map[string]any
		if json.Unmarshal([]byte(strings.TrimPrefix(line, "data: ")), &event) == nil && event["type"] == "response.completed" {
			return event["response"].(map[string]any)
		}
	}
	t.Fatal("missing completed response")
	return nil
}

func waitAnonymousForkStart(t *testing.T, ctx context.Context, started <-chan int, want int) {
	t.Helper()
	select {
	case got := <-started:
		if got != want {
			t.Fatalf("upstream call order = %d, want %d", got, want)
		}
	case <-ctx.Done():
		t.Fatal("anonymous request did not reach loopback upstream")
	}
}
