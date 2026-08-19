package cli

import (
	"context"
	"fmt"
	"path/filepath"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
)

func runCredential(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	if len(arguments) == 0 {
		return fmt.Errorf("credential command is required")
	}
	switch arguments[0] {
	case "login":
		return credentialLogin(ctx, options, arguments[1:], environment)
	case "import-codex":
		return credentialImportCodex(ctx, options, arguments[1:], environment)
	case "add-api-key":
		return credentialAddAPIKey(ctx, options, arguments[1:], environment)
	case "list":
		return credentialList(ctx, options, arguments[1:], environment)
	case "enable", "disable":
		return credentialSetEnabled(ctx, options, arguments[0] == "enable", arguments[1:], environment)
	case "revoke":
		return credentialRevoke(ctx, options, arguments[1:], environment)
	case "remove":
		return credentialRemove(ctx, options, arguments[1:], environment)
	case "help", "--help", "-h":
		_, err := fmt.Fprintln(environment.Stdout, "credential commands: login, import-codex, add-api-key, list, enable, disable, revoke, remove")
		return err
	default:
		return fmt.Errorf("unknown credential command %q", arguments[0])
	}
}

func credentialImportCodex(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	if requestedHelp(arguments) {
		_, err := fmt.Fprintln(environment.Stdout, "usage: credential import-codex --name NAME --auth-file FILE")
		return err
	}
	flags := newFlagSet("credential import-codex", environment.Stderr)
	name := flags.String("name", "", "credential display name")
	authFile := flags.String("auth-file", "", "existing Codex auth.json path")
	issuer := flags.String("issuer", "https://auth.openai.com", "OAuth issuer override")
	clientID := flags.String("client-id", "app_EMoamEEZ73f0CkXaXp7hrann", "OAuth client id override")
	upstreamURL := flags.String("upstream-url", "https://chatgpt.com/backend-api/codex/responses", "Codex Responses URL")
	if err := flags.Parse(arguments); err != nil {
		return err
	}
	if flags.NArg() != 0 || *name == "" || *authFile == "" {
		return fmt.Errorf("usage: credential import-codex --name NAME --auth-file FILE")
	}
	absoluteAuthFile, err := filepath.Abs(*authFile)
	if err != nil {
		return fmt.Errorf("resolve Codex auth file: %w", err)
	}
	metadata, err := runCoreCredential(ctx, options, []string{
		"import-codex-auth", "--state-dir", coreStateDirectory(options),
		"--auth-file", absoluteAuthFile, "--issuer", *issuer,
		"--client-id", *clientID, "--upstream-url", *upstreamURL,
	}, nil, environment.Stderr)
	if err != nil {
		return err
	}
	return persistCoreCredential(ctx, options, *name, metadata, environment)
}

func credentialLogin(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	if requestedHelp(arguments) {
		_, err := fmt.Fprintln(environment.Stdout, "usage: credential login codex --name NAME [--flow device|browser]")
		return err
	}
	if len(arguments) == 0 || arguments[0] != "codex" {
		return fmt.Errorf("usage: credential login codex --name NAME")
	}
	flags := newFlagSet("credential login", environment.Stderr)
	name := flags.String("name", "", "credential display name")
	flow := flags.String("flow", "device", "OAuth flow: device or browser")
	issuer := flags.String("issuer", "https://auth.openai.com", "OAuth issuer override")
	clientID := flags.String("client-id", "app_EMoamEEZ73f0CkXaXp7hrann", "OAuth client id override")
	upstreamURL := flags.String("upstream-url", "https://chatgpt.com/backend-api/codex/responses", "Codex Responses URL")
	if err := flags.Parse(arguments[1:]); err != nil {
		return err
	}
	if flags.NArg() != 0 {
		return fmt.Errorf("usage: credential login codex --name NAME")
	}
	if *name == "" || (*flow != "device" && *flow != "browser") {
		return fmt.Errorf("--name is required and --flow must be device or browser")
	}
	metadata, err := runCoreCredential(ctx, options, []string{
		"login", "--state-dir", coreStateDirectory(options), "--flow", *flow,
		"--issuer", *issuer, "--client-id", *clientID, "--upstream-url", *upstreamURL,
	}, environment.Stdin, environment.Stderr)
	if err != nil {
		return err
	}
	return persistCoreCredential(ctx, options, *name, metadata, environment)
}

func credentialAddAPIKey(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	if requestedHelp(arguments) {
		_, err := fmt.Fprintln(environment.Stdout, "usage: credential add-api-key codex --name NAME --secret-stdin")
		return err
	}
	if len(arguments) == 0 || arguments[0] != "codex" {
		return fmt.Errorf("usage: credential add-api-key codex --name NAME --secret-stdin")
	}
	flags := newFlagSet("credential add-api-key", environment.Stderr)
	name := flags.String("name", "", "credential display name")
	secretStdin := flags.Bool("secret-stdin", false, "read the upstream API key from stdin")
	upstreamURL := flags.String("upstream-url", "https://api.openai.com/v1/responses", "OpenAI Responses URL")
	if err := flags.Parse(arguments[1:]); err != nil {
		return err
	}
	if flags.NArg() != 0 || *name == "" || !*secretStdin {
		return fmt.Errorf("usage: credential add-api-key codex --name NAME --secret-stdin")
	}
	metadata, err := runCoreCredential(ctx, options, []string{
		"add-api-key", "--state-dir", coreStateDirectory(options),
		"--upstream-url", *upstreamURL, "--secret-stdin",
	}, environment.Stdin, environment.Stderr)
	if err != nil {
		return err
	}
	return persistCoreCredential(ctx, options, *name, metadata, environment)
}

func persistCoreCredential(
	ctx context.Context,
	options globalOptions,
	name string,
	metadata coreCredentialMetadata,
	environment Environment,
) error {
	store, err := openStore(ctx, options)
	if err != nil {
		_ = removeCoreCredential(ctx, options, metadata.AccountRef, environment.Stderr)
		return err
	}
	defer store.Close()
	credential, err := store.CreateCredential(
		ctx, name, "codex", metadata.AuthKind, metadata.AccountRef, metadata.UpstreamAccountID,
	)
	if err != nil {
		_ = removeCoreCredential(ctx, options, metadata.AccountRef, environment.Stderr)
		return err
	}
	return writeResult(
		environment.Stdout, options.json, credential,
		fmt.Sprintf("Created credential %s (%s).", credential.ID, credential.AuthKind),
	)
}

func credentialList(ctx context.Context, options globalOptions, arguments []string, environment Environment) error {
	if requestedHelp(arguments) {
		_, err := fmt.Fprintln(environment.Stdout, "usage: credential list")
		return err
	}
	if len(arguments) != 0 {
		return fmt.Errorf("credential list accepts no arguments")
	}
	store, err := openStore(ctx, options)
	if err != nil {
		return err
	}
	defer store.Close()
	credentials, err := store.Credentials(ctx)
	if err != nil {
		return err
	}
	if options.json {
		return writeResult(environment.Stdout, true, credentials, "")
	}
	if len(credentials) == 0 {
		_, err = fmt.Fprintln(environment.Stdout, "No credentials.")
		return err
	}
	for _, credential := range credentials {
		if _, err := fmt.Fprintf(
			environment.Stdout, "%s\t%s\t%s\t%s\n",
			credential.ID, credential.Name, credential.AuthKind, credential.Status,
		); err != nil {
			return err
		}
	}
	return nil
}

func credentialSetEnabled(
	ctx context.Context,
	options globalOptions,
	enabled bool,
	arguments []string,
	environment Environment,
) error {
	if requestedHelp(arguments) {
		command := "disable"
		if enabled {
			command = "enable"
		}
		_, err := fmt.Fprintf(environment.Stdout, "usage: credential %s ID\n", command)
		return err
	}
	if len(arguments) != 1 {
		return fmt.Errorf("credential id is required")
	}
	store, err := openStore(ctx, options)
	if err != nil {
		return err
	}
	defer store.Close()
	if err := store.SetCredentialEnabled(ctx, arguments[0], enabled); err != nil {
		return err
	}
	status := storage.CredentialDisabled
	if enabled {
		status = storage.CredentialEnabled
	}
	return writeResult(environment.Stdout, options.json, map[string]any{
		"id": arguments[0], "status": status,
	}, fmt.Sprintf("Credential %s is now %s.", arguments[0], status))
}

func credentialRevoke(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	if requestedHelp(arguments) {
		_, err := fmt.Fprintln(environment.Stdout, "usage: credential revoke ID [--yes]")
		return err
	}
	yes, remaining, err := takeBoolFlag(arguments, "--yes")
	if err != nil {
		return err
	}
	if len(remaining) != 1 {
		return fmt.Errorf("credential id is required")
	}
	id := remaining[0]
	store, err := openStore(ctx, options)
	if err != nil {
		return err
	}
	defer store.Close()
	credential, err := store.Credential(ctx, id)
	if err != nil {
		return err
	}
	if credential.AuthKind != "codex_oauth" {
		return fmt.Errorf("regular upstream API keys use credential remove; provider-side deletion is not supported")
	}
	if err := requireNoActiveKeys(ctx, store, id); err != nil {
		return err
	}
	confirmed, err := confirm(
		environment.Stdin, environment.Stdout,
		fmt.Sprintf("Revoke upstream OAuth and remove credential %s?", id), yes,
	)
	if err != nil || !confirmed {
		if err != nil {
			return err
		}
		return fmt.Errorf("credential revoke cancelled")
	}
	if err := store.SetCredentialEnabled(ctx, id, false); err != nil {
		return err
	}
	if err := requireNoActiveKeys(ctx, store, id); err != nil {
		return err
	}
	if err := waitForNoInFlight(ctx, store, id); err != nil {
		return err
	}
	if _, err := runCoreCredential(ctx, options, []string{
		"revoke", "--state-dir", coreStateDirectory(options), credential.AccountRef,
	}, nil, environment.Stderr); err != nil {
		return fmt.Errorf("upstream revoke failed; the disabled service-side credential was retained: %w", err)
	}
	if err := store.DeleteCredentialMetadata(ctx, id); err != nil {
		return err
	}
	return writeResult(environment.Stdout, options.json, map[string]any{
		"id": id, "revoked": true,
	}, fmt.Sprintf("Revoked and removed credential %s.", id))
}

func credentialRemove(
	ctx context.Context,
	options globalOptions,
	arguments []string,
	environment Environment,
) error {
	if requestedHelp(arguments) {
		_, err := fmt.Fprintln(environment.Stdout, "usage: credential remove ID [--force-service-only --yes]")
		return err
	}
	yes, remaining, err := takeBoolFlag(arguments, "--yes")
	if err != nil {
		return err
	}
	forceOAuth, remaining, err := takeBoolFlag(remaining, "--force-service-only")
	if err != nil {
		return err
	}
	if len(remaining) != 1 {
		return fmt.Errorf("credential id is required")
	}
	id := remaining[0]
	store, err := openStore(ctx, options)
	if err != nil {
		return err
	}
	defer store.Close()
	credential, err := store.Credential(ctx, id)
	if err != nil {
		return err
	}
	if credential.AuthKind == "codex_oauth" && !forceOAuth {
		return fmt.Errorf("OAuth removal requires credential revoke, or --force-service-only with --yes")
	}
	if forceOAuth && !yes {
		return fmt.Errorf("--force-service-only requires --yes")
	}
	if err := requireNoActiveKeys(ctx, store, id); err != nil {
		return err
	}
	confirmed, err := confirm(
		environment.Stdin, environment.Stdout,
		fmt.Sprintf("Remove service-side credential %s?", id), yes,
	)
	if err != nil || !confirmed {
		if err != nil {
			return err
		}
		return fmt.Errorf("credential removal cancelled")
	}
	if err := store.SetCredentialEnabled(ctx, id, false); err != nil {
		return err
	}
	if err := requireNoActiveKeys(ctx, store, id); err != nil {
		return err
	}
	if err := waitForNoInFlight(ctx, store, id); err != nil {
		return err
	}
	if err := removeCoreCredential(ctx, options, credential.AccountRef, environment.Stderr); err != nil {
		return err
	}
	if err := store.DeleteCredentialMetadata(ctx, id); err != nil {
		return err
	}
	return writeResult(environment.Stdout, options.json, map[string]any{
		"id": id, "removed": true,
	}, fmt.Sprintf("Removed credential %s from mini-sub2api.", id))
}

func requireNoActiveKeys(ctx context.Context, store *storage.Store, credentialID string) error {
	count, err := store.ActiveKeyCount(ctx, credentialID)
	if err != nil {
		return err
	}
	if count != 0 {
		return fmt.Errorf("credential %s still has %d active downstream API key(s)", credentialID, count)
	}
	return nil
}

func waitForNoInFlight(ctx context.Context, store *storage.Store, credentialID string) error {
	deadline := time.NewTimer(30 * time.Second)
	defer deadline.Stop()
	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()
	for {
		count, err := store.InFlightCount(ctx, credentialID)
		if err != nil {
			return err
		}
		if count == 0 {
			return nil
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-deadline.C:
			return fmt.Errorf("timed out waiting for %d in-flight request(s)", count)
		case <-ticker.C:
		}
	}
}
