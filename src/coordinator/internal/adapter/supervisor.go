package adapter

import (
	"bufio"
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"time"

	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

var ErrUnavailable = errors.New("Codex core is unavailable")

type Config struct {
	Binary   string
	StateDir string
}

type Supervisor struct {
	config  Config
	ctx     context.Context
	cancel  context.CancelFunc
	mu      sync.RWMutex
	current *runningCore
	done    chan struct{}
}

type runningCore struct {
	command   *exec.Cmd
	readiness protocolv1.Readiness
	baseURL   string
	token     string
	exited    chan error
}

func Start(ctx context.Context, config Config) (*Supervisor, error) {
	if config.Binary == "" {
		return nil, fmt.Errorf("core binary path is required")
	}
	if config.StateDir == "" {
		return nil, fmt.Errorf("core state directory is required")
	}
	absoluteBinary, err := exec.LookPath(config.Binary)
	if err != nil {
		return nil, fmt.Errorf("find Codex core binary: %w", err)
	}
	config.Binary = absoluteBinary
	config.StateDir, err = filepath.Abs(config.StateDir)
	if err != nil {
		return nil, fmt.Errorf("resolve core state directory: %w", err)
	}
	supervisorContext, cancel := context.WithCancel(ctx)
	supervisor := &Supervisor{
		config: config,
		ctx:    supervisorContext,
		cancel: cancel,
		done:   make(chan struct{}),
	}
	core, err := supervisor.startCore()
	if err != nil {
		cancel()
		return nil, err
	}
	supervisor.current = core
	go supervisor.monitor(core)
	return supervisor, nil
}

func (s *Supervisor) Close() error {
	s.cancel()
	s.mu.RLock()
	core := s.current
	s.mu.RUnlock()
	if core != nil && core.command.Process != nil {
		_ = core.command.Process.Kill()
	}
	<-s.done
	return nil
}

func (s *Supervisor) Readiness() (protocolv1.Readiness, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.current == nil {
		return protocolv1.Readiness{}, false
	}
	return s.current.readiness, true
}

func (s *Supervisor) snapshot() (*runningCore, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.current == nil {
		return nil, ErrUnavailable
	}
	return s.current, nil
}

func (s *Supervisor) monitor(core *runningCore) {
	defer close(s.done)
	backoff := 250 * time.Millisecond
	for {
		select {
		case <-s.ctx.Done():
			return
		case <-core.exited:
		}
		s.mu.Lock()
		if s.current == core {
			s.current = nil
		}
		s.mu.Unlock()

		for {
			select {
			case <-s.ctx.Done():
				return
			case <-time.After(backoff):
			}
			restarted, err := s.startCore()
			if err != nil {
				backoff *= 2
				if backoff > 5*time.Second {
					backoff = 5 * time.Second
				}
				continue
			}
			core = restarted
			backoff = 250 * time.Millisecond
			s.mu.Lock()
			s.current = core
			s.mu.Unlock()
			break
		}
	}
}

func (s *Supervisor) startCore() (*runningCore, error) {
	tokenBytes := make([]byte, 32)
	if _, err := rand.Read(tokenBytes); err != nil {
		return nil, fmt.Errorf("generate internal core token: %w", err)
	}
	token := base64.RawURLEncoding.EncodeToString(tokenBytes)
	command := exec.CommandContext(
		s.ctx, s.config.Binary, "serve", "--listen", "127.0.0.1:0", "--state-dir", s.config.StateDir,
	)
	stdin, err := command.StdinPipe()
	if err != nil {
		return nil, fmt.Errorf("open core stdin: %w", err)
	}
	stdout, err := command.StdoutPipe()
	if err != nil {
		return nil, fmt.Errorf("open core stdout: %w", err)
	}
	command.Stderr = os.Stderr
	if err := command.Start(); err != nil {
		return nil, fmt.Errorf("start Codex core: %w", err)
	}
	if _, err := io.WriteString(stdin, token+"\n"); err != nil {
		stdin.Close()
		_ = command.Process.Kill()
		_ = command.Wait()
		return nil, fmt.Errorf("send internal core token: %w", err)
	}
	if err := stdin.Close(); err != nil {
		_ = command.Process.Kill()
		_ = command.Wait()
		return nil, fmt.Errorf("close core stdin: %w", err)
	}

	reader := bufio.NewReaderSize(stdout, 4097)
	readinessChannel := make(chan readinessResult, 1)
	go func() {
		readinessChannel <- readReadiness(reader, command.Process.Pid)
	}()
	var readiness protocolv1.Readiness
	select {
	case <-s.ctx.Done():
		_ = command.Process.Kill()
		_ = command.Wait()
		return nil, s.ctx.Err()
	case <-time.After(10 * time.Second):
		_ = command.Process.Kill()
		_ = command.Wait()
		return nil, fmt.Errorf("Codex core readiness timed out")
	case result := <-readinessChannel:
		if result.err != nil {
			_ = command.Process.Kill()
			_ = command.Wait()
			return nil, result.err
		}
		readiness = result.readiness
	}
	go func() {
		_, _ = io.Copy(io.Discard, reader)
	}()
	exited := make(chan error, 1)
	go func() {
		exited <- command.Wait()
		close(exited)
	}()
	return &runningCore{
		command:   command,
		readiness: readiness,
		baseURL:   fmt.Sprintf("http://127.0.0.1:%d", readiness.Port),
		token:     token,
		exited:    exited,
	}, nil
}

type readinessResult struct {
	readiness protocolv1.Readiness
	err       error
}

func readReadiness(reader *bufio.Reader, expectedPID int) readinessResult {
	line, err := reader.ReadSlice('\n')
	if errors.Is(err, bufio.ErrBufferFull) {
		return readinessResult{err: fmt.Errorf("core readiness exceeds 4096 bytes")}
	}
	if err != nil {
		return readinessResult{err: fmt.Errorf("read core readiness: %w", err)}
	}
	if len(line) > 4096 {
		return readinessResult{err: fmt.Errorf("core readiness exceeds 4096 bytes")}
	}
	var readiness protocolv1.Readiness
	if err := json.Unmarshal(line, &readiness); err != nil {
		return readinessResult{err: fmt.Errorf("decode core readiness: %w", err)}
	}
	if readiness.ProtocolVersion != protocolv1.Version || readiness.Port == 0 || readiness.PID != expectedPID {
		return readinessResult{err: fmt.Errorf("core readiness identity is invalid")}
	}
	return readinessResult{readiness: readiness}
}
