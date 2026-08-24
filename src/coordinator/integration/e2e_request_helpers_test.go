package integration

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

type capturedRequest struct {
	Authorization string
	AccountID     string
	Originator    string
	Encoding      string
	Organization  string
	Project       string
	SDKLanguage   string
	SDKVersion    string
	SDKUnreviewed string
	Body          string
}

func publicRequest(t *testing.T, server *httptest.Server, secret, body string) (int, string, http.Header) {
	t.Helper()
	return publicRequestWithHeaders(t, server, secret, body, nil)
}

func publicRequestWithHeaders(
	t *testing.T,
	server *httptest.Server,
	secret, body string,
	headers http.Header,
) (int, string, http.Header) {
	t.Helper()
	request, err := http.NewRequest(http.MethodPost, server.URL+"/v1/responses", strings.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("Authorization", "Bearer "+secret)
	request.Header.Set("Content-Type", "application/json")
	for name, values := range headers {
		for _, value := range values {
			request.Header.Add(name, value)
		}
	}
	response, err := server.Client().Do(request)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	responseBody, err := io.ReadAll(response.Body)
	if err != nil {
		t.Fatal(err)
	}
	return response.StatusCode, string(responseBody), response.Header.Clone()
}
