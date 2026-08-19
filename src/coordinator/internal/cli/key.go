package cli

import (
	"context"
	"fmt"

	"mini-sub2api/src/coordinator/internal/storage"
)

func runKey(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	if len(arguments) == 0 {
		return fmt.Errorf("key command is required")
	}
	switch arguments[0] {
	case "create":
		return keyCreate(ctx, options, arguments[1:], environment)
	case "list":
		return keyList(ctx, options, arguments[1:], environment)
	case "revoke":
		return keyRevoke(ctx, options, arguments[1:], environment)
	case "help", "--help", "-h":
		_, err := fmt.Fprintln(environment.Stdout, "key commands: create, list, revoke")
		return err
	default:
		return fmt.Errorf("unknown key command %q", arguments[0])
	}
}

func keyCreate(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	flags := newFlagSet("key create", environment.Stderr)
	credentialID := flags.String("credential", "", "credential id")
	name := flags.String("name", "", "API key display name")
	if err := flags.Parse(arguments); err != nil {
		return err
	}
	if flags.NArg() != 0 || *credentialID == "" || *name == "" {
		return fmt.Errorf("usage: key create --credential ID --name NAME")
	}
	store, err := openStore(ctx, options)
	if err != nil {
		return err
	}
	defer store.Close()
	created, err := store.CreateAPIKey(ctx, *credentialID, *name)
	if err != nil {
		return err
	}
	if options.json {
		return writeResult(environment.Stdout, true, created, "")
	}
	_, err = fmt.Fprintf(
		environment.Stdout,
		"Created API key %s.\nSecret (shown once): %s\n",
		created.ID, created.Secret,
	)
	return err
}

func keyList(ctx context.Context, options globalOptions, arguments []string, environment Environment) error {
	if requestedHelp(arguments) {
		_, err := fmt.Fprintln(environment.Stdout, "usage: key list")
		return err
	}
	if len(arguments) != 0 {
		return fmt.Errorf("key list accepts no arguments")
	}
	store, err := openStore(ctx, options)
	if err != nil {
		return err
	}
	defer store.Close()
	keys, err := store.APIKeys(ctx)
	if err != nil {
		return err
	}
	if options.json {
		return writeResult(environment.Stdout, true, keys, "")
	}
	if len(keys) == 0 {
		_, err = fmt.Fprintln(environment.Stdout, "No API keys.")
		return err
	}
	for _, key := range keys {
		if _, err := fmt.Fprintf(
			environment.Stdout, "%s\t%s\t%s...\t%s\t%s\n",
			key.ID, key.Name, key.Prefix, key.CredentialID, key.Status,
		); err != nil {
			return err
		}
	}
	return nil
}

func keyRevoke(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	if requestedHelp(arguments) {
		_, err := fmt.Fprintln(environment.Stdout, "usage: key revoke ID [--yes]")
		return err
	}
	yes, remaining, err := takeBoolFlag(arguments, "--yes")
	if err != nil {
		return err
	}
	if len(remaining) != 1 {
		return fmt.Errorf("API key id is required")
	}
	id := remaining[0]
	confirmed, err := confirm(
		environment.Stdin, environment.Stdout,
		fmt.Sprintf("Revoke downstream API key %s?", id), yes,
	)
	if err != nil || !confirmed {
		if err != nil {
			return err
		}
		return fmt.Errorf("API key revoke cancelled")
	}
	store, err := openStore(ctx, options)
	if err != nil {
		return err
	}
	defer store.Close()
	if err := store.RevokeAPIKey(ctx, id); err != nil {
		return err
	}
	return writeResult(environment.Stdout, options.json, map[string]any{
		"id": id, "status": storage.KeyRevoked,
	}, fmt.Sprintf("Revoked API key %s.", id))
}
