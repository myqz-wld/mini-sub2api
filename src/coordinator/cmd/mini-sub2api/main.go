package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"syscall"

	"mini-sub2api/src/coordinator/internal/buildmeta"
	"mini-sub2api/src/coordinator/internal/cli"
)

var version = "0.1.0"
var buildCommit = "unknown"

func main() {
	if len(os.Args) == 2 && os.Args[1] == "--version" {
		fmt.Printf("mini-sub2api %s (%s)\n", version, shortCommit(buildCommit))
		return
	}
	if len(os.Args) == 2 && os.Args[1] == "--check-installed" {
		os.Exit(checkInstalled())
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if err := cli.Run(ctx, os.Args[1:], cli.Environment{
		Stdin: os.Stdin, Stdout: os.Stdout, Stderr: os.Stderr,
	}); err != nil {
		fmt.Fprintf(os.Stderr, "mini-sub2api: %v\n", err)
		os.Exit(1)
	}
}

func checkInstalled() int {
	executable, err := os.Executable()
	if err != nil {
		_ = json.NewEncoder(os.Stdout).Encode(map[string]string{
			"status": "metadata_invalid", "message": err.Error(),
		})
		return 1
	}
	workingDirectory, err := os.Getwd()
	if err != nil {
		workingDirectory = "."
	}
	result, ok := buildmeta.Check(executable, workingDirectory, version, buildCommit)
	_ = json.NewEncoder(os.Stdout).Encode(result)
	if !ok {
		return 1
	}
	return 0
}

func shortCommit(commit string) string {
	if len(commit) > 12 {
		return commit[:12]
	}
	return commit
}
