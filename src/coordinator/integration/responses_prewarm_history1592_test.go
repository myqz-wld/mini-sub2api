package integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"testing"
	"time"

	"github.com/coder/websocket"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

const prewarm1592Metadata = "internal_chat_message_metadata_passthrough"

func prewarm1592Message(role, turn, text string) map[string]any {
	contentType := "input_text"
	if role == "assistant" {
		contentType = "output_text"
	}
	return map[string]any{"type": "message", "role": role,
		"content":           []any{map[string]any{"type": contentType, "text": text}},
		prewarm1592Metadata: map[string]any{"turn_id": turn}}
}

func prewarm1592Request(t *testing.T, model, session, turn string, input []any, prewarm bool) map[string]any {
	t.Helper()
	kind := "turn"
	if prewarm {
		kind = "prewarm"
	}
	body := map[string]any{"type": "response.create", "model": model, "input": input,
		"instructions": "Synthetic caller base", "tools": []any{},
		"client_metadata": map[string]any{"session_id": session, "thread_id": session, "turn_id": turn,
			"x-codex-turn-metadata": string(mustRequestJSON(t, map[string]any{
				"request_kind": kind, "session_id": session, "thread_id": session, "turn_id": turn,
			}))}}
	if prewarm {
		body["generate"] = false
	}
	return body
}

func prewarm1592Fixture(t *testing.T) responsesProfileWebSocketFixture {
	t.Helper()
	return newResponsesProfileWebSocketFixtureWithResponder(t, func(c *websocket.Conn, body []byte, id string) {
		if isSyntheticResponsesProfilePrewarm(body) {
			writeResponsesProfileEvents(c, id, true)
			return
		}
		for _, event := range historyImportEvents(id, historyImportTurn(t, body)) {
			_ = c.Write(context.Background(), websocket.MessageText, mustRequestJSONValue(event))
		}
	})
}

func prewarm1592Exchange(t *testing.T, c *websocket.Conn, fixture responsesProfileWebSocketFixture, body map[string]any) (map[string]any, responsesProfileWebSocketCapture) {
	t.Helper()
	writeE2EWebSocketText(t, c, string(mustRequestJSON(t, body)))
	response := historyImportCompleted(t, readResponsesProfileTerminalEvents(t, c))
	capture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
	return response, capture
}

func assertPrewarm1592Identity(t *testing.T, wire map[string]any) {
	t.Helper()
	metadata, _ := wire["client_metadata"].(map[string]any)
	var turn map[string]any
	raw, _ := metadata["x-codex-turn-metadata"].(string)
	if json.Unmarshal([]byte(raw), &turn) != nil || turn["request_kind"] != "prewarm" || turn["turn_id"] != "" || metadata["turn_id"] != "" {
		t.Fatal("prewarm request did not retain its empty turn identity")
	}
	for _, field := range []string{"root_turn_id", "parent_turn_id", "turn_started_at_unix_ms"} {
		if _, ok := turn[field]; ok {
			t.Fatalf("prewarm unexpectedly has %s", field)
		}
		if _, ok := metadata[field]; ok {
			t.Fatalf("prewarm client metadata unexpectedly has %s", field)
		}
	}
	if wire["generate"] != false || wire["previous_response_id"] != nil {
		t.Fatal("history prewarm must send full input with generate=false")
	}
}

func TestResponsesHistoryPrewarm1592(t *testing.T) {
	for _, model := range []string{"gpt-5.5", "gpt-6-sol"} {
		for _, historyKind := range []string{"base", "unknown", "known_reconnect"} {
			t.Run(model+"/"+historyKind, func(t *testing.T) {
				fixture := prewarm1592Fixture(t)
				headers := http.Header{"Originator": []string{"codex_exec"}}
				conn := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, headers)
				defer func() { _ = conn.CloseNow() }()
				history := []any{}
				knownTurns := map[string]string{}
				if historyKind == "known_reconnect" {
					for _, turn := range []string{"history-first", "history-second"} {
						history = append(history, prewarm1592Message("user", turn, "Synthetic "+turn))
						response, capture := prewarm1592Exchange(t, conn, fixture,
							prewarm1592Request(t, model, "history-session", turn, history, false))
						knownTurns[turn] = historyImportTurn(t, capture.Frame)
						output, _ := response["output"].([]any)
						if len(output) != 1 {
							t.Fatal("synthetic history response did not provide one complete item")
						}
						history = append(history, output...)
					}
					_ = conn.CloseNow()
					conn = dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, headers)
				} else if historyKind == "unknown" {
					for _, turn := range []string{"import-first", "import-second"} {
						history = append(history, prewarm1592Message("user", turn, "Synthetic "+turn),
							prewarm1592Message("assistant", turn, "Synthetic prior answer"))
					}
				}
				warmResponse, warmCapture := prewarm1592Exchange(t, conn, fixture,
					prewarm1592Request(t, model, "history-session", "", history, true))
				warm := decodeRequestObject(t, warmCapture.Frame)
				assertPrewarm1592Identity(t, warm)
				items, _ := warm["input"].([]any)
				prefix := len(items) - len(history)
				if (model == "gpt-6-sol" && prefix != 2) || (model == "gpt-5.5" && prefix != 0) {
					t.Fatal("prewarm changed the ordinary/Lite base setup")
				}
				for _, raw := range items[:prefix] {
					item := raw.(map[string]any)
					if _, ok := item[prewarm1592Metadata]; ok {
						t.Fatal("generated native Lite setup acquired item turn metadata")
					}
				}
				projected := map[string]string{}
				for i, original := range history {
					rawTurn := original.(map[string]any)[prewarm1592Metadata].(map[string]any)["turn_id"].(string)
					item := items[prefix+i].(map[string]any)
					metadata, _ := item[prewarm1592Metadata].(map[string]any)
					turn, _ := metadata["turn_id"].(string)
					if turn == "" || turn == rawTurn || (projected[rawTurn] != "" && projected[rawTurn] != turn) {
						t.Fatal("historical item turn was lost or projected inconsistently")
					}
					if known := knownTurns[rawTurn]; known != "" && turn != known {
						t.Fatal("known historical turn changed after reconnect prewarm")
					}
					projected[rawTurn] = turn
				}
				if len(history) > 0 && (len(projected) != 2 || projectedTurnCount1592(projected) != 2) {
					t.Fatal("prewarm collapsed distinct historical turns")
				}
				// Native resumes through the response returned by explicit history prewarm.
				follow := prewarm1592Request(t, model, "history-session", "follow-up",
					[]any{prewarm1592Message("user", "follow-up", "Synthetic continuation")}, false)
				follow["previous_response_id"] = warmResponse["id"]
				response, followCapture := prewarm1592Exchange(t, conn, fixture, follow)
				wire := decodeRequestObject(t, followCapture.Frame)
				delta, _ := wire["input"].([]any)
				if wire["previous_response_id"] != warmCapture.ResponseID || len(delta) != 1 {
					t.Fatal("native follow-up did not reuse the prewarm prefix")
				}
				if historyImportTurn(t, followCapture.Frame) == "" {
					t.Fatal("follow-up sampling has no current turn")
				}
				next := prewarm1592Request(t, model, "history-session", "next-turn",
					[]any{prewarm1592Message("user", "next-turn", "Synthetic explicit continuation")}, false)
				next["previous_response_id"] = response["id"]
				_, explicitCapture := prewarm1592Exchange(t, conn, fixture, next)
				explicit := decodeRequestObject(t, explicitCapture.Frame)
				if explicit["previous_response_id"] != followCapture.ResponseID || len(explicit["input"].([]any)) != 1 {
					t.Fatal("explicit continuation after history prewarm did not keep the same socket baseline")
				}
			})
		}
	}
}

func projectedTurnCount1592(turns map[string]string) int {
	unique := map[string]bool{}
	for _, turn := range turns {
		unique[turn] = true
	}
	return len(unique)
}

func TestResponsesHistoryPrewarm1592RejectsWrongScope(t *testing.T) {
	for _, model := range []string{"gpt-5.5", "gpt-6-sol"} {
		for _, boundary := range []string{"unrelated_thread", "other_key_response"} {
			t.Run(model+"/"+boundary, func(t *testing.T) {
				fixture := prewarm1592Fixture(t)
				headers := http.Header{"Originator": []string{"codex_exec"}}
				source := dialResponsesProfileWebSocket(t, fixture.public, fixture.subscriptionKey, headers)
				defer source.CloseNow()
				input := []any{prewarm1592Message("user", "source-turn", "Synthetic owned history")}
				response, _ := prewarm1592Exchange(t, source, fixture,
					prewarm1592Request(t, model, "source-session", "source-turn", input, false))
				input = append(input, response["output"].([]any)...)
				secret := fixture.subscriptionKey
				body := prewarm1592Request(t, model, "unrelated-session", "", input, true)
				if boundary == "other_key_response" {
					route, err := fixture.store.AuthenticateConnection(context.Background(), fixture.subscriptionKey)
					if err != nil {
						t.Fatal("resolve synthetic credential binding")
					}
					secret = createDownstreamKey(t, fixture.store, route.CredentialID, "Synthetic second key").Secret
					body = prewarm1592Request(t, model, "source-session", "", input, true)
					body["previous_response_id"] = response["id"]
				}
				target := dialResponsesProfileWebSocket(t, fixture.public, secret, headers)
				defer target.CloseNow()
				writeE2EWebSocketText(t, target, string(mustRequestJSON(t, body)))
				ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
				defer cancel()
				wantClose := websocket.StatusProtocolError
				if boundary == "other_key_response" {
					wantClose = websocket.StatusCode(protocolv1.FailureCloseCode)
				}
				for {
					_, raw, err := target.Read(ctx)
					if err != nil {
						if websocket.CloseStatus(err) != wantClose {
							t.Fatalf("wrong-scope prewarm close status=%d", websocket.CloseStatus(err))
						}
						break
					}
					var event map[string]any
					if json.Unmarshal(raw, &event) != nil || event["type"] != "error" {
						t.Fatal("wrong-scope prewarm delivered inference output")
					}
				}
				select {
				case <-fixture.captures:
					t.Fatal("wrong-scope prewarm reached upstream")
				default:
				}
			})
		}
	}
}

func TestResponsesHistoryPrewarm1592APIKeyTransparent(t *testing.T) {
	fixture := newResponsesProfileWebSocketFixture(t)
	for _, model := range []string{"gpt-5.5", "gpt-6-sol"} {
		t.Run(model, func(t *testing.T) {
			conn := dialResponsesProfileWebSocket(t, fixture.public, fixture.apiKey,
				http.Header{"Originator": []string{"codex_exec"}})
			defer conn.CloseNow()
			body := prewarm1592Request(t, model, "api-session", "",
				[]any{prewarm1592Message("user", "api-turn", fmt.Sprintf("Synthetic %s history", model))}, true)
			encoded := mustRequestJSON(t, body)
			writeE2EWebSocketText(t, conn, string(encoded))
			readResponsesProfileTerminalEvents(t, conn)
			capture := waitForResponsesProfileWebSocketCaptures(t, fixture.captures, 1)[0]
			if !bytes.Equal(capture.Frame, encoded) {
				t.Fatal("API-key history prewarm frame changed")
			}
		})
	}
}
