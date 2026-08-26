package integration

import (
	"bytes"
	"context"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestResponsesProfileWebSocketOrdinarySecondTurnUsesDelta(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixture(t)
	first := map[string]any{
		"type": "response.create", "model": "gpt-5.4",
		"input": []any{responsesProfileMessage("ordinary-first")},
	}
	second := map[string]any{
		"type": "response.create", "model": "gpt-5.4", "input": []any{
			responsesProfileMessage("ordinary-first"),
			map[string]any{"type": "message", "id": "msg_profile_assistant", "role": "assistant", "content": []any{}},
			responsesProfileMessage("ordinary-second"),
		},
	}

	connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, nil)
	defer connection.CloseNow()
	for _, frame := range [][]byte{mustRequestJSON(t, first), mustRequestJSON(t, second)} {
		writeE2EWebSocketText(t, connection, string(frame))
		readResponsesProfileTerminalEvents(t, connection)
	}

	captures := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 3)
	hidden := decodeResponsesProfileWebSocketFrame(t, captures[0].Frame)
	if hidden["generate"] != false || len(hidden["input"].([]any)) != 0 {
		t.Fatal("minimal caller hidden setup was not an empty prewarm")
	}
	publicFirst := decodeResponsesProfileWebSocketFrame(t, captures[1].Frame)
	publicSecond := decodeResponsesProfileWebSocketFrame(t, captures[2].Frame)
	if publicSecond["previous_response_id"] != captures[1].ResponseID {
		t.Fatal("ordinary second turn did not reference the first public response")
	}
	input, ok := publicSecond["input"].([]any)
	if !ok || len(input) != 1 || !bytes.Contains(captures[2].Frame, []byte("ordinary-second")) {
		t.Fatalf("ordinary second-turn delta = %#v", publicSecond["input"])
	}
	assertResponsesProfileSocketIdentityStable(t, publicFirst, publicSecond)
}

func TestResponsesProfileWebSocketDeferredSetupHonorsCancellationAndOverlap(t *testing.T) {
	for _, test := range []struct {
		name    string
		overlap bool
	}{
		{name: "downstream_cancel"},
		{name: "overlapping_create", overlap: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			hiddenSeen := make(chan struct{}, 1)
			release := make(chan struct{})
			fixture := newResponsesProfileWebSocketFixtureWithResponder(
				t,
				func(connection *websocket.Conn, payload []byte, responseID string) {
					if isSyntheticResponsesProfilePrewarm(payload) {
						hiddenSeen <- struct{}{}
						<-release
						writeResponsesProfileEvents(connection, responseID, true)
						return
					}
					writeResponsesProfileEvents(connection, responseID, false)
				},
			)
			connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, nil)
			frame := responsesProfileRequest("gpt-5.4", []any{responsesProfileMessage("deferred")})
			delete(frame, "conversation")
			frame["type"] = "response.create"
			encoded := mustRequestJSON(t, frame)
			writeE2EWebSocketText(t, connection, string(encoded))
			select {
			case <-hiddenSeen:
			case <-time.After(2 * time.Second):
				t.Fatal("hidden setup was not observed")
			}
			if test.overlap {
				writeE2EWebSocketText(t, connection, string(encoded))
				readContext, cancel := context.WithTimeout(context.Background(), 2*time.Second)
				defer cancel()
				_, _, err := connection.Read(readContext)
				if websocket.CloseStatus(err) != websocket.StatusPolicyViolation {
					t.Fatalf("overlap close = %v", err)
				}
				<-time.After(50 * time.Millisecond)
			} else {
				connection.CloseNow()
				<-time.After(50 * time.Millisecond)
			}
			close(release)
			captures := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)
			assertResponsesProfilePrewarm(t, captures[0], false)
			select {
			case capture := <-fixture.captures:
				t.Fatalf("public create crossed cancelled deferred setup: %s", capture.ResponseID)
			case <-time.After(300 * time.Millisecond):
			}
		})
	}
}

func TestResponsesProfileWebSocketDeferredPendingQueueIsBounded(t *testing.T) {
	hiddenSeen := make(chan struct{}, 1)
	release := make(chan struct{})
	defer close(release)
	fixture := newResponsesProfileWebSocketFixtureWithResponder(
		t,
		func(connection *websocket.Conn, payload []byte, responseID string) {
			if isSyntheticResponsesProfilePrewarm(payload) {
				hiddenSeen <- struct{}{}
				<-release
				writeResponsesProfileEvents(connection, responseID, true)
			}
		},
	)
	connection := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, nil)
	defer connection.CloseNow()
	first := mustRequestJSON(t, map[string]any{
		"type": "response.create", "model": "gpt-5.4", "input": []any{},
	})
	writeE2EWebSocketText(t, connection, string(first))
	select {
	case <-hiddenSeen:
	case <-time.After(2 * time.Second):
		t.Fatal("hidden setup was not observed")
	}

	readContext, cancelRead := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancelRead()
	closed := make(chan error, 1)
	go func() {
		_, _, err := connection.Read(readContext)
		closed <- err
	}()
	control := []byte(`{"type":"response.cancel"}`)
	for index := 0; index <= 1024; index++ {
		writeContext, cancelWrite := context.WithTimeout(context.Background(), 2*time.Second)
		err := connection.Write(writeContext, websocket.MessageText, control)
		cancelWrite()
		if err != nil {
			break
		}
	}
	if err := <-closed; websocket.CloseStatus(err) != websocket.StatusMessageTooBig {
		t.Fatalf("pending queue close = %v", err)
	}
	captures := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)
	hidden := decodeResponsesProfileWebSocketFrame(t, captures[0].Frame)
	if hidden["generate"] != false {
		t.Fatal("pending queue test did not capture hidden setup")
	}
	select {
	case capture := <-fixture.captures:
		t.Fatalf("public create crossed the pending queue bound: %s", capture.ResponseID)
	case <-time.After(200 * time.Millisecond):
	}
}

func assertResponsesProfileSocketIdentityStable(t *testing.T, first, second map[string]any) {
	t.Helper()
	if first["prompt_cache_key"] == "" || first["prompt_cache_key"] != second["prompt_cache_key"] {
		t.Fatal("socket prompt_cache_key was not stable")
	}
	firstMetadata, firstOK := first["client_metadata"].(map[string]any)
	secondMetadata, secondOK := second["client_metadata"].(map[string]any)
	if !firstOK || !secondOK {
		t.Fatal("socket client_metadata was not synthesized")
	}
	for _, field := range []string{"session_id", "thread_id", "x-codex-installation-id"} {
		if firstMetadata[field] == "" || firstMetadata[field] != secondMetadata[field] {
			t.Fatalf("socket %s identity was not stable", field)
		}
	}
}
