package cli

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
)

type Environment struct {
	Stdin  io.Reader
	Stdout io.Writer
	Stderr io.Writer
}

type globalOptions struct {
	stateDir   string
	coreBinary string
	json       bool
}

func Run(ctx context.Context, arguments []string, environment Environment) error {
	if environment.Stdin == nil {
		environment.Stdin = os.Stdin
	}
	if environment.Stdout == nil {
		environment.Stdout = os.Stdout
	}
	if environment.Stderr == nil {
		environment.Stderr = os.Stderr
	}
	options, remaining, err := extractGlobalOptions(arguments)
	if err != nil {
		return err
	}
	if len(remaining) == 0 {
		printRootHelp(environment.Stdout)
		return nil
	}
	var commandErr error
	switch remaining[0] {
	case "serve":
		commandErr = runServe(ctx, options, remaining[1:], environment)
	case "credential":
		commandErr = runCredential(ctx, options, remaining[1:], environment)
	case "key":
		commandErr = runKey(ctx, options, remaining[1:], environment)
	case "usage":
		commandErr = runUsage(ctx, options, remaining[1:], environment)
	case "help", "--help", "-h":
		printRootHelp(environment.Stdout)
		return nil
	default:
		return fmt.Errorf("unknown command %q", remaining[0])
	}
	if errors.Is(commandErr, flag.ErrHelp) {
		return nil
	}
	return commandErr
}

func extractGlobalOptions(arguments []string) (globalOptions, []string, error) {
	options := globalOptions{
		stateDir:   os.Getenv("MINI_SUB2API_STATE_DIR"),
		coreBinary: os.Getenv("MINI_SUB2API_CORE_CODEX_BINARY"),
	}
	var remaining []string
	for index := 0; index < len(arguments); index++ {
		argument := arguments[index]
		switch {
		case argument == "--json":
			options.json = true
		case argument == "--state-dir" || argument == "--core-binary":
			if index+1 >= len(arguments) {
				return options, nil, fmt.Errorf("%s requires a value", argument)
			}
			index++
			if argument == "--state-dir" {
				options.stateDir = arguments[index]
			} else {
				options.coreBinary = arguments[index]
			}
		case strings.HasPrefix(argument, "--state-dir="):
			options.stateDir = strings.TrimPrefix(argument, "--state-dir=")
		case strings.HasPrefix(argument, "--core-binary="):
			options.coreBinary = strings.TrimPrefix(argument, "--core-binary=")
		default:
			remaining = append(remaining, argument)
		}
	}
	if options.stateDir == "" {
		options.stateDir = ".mini-sub2api"
	}
	absolute, err := filepath.Abs(options.stateDir)
	if err != nil {
		return options, nil, fmt.Errorf("resolve state directory: %w", err)
	}
	options.stateDir = absolute
	return options, remaining, nil
}

func printRootHelp(output io.Writer) {
	fmt.Fprintln(output, `mini-sub2api

Usage:
  mini-sub2api [--state-dir DIR] [--json] serve [options]
  mini-sub2api [--state-dir DIR] [--json] credential <command>
  mini-sub2api [--state-dir DIR] [--json] key <command>
  mini-sub2api [--state-dir DIR] [--json] usage <command>

Commands:
  serve       Start the public Responses service and supervised Codex core.
  credential  Manage Codex OAuth and upstream API-key credentials.
  key         Create, list, and revoke downstream API keys.
  usage       Inspect or prune per-key request usage.`)
}
