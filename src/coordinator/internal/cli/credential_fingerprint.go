package cli

import (
	"context"
	"fmt"
	"strings"

	"mini-sub2api/src/coordinator/internal/storage"
)

const (
	fingerprintModeOff    = "off"
	fingerprintModeDevice = "device"
)

type credentialFingerprintResult struct {
	ID       string `json:"id"`
	Mode     string `json:"mode"`
	Revision uint64 `json:"revision"`
	Status   string `json:"status,omitempty"`
}

func credentialFingerprint(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	if requestedHelp(arguments) {
		_, err := fmt.Fprintln(environment.Stdout, "usage: credential fingerprint ID [--mode off|device]")
		return err
	}
	mode, modeSet, remaining, err := takeStringFlag(arguments, "--mode")
	if err != nil {
		return err
	}
	if len(remaining) != 1 {
		return fmt.Errorf("usage: credential fingerprint ID [--mode off|device]")
	}
	if modeSet {
		if err := validateFingerprintMode(mode); err != nil {
			return fmt.Errorf("--mode must be off or device")
		}
	}

	credentialID := remaining[0]
	store, err := openStore(ctx, options)
	if err != nil {
		return err
	}
	defer store.Close()
	credential, err := store.Credential(ctx, credentialID)
	if err != nil {
		return err
	}
	if credential.Status == storage.CredentialDeleted {
		return storage.ErrNotFound
	}
	if !modeSet {
		metadata, err := runCoreFingerprint(
			ctx, options, credential.AccountRef, "", environment.Stderr,
		)
		if err != nil {
			return err
		}
		return writeFingerprintResult(options, environment, credentialID, metadata, false)
	}
	if credential.Status != storage.CredentialDisabled {
		return fmt.Errorf(
			"credential %s must be disabled before changing fingerprint mode",
			credentialID,
		)
	}
	if err := waitForNoInFlight(ctx, store, credentialID); err != nil {
		return err
	}
	var metadata coreFingerprintMetadata
	err = store.WithCredentialMutationFence(ctx, credentialID, func(accountRef string) error {
		var mutationErr error
		metadata, mutationErr = runCoreFingerprint(
			ctx, options, accountRef, mode, environment.Stderr,
		)
		return mutationErr
	})
	if err != nil {
		return fmt.Errorf("change credential fingerprint mode: %w", err)
	}
	return writeFingerprintResult(options, environment, credentialID, metadata, true)
}

func writeFingerprintResult(
	options globalOptions,
	environment Environment,
	credentialID string,
	metadata coreFingerprintMetadata,
	mutated bool,
) error {
	result := credentialFingerprintResult{
		ID: credentialID, Mode: metadata.Mode, Revision: metadata.Revision,
	}
	human := fmt.Sprintf(
		"Credential %s uses fingerprint mode %s (revision %d).",
		credentialID, metadata.Mode, metadata.Revision,
	)
	if mutated {
		result.Status = storage.CredentialDisabled
		human = fmt.Sprintf(
			"Credential %s fingerprint mode is now %s (revision %d); it remains disabled.",
			credentialID, metadata.Mode, metadata.Revision,
		)
	}
	return writeResult(environment.Stdout, options.json, result, human)
}

func validateFingerprintMode(mode string) error {
	if mode != fingerprintModeOff && mode != fingerprintModeDevice {
		return fmt.Errorf("--fingerprint-mode must be off or device")
	}
	return nil
}

func takeStringFlag(arguments []string, name string) (string, bool, []string, error) {
	remaining := make([]string, 0, len(arguments))
	var value string
	found := false
	for index := 0; index < len(arguments); index++ {
		argument := arguments[index]
		if argument == name {
			if found || index+1 >= len(arguments) {
				return "", false, nil, fmt.Errorf("%s requires exactly one value", name)
			}
			found = true
			index++
			value = arguments[index]
			continue
		}
		if strings.HasPrefix(argument, name+"=") {
			if found {
				return "", false, nil, fmt.Errorf("%s requires exactly one value", name)
			}
			found = true
			value = strings.TrimPrefix(argument, name+"=")
			continue
		}
		remaining = append(remaining, argument)
	}
	return value, found, remaining, nil
}
