package buildmeta

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

type Info struct {
	PackageName    string   `json:"packageName"`
	Version        string   `json:"version"`
	FullCommit     string   `json:"fullCommit"`
	ShortCommit    string   `json:"shortCommit"`
	Branch         string   `json:"branch"`
	Dirty          bool     `json:"dirty"`
	BuildTimestamp string   `json:"buildTimestamp"`
	Artifacts      []string `json:"artifacts"`
}

type CheckResult struct {
	Status           string `json:"status"`
	MetadataPath     string `json:"metadataPath"`
	InstalledVersion string `json:"installedVersion,omitempty"`
	InstalledCommit  string `json:"installedCommit,omitempty"`
	SourceCommit     string `json:"sourceCommit,omitempty"`
	OriginMainCommit string `json:"originMainCommit,omitempty"`
	SourceDirty      *bool  `json:"sourceDirty,omitempty"`
	Message          string `json:"message"`
}

func Generate(repository, version string, now time.Time) (Info, error) {
	git, err := ReadGit(repository)
	if err != nil {
		return Info{}, err
	}
	return Info{
		PackageName:    "mini-sub2api",
		Version:        version,
		FullCommit:     git.Commit,
		ShortCommit:    git.ShortCommit,
		Branch:         git.Branch,
		Dirty:          git.Dirty,
		BuildTimestamp: now.UTC().Format(time.RFC3339),
		Artifacts:      []string{"mini-sub2api", "mini-sub2api-core-codex", "build-info.json"},
	}, nil
}

func Write(path string, info Info) error {
	data, err := json.MarshalIndent(info, "", "  ")
	if err != nil {
		return err
	}
	data = append(data, '\n')
	temporary := path + ".tmp"
	if err := os.WriteFile(temporary, data, 0o644); err != nil {
		return err
	}
	return os.Rename(temporary, path)
}

func LoadAdjacent(executable string) (Info, string, error) {
	path := filepath.Join(filepath.Dir(executable), "build-info.json")
	data, err := os.ReadFile(path)
	if err != nil {
		return Info{}, path, err
	}
	var info Info
	if err := json.Unmarshal(data, &info); err != nil {
		return Info{}, path, err
	}
	return info, path, nil
}

func Check(executable, sourceDir, embeddedVersion, embeddedCommit string) (CheckResult, bool) {
	info, metadataPath, err := LoadAdjacent(executable)
	result := CheckResult{MetadataPath: metadataPath}
	if err != nil {
		result.Status = "metadata_missing"
		if !errors.Is(err, os.ErrNotExist) {
			result.Status = "metadata_invalid"
		}
		result.Message = err.Error()
		return result, false
	}
	result.InstalledVersion = info.Version
	result.InstalledCommit = info.FullCommit
	if info.PackageName != "mini-sub2api" || info.Version != embeddedVersion ||
		info.FullCommit != embeddedCommit {
		result.Status = "artifact_mismatch"
		result.Message = "embedded build identity does not match build-info.json"
		return result, false
	}
	git, err := ReadGit(sourceDir)
	if err != nil {
		result.Status = "source_unavailable"
		result.Message = err.Error()
		return result, false
	}
	result.SourceCommit = git.Commit
	result.OriginMainCommit = git.OriginMainCommit
	result.SourceDirty = &git.Dirty
	if git.Commit != info.FullCommit {
		result.Status = "source_mismatch"
		result.Message = "installed commit differs from the current source checkout"
		return result, false
	}
	result.Status = "ok"
	result.Message = "installed metadata matches the current source checkout"
	return result, true
}

type GitState struct {
	Commit           string
	ShortCommit      string
	Branch           string
	Dirty            bool
	OriginMainCommit string
}

func ReadGit(repository string) (GitState, error) {
	inside, err := gitOutput(repository, "rev-parse", "--is-inside-work-tree")
	if err != nil || inside != "true" {
		return GitState{}, fmt.Errorf("source directory is not a Git checkout")
	}
	state := GitState{}
	state.Commit, err = gitOutput(repository, "rev-parse", "HEAD")
	if err != nil {
		state.Commit = "unborn"
	}
	if state.Commit == "unborn" {
		state.ShortCommit = "unborn"
	} else if len(state.Commit) >= 12 {
		state.ShortCommit = state.Commit[:12]
	} else {
		state.ShortCommit = state.Commit
	}
	state.Branch, err = gitOutput(repository, "branch", "--show-current")
	if err != nil || state.Branch == "" {
		state.Branch = "detached"
	}
	status, err := gitOutput(repository, "status", "--porcelain")
	if err != nil {
		return GitState{}, fmt.Errorf("inspect Git dirty state: %w", err)
	}
	state.Dirty = status != ""
	state.OriginMainCommit, _ = gitOutput(repository, "rev-parse", "--verify", "origin/main")
	return state, nil
}

func gitOutput(repository string, arguments ...string) (string, error) {
	command := exec.Command("git", append([]string{"-C", repository}, arguments...)...)
	var stdout, stderr bytes.Buffer
	command.Stdout = &stdout
	command.Stderr = &stderr
	if err := command.Run(); err != nil {
		return "", fmt.Errorf("git %s failed: %w", strings.Join(arguments, " "), err)
	}
	return strings.TrimSpace(stdout.String()), nil
}
