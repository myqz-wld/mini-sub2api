package integration

import (
	"fmt"
	"io"
	"net/http"
	"strings"

	"github.com/klauspost/compress/zstd"
)

func readCapturedUpstreamBody(request *http.Request) ([]byte, error) {
	body, err := io.ReadAll(request.Body)
	if err != nil {
		return nil, err
	}
	switch strings.ToLower(strings.TrimSpace(request.Header.Get("Content-Encoding"))) {
	case "", "identity":
		return body, nil
	case "zstd":
		decoder, err := zstd.NewReader(nil, zstd.WithDecoderConcurrency(1))
		if err != nil {
			return nil, fmt.Errorf("create zstd decoder: %w", err)
		}
		defer decoder.Close()
		decoded, err := decoder.DecodeAll(body, nil)
		if err != nil {
			return nil, fmt.Errorf("decode zstd request body: %w", err)
		}
		return decoded, nil
	default:
		return nil, fmt.Errorf("unsupported request content encoding %q", request.Header.Get("Content-Encoding"))
	}
}
