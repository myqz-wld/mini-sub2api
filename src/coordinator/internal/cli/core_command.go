package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os/exec"
	"path/filepath"
)

type coreCredentialMetadata struct {
	AccountRef        string  `json:"accountRef"`
	AuthKind          string  `json:"authKind"`
	UpstreamAccountID *string `json:"upstreamAccountId"`
	Status            string  `json:"status"`
}

func runCoreCredential(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	input io.Reader,
	errorOutput io.Writer,
) (coreCredentialMetadata, error) {
	binary, err := resolveCoreBinary(options.coreBinary)
	if err != nil {
		return coreCredentialMetadata{}, err
	}
	command := exec.CommandContext(ctx, binary, append(
		[]string{"credential"}, arguments...,
	)...)
	command.Stdin = input
	command.Stderr = errorOutput
	var output bytes.Buffer
	command.Stdout = &output
	if err := command.Run(); err != nil {
		return coreCredentialMetadata{}, fmt.Errorf("Codex core credential command failed: %w", err)
	}
	if output.Len() > 64*1024 {
		return coreCredentialMetadata{}, fmt.Errorf("Codex core credential output is too large")
	}
	var metadata coreCredentialMetadata
	if err := json.Unmarshal(output.Bytes(), &metadata); err != nil {
		return coreCredentialMetadata{}, fmt.Errorf("decode Codex core credential output: %w", err)
	}
	return metadata, nil
}

func coreStateDirectory(options globalOptions) string {
	return filepath.Join(options.stateDir, "core-codex")
}

func removeCoreCredential(
	ctx context.Context,
	options globalOptions,
	accountRef string,
	errorOutput io.Writer,
) error {
	_, err := runCoreCredential(ctx, options, []string{
		"remove", "--state-dir", coreStateDirectory(options), accountRef,
	}, nil, errorOutput)
	return err
}
