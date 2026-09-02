package httpapi

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"

	"mini-sub2api/src/coordinator/internal/adapter"
	"mini-sub2api/src/coordinator/internal/storage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

func TestHTTPResponseTerminalsRecordUpstreamErrors(t *testing.T) {
	tests := []struct {
		name    string
		headers http.Header
		body    string
	}{
		{
			name:    "sse_failed",
			headers: http.Header{"Content-Type": []string{"text/event-stream"}},
			body:    `data: {"type":"response.failed","response":{"usage":{"total_tokens":2}}}` + "\n\n",
		},
		{
			name:    "sse_incomplete",
			headers: http.Header{"Content-Type": []string{"text/event-stream"}},
			body:    `data: {"type":"response.incomplete","response":{}}`,
		},
		{
			name:    "sse_error",
			headers: http.Header{"Content-Type": []string{"text/event-stream"}},
			body:    `data: {"type":"error","error":{"message":"opaque"}}` + "\n\n",
		},
		{
			name: "aggregated_failed",
			headers: http.Header{
				"Content-Type": []string{"application/json"},
				http.CanonicalHeaderKey(protocolv1.ResponseTerminalHeader): []string{
					protocolv1.ResponseTerminalFailed,
				},
			},
			body: `{"id":"resp_failed","usage":{"total_tokens":2}}`,
		},
		{
			name: "aggregated_incomplete",
			headers: http.Header{
				"Content-Type": []string{"application/json"},
				http.CanonicalHeaderKey(protocolv1.ResponseTerminalHeader): []string{
					protocolv1.ResponseTerminalIncomplete,
				},
			},
			body: `{"id":"resp_incomplete"}`,
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			store, _, key := setupHTTPTest(t)
			core := &fakeCore{response: func(context.Context, string) (*http.Response, error) {
				return &http.Response{
					StatusCode: http.StatusOK,
					Header:     test.headers.Clone(),
					Body:       io.NopCloser(bytes.NewBufferString(test.body)),
				}, nil
			}}
			server := httptest.NewServer(NewHandler(store, core, nil))
			t.Cleanup(server.Close)
			request, err := http.NewRequest(
				http.MethodPost, server.URL+"/v1/responses", bytes.NewBufferString(`{"stream":true}`),
			)
			if err != nil {
				t.Fatal(err)
			}
			request.Header.Set("Authorization", "Bearer "+key.Secret)
			response, err := server.Client().Do(request)
			if err != nil {
				t.Fatal(err)
			}
			_, readErr := io.ReadAll(response.Body)
			response.Body.Close()
			if readErr != nil || response.StatusCode != http.StatusOK {
				t.Fatalf("response = %d, %v", response.StatusCode, readErr)
			}
			if response.Header.Get(protocolv1.ResponseTerminalHeader) != "" {
				t.Fatalf("private terminal header crossed: %#v", response.Header)
			}
			history := waitForHistory(t, store, key.ID, 1)
			if history[0].Status != storage.RequestUpstreamErr {
				t.Fatalf("history = %#v", history)
			}
			stats := waitForStats(t, store, key.ID)
			if len(stats) != 1 || stats[0].CompletedCount != 0 || stats[0].ErrorCount != 1 {
				t.Fatalf("stats = %#v", stats)
			}
		})
	}
}

func TestCoreUnavailableFailureMetadataReflectsSendBoundary(t *testing.T) {
	tests := []struct {
		name          string
		forwardError  error
		status        int
		code          string
		retryAdvice   string
		phase         string
		deliveryState string
	}{
		{
			name: "supervisor_has_no_core", forwardError: adapter.ErrUnavailable,
			status: http.StatusServiceUnavailable, code: "adapter_unavailable",
			retryAdvice: "safe", phase: "internal", deliveryState: "not_delivered",
		},
		{
			name: "internal_transport_failed", forwardError: errors.New("write failed"),
			status: http.StatusBadGateway, code: "upstream_unavailable",
			retryAdvice: "ambiguous", phase: "upstream_request", deliveryState: "possibly_delivered",
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			store, _, key := setupHTTPTest(t)
			core := &fakeCore{response: func(context.Context, string) (*http.Response, error) {
				return nil, test.forwardError
			}}
			server := httptest.NewServer(NewHandler(store, core, nil))
			t.Cleanup(server.Close)
			request, err := http.NewRequest(
				http.MethodPost, server.URL+"/v1/responses", bytes.NewBufferString(`{}`),
			)
			if err != nil {
				t.Fatal(err)
			}
			request.Header.Set("Authorization", "Bearer "+key.Secret)
			response, err := server.Client().Do(request)
			if err != nil {
				t.Fatal(err)
			}
			defer response.Body.Close()
			var envelope struct {
				Error map[string]any `json:"error"`
			}
			if err := json.NewDecoder(response.Body).Decode(&envelope); err != nil {
				t.Fatal(err)
			}
			if response.StatusCode != test.status || envelope.Error["code"] != test.code ||
				envelope.Error["retryAdvice"] != test.retryAdvice ||
				envelope.Error["phase"] != test.phase ||
				envelope.Error["deliveryState"] != test.deliveryState {
				t.Fatalf("failure = %d %#v", response.StatusCode, envelope.Error)
			}
		})
	}
}
