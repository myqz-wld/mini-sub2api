package main

import (
	"flag"
	"fmt"
	"os"
	"time"

	"mini-sub2api/src/coordinator/internal/buildmeta"
)

func main() {
	output := flag.String("output", "", "build-info.json output path")
	version := flag.String("version", "", "semantic version")
	repository := flag.String("repository", ".", "source repository")
	flag.Parse()
	if *output == "" || *version == "" {
		fmt.Fprintln(os.Stderr, "--output and --version are required")
		os.Exit(2)
	}
	info, err := buildmeta.Generate(*repository, *version, time.Now())
	if err == nil {
		err = buildmeta.Write(*output, info)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
