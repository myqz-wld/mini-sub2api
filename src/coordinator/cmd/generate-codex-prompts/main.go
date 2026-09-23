package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
)

const sourceCommit = "fe74a774532af67b5a4a3dec03ce9469e17f89af"
const promptDirectory = "src/core/codex/prompts/codex-0.156.0"

var modelFiles = map[string]string{
	"gpt-5.6-sol": "gpt-5.6.md", "gpt-5.6-terra": "gpt-5.6.md", "gpt-5.6-luna": "gpt-5.6.md",
	"gpt-5.5": "gpt-5.5.md", "gpt-5.4": "gpt-5.4.md",
	"codex-auto-review": "gpt-daybreak-blue.md",
	"gpt-6-astra":       "gpt-6-astra.md", "gpt-daybreak-blue-latest": "gpt-daybreak-blue.md",
	"gpt-daybreak-red-latest": "gpt-daybreak-red.md",
}

type model struct {
	Slug     string         `json:"slug"`
	Messages *modelMessages `json:"model_messages"`
}

type modelMessages struct {
	Template *string `json:"instructions_template"`
}

func main() {
	source := flag.String("codex-source", "", "local Codex Git repository containing the pinned 0.156.0 commit")
	output := flag.String("output", promptDirectory, "snapshot output directory")
	check := flag.Bool("check", false, "compare snapshots without writing files")
	flag.Parse()
	if *source == "" {
		fmt.Fprintln(os.Stderr, "--codex-source is required; this tool never fetches source")
		os.Exit(2)
	}
	if err := run(*source, *output, *check); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	fmt.Println("Validated Codex 0.156.0: 9 catalog defaults and 1 fallback in 7 prompt files.")
}

func run(source, output string, check bool) error {
	read := func(path string) ([]byte, error) {
		command := exec.Command("git", "-C", source, "show", sourceCommit+":"+path)
		command.Env = append(os.Environ(), "GIT_NO_LAZY_FETCH=1")
		data, err := command.Output()
		if err != nil {
			return nil, fmt.Errorf("cannot read pinned Codex source %s (commit %s)", path, sourceCommit)
		}
		return data, nil
	}
	catalog, err := read("codex-rs/models-manager/models.json")
	if err != nil {
		return err
	}
	fallback, err := read("codex-rs/models-manager/prompt.md")
	if err != nil {
		return err
	}
	snapshots, err := renderSnapshots(catalog, string(fallback))
	if err != nil {
		return err
	}
	for _, name := range []string{"LICENSE", "NOTICE"} {
		data, err := read(name)
		if err != nil {
			return err
		}
		snapshots[name] = data
	}
	// Finish rendering and validation before any output mutation.
	names := make([]string, 0, len(snapshots))
	for name := range snapshots {
		names = append(names, name)
	}
	sort.Strings(names)
	if !check {
		if err := os.MkdirAll(output, 0o755); err != nil {
			return err
		}
	}
	for _, name := range names {
		path := filepath.Join(output, name)
		if check {
			existing, err := os.ReadFile(path)
			if err != nil || !bytes.Equal(existing, snapshots[name]) {
				return fmt.Errorf("snapshot mismatch: %s", name)
			}
		} else if err := os.WriteFile(path, snapshots[name], 0o644); err != nil {
			return err
		}
	}
	return nil
}

func renderSnapshots(catalog []byte, fallback string) (map[string][]byte, error) {
	var decoded struct {
		Models []model `json:"models"`
	}
	if err := json.Unmarshal(catalog, &decoded); err != nil {
		return nil, fmt.Errorf("invalid model catalog: %w", err)
	}
	snapshots := map[string][]byte{
		"fallback.md": []byte(fallback),
	}
	seen := make(map[string]bool)
	for _, entry := range decoded.Models {
		name, supported := modelFiles[entry.Slug]
		if !supported || seen[entry.Slug] {
			return nil, fmt.Errorf("unexpected or duplicate catalog model: %s", entry.Slug)
		}
		seen[entry.Slug] = true
		if entry.Messages == nil || entry.Messages.Template == nil {
			return nil, fmt.Errorf("missing instruction template: %s", entry.Slug)
		}
		text := *entry.Messages.Template
		// Codex 0.156.0 consumes the template literally; personalities are already embedded.
		if previous, exists := snapshots[name]; exists && string(previous) != text {
			return nil, fmt.Errorf("shared prompt differs for model: %s", entry.Slug)
		}
		snapshots[name] = []byte(text)
	}
	if len(seen) != len(modelFiles) {
		return nil, fmt.Errorf("catalog must contain all %d pinned models", len(modelFiles))
	}
	for name, data := range snapshots {
		if len(bytes.TrimSpace(data)) == 0 {
			return nil, fmt.Errorf("empty rendered prompt: %s", name)
		}

	}
	return snapshots, nil
}
