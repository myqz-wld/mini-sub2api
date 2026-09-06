package adapter

import (
	"context"
	"os/exec"
	"testing"
	"time"
)

// Model the owned child exit barrier without starting or signaling an external process.
func TestSupervisorCloseWaitsForOwnedExit(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	exited := make(chan error, 1)
	core := &runningCore{command: &exec.Cmd{}, exited: exited}
	supervisor := &Supervisor{ctx: ctx, cancel: cancel, current: core, done: make(chan struct{})}
	go supervisor.monitor(core)
	closed := make(chan struct{})
	go func() { _ = supervisor.Close(); close(closed) }()
	<-ctx.Done()
	select {
	case <-closed:
		t.Error("Close returned before the owned child exit barrier")
	case <-time.After(100 * time.Millisecond):
	}
	exited <- nil
	close(exited)
	select {
	case <-closed:
	case <-time.After(time.Second):
		t.Fatal("Close did not finish after child exit")
	}
}
