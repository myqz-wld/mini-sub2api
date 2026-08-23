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

type coreFingerprintMetadata struct {
	AccountRef string `json:"accountRef"`
	Mode       string `json:"mode"`
	Revision   uint64 `json:"revision"`
}

func runCoreCredential(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	input io.Reader,
	errorOutput io.Writer,
) (coreCredentialMetadata, error) {
	output, err := runCoreCredentialOutput(ctx, options, arguments, input, errorOutput)
	if err != nil {
		return coreCredentialMetadata{}, err
	}
	var metadata coreCredentialMetadata
	if err := json.Unmarshal(output, &metadata); err != nil {
		return coreCredentialMetadata{}, fmt.Errorf("decode Codex core credential output: %w", err)
	}
	return metadata, nil
}

func runCoreCredentialOutput(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	input io.Reader,
	errorOutput io.Writer,
) ([]byte, error) {
	binary, err := resolveCoreBinary(options.coreBinary)
	if err != nil {
		return nil, err
	}
	command := exec.CommandContext(ctx, binary, append(
		[]string{"credential"}, arguments...,
	)...)
	command.Stdin = input
	command.Stderr = errorOutput
	var output bytes.Buffer
	command.Stdout = &output
	if err := command.Run(); err != nil {
		return nil, fmt.Errorf("Codex core credential command failed: %w", err)
	}
	if output.Len() > 64*1024 {
		return nil, fmt.Errorf("Codex core credential output is too large")
	}
	return output.Bytes(), nil
}

func runCoreFingerprint(
	ctx context.Context,
	options globalOptions,
	accountRef, mode string,
	errorOutput io.Writer,
) (coreFingerprintMetadata, error) {
	arguments := []string{
		"fingerprint", "--state-dir", coreStateDirectory(options), accountRef,
	}
	if mode != "" {
		arguments = append(arguments, "--mode", mode)
	}
	output, err := runCoreCredentialOutput(ctx, options, arguments, nil, errorOutput)
	if err != nil {
		return coreFingerprintMetadata{}, err
	}
	decoder := json.NewDecoder(bytes.NewReader(output))
	decoder.DisallowUnknownFields()
	var metadata coreFingerprintMetadata
	if err := decoder.Decode(&metadata); err != nil {
		return coreFingerprintMetadata{}, fmt.Errorf("decode Codex core fingerprint output: %w", err)
	}
	var trailing any
	if err := decoder.Decode(&trailing); err != io.EOF {
		return coreFingerprintMetadata{}, fmt.Errorf("decode Codex core fingerprint output: trailing data")
	}
	if metadata.AccountRef != accountRef || metadata.Revision == 0 ||
		(metadata.Mode != fingerprintModeOff && metadata.Mode != fingerprintModeDevice) {
		return coreFingerprintMetadata{}, fmt.Errorf("Codex core fingerprint output is invalid")
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
