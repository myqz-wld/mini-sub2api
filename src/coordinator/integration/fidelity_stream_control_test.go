package integration

import (
	"context"
	"fmt"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestFidelityFollowupErrorWaitsForFailedFooter(t *testing.T) {
	for _, delay := range []time.Duration{0, 1250 * time.Millisecond} {
		t.Run(delay.String(), func(t *testing.T) {
			fixture := newResponsesProfileHTTPFixtureWithResponder(t, func(w http.ResponseWriter, _ []byte) (string, string) {
				w.Header().Set("Content-Type", "text/event-stream")
				fmt.Fprint(w, "data: {\"type\":\"error\",\"code\":\"invalid_prompt\",\"message\":\"synthetic\"}\n\n")
				w.(http.Flusher).Flush()
				time.Sleep(delay)
				fmt.Fprint(w, "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_delayed_footer\",\"status\":\"failed\",\"error\":{\"code\":\"invalid_prompt\",\"message\":\"synthetic\"},\"output\":[]}}\n\n")
				return "resp_delayed_footer", "synthetic-provider"
			})
			status, body, _ := publicRequestWithHeaders(t, fixture.public, fixture.subscriptionKey, `{"model":"gpt-5.4","input":"synthetic","stream":true}`, nil)
			if status != http.StatusOK || !strings.Contains(string(body), `"type":"error"`) || !strings.Contains(string(body), `"type":"response.failed"`) {
				t.Fatal("complete error/failed sequence was not delivered")
			}
			waitForRoutingCapture(t, fixture.captures)
		})
	}
}

func TestFidelityFollowupWebSocketApplicationControlsAreIgnored(t *testing.T) {
	release := make(chan struct{})
	defer func() {
		select {
		case <-release:
		default:
			close(release)
		}
	}()
	fixture := newResponsesProfileWebSocketFixtureWithResponder(t, func(conn *websocket.Conn, _ []byte, id string) {
		<-release
		_ = conn.Write(context.Background(), websocket.MessageText, mustRequestJSON(t, map[string]any{"type": "response.completed", "response": map[string]any{"id": id, "output": []any{}}}))
	})
	conn, _ := dialResponsesProfileWebSocketWithResponse(t, fixture.public, fixture.subscriptionKey, http.Header{"Originator": []string{"codex_exec"}})
	defer conn.CloseNow()
	create := `{"type":"response.create","model":"gpt-5.4","input":"synthetic"}`
	writeE2EWebSocketText(t, conn, create)
	waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)
	for _, kind := range []string{"response.ping", "response.append_input_item", "response.cancel", "synthetic.unknown"} {
		writeE2EWebSocketText(t, conn, fmt.Sprintf(`{"type":%q,"extra_probe":true}`, kind))
	}
	// Ping needs an active reader. A second valid create proves the ignored controls neither
	// closed the connection nor invalidated the ordinary continuation state.
	readDone := make(chan struct{})
	go func() { defer close(readDone); readResponsesProfileTerminalEvents(t, conn) }()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if err := conn.Ping(ctx); err != nil {
		t.Fatal("protocol heartbeat failed")
	}
	close(release)
	select {
	case <-readDone:
	case <-ctx.Done():
		t.Fatal("first create timed out")
	}
	writeE2EWebSocketText(t, conn, create)
	readResponsesProfileTerminalEvents(t, conn)
	capture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
	if decodeRequestObject(t, capture.Frame)["type"] != "response.create" {
		t.Fatal("application control reached upstream")
	}
	select {
	case <-fixture.captures:
		t.Fatal("extra application frame reached upstream")
	case <-time.After(50 * time.Millisecond):
	}
	_ = conn.Close(websocket.StatusNormalClosure, "done")
}
