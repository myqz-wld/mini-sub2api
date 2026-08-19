package cli

import (
	"bufio"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
)

func newFlagSet(name string, errorOutput io.Writer) *flag.FlagSet {
	flags := flag.NewFlagSet(name, flag.ContinueOnError)
	flags.SetOutput(errorOutput)
	return flags
}

func openStore(ctx context.Context, options globalOptions) (*storage.Store, error) {
	return storage.Open(ctx, options.stateDir, time.Now)
}

func writeResult(output io.Writer, jsonOutput bool, value any, human string) error {
	if jsonOutput {
		encoder := json.NewEncoder(output)
		encoder.SetEscapeHTML(false)
		return encoder.Encode(value)
	}
	_, err := fmt.Fprintln(output, human)
	return err
}

func confirm(input io.Reader, output io.Writer, prompt string, yes bool) (bool, error) {
	if yes {
		return true, nil
	}
	if _, err := fmt.Fprintf(output, "%s [y/N] ", prompt); err != nil {
		return false, err
	}
	line, err := bufio.NewReader(input).ReadString('\n')
	if err != nil && err != io.EOF {
		return false, err
	}
	answer := strings.ToLower(strings.TrimSpace(line))
	return answer == "y" || answer == "yes", nil
}

func resolveCoreBinary(configured string) (string, error) {
	if configured != "" {
		return configured, nil
	}
	executable, err := os.Executable()
	if err == nil {
		sibling := filepath.Join(filepath.Dir(executable), "mini-sub2api-core-codex")
		if info, statErr := os.Stat(sibling); statErr == nil && !info.IsDir() {
			return sibling, nil
		}
	}
	return "mini-sub2api-core-codex", nil
}

func logger(output io.Writer) *log.Logger {
	return log.New(output, "mini-sub2api: ", log.LstdFlags)
}

func takeBoolFlag(arguments []string, name string) (bool, []string, error) {
	value := false
	remaining := make([]string, 0, len(arguments))
	for _, argument := range arguments {
		if argument == name {
			value = true
			continue
		}
		if strings.HasPrefix(argument, name+"=") {
			parsed, err := strconv.ParseBool(strings.TrimPrefix(argument, name+"="))
			if err != nil {
				return false, nil, fmt.Errorf("invalid value for %s", name)
			}
			value = parsed
			continue
		}
		remaining = append(remaining, argument)
	}
	return value, remaining, nil
}

func requestedHelp(arguments []string) bool {
	return len(arguments) == 1 &&
		(arguments[0] == "--help" || arguments[0] == "-h" || arguments[0] == "help")
}
