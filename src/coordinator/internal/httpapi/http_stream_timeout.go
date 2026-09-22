package httpapi

import (
	"context"
	"sync"
	"time"
)

type httpStreamTimeouts struct {
	idle           time.Duration
	outputIdle     time.Duration
	terminalTail   time.Duration
	write          time.Duration
	progressReport time.Duration
}

func defaultHTTPStreamTimeouts() httpStreamTimeouts {
	return httpStreamTimeouts{idle: 300 * time.Second, outputIdle: 300 * time.Second, terminalTail: 2 * time.Second, write: 120 * time.Second, progressReport: time.Minute}
}

type streamStopReason string

const (
	streamStopNone            streamStopReason = ""
	streamStopIdle            streamStopReason = "event_idle_timeout"
	streamStopFirstOutput     streamStopReason = "first_output_timeout"
	streamStopOutputIdle      streamStopReason = "output_idle_timeout"
	streamStopTail            streamStopReason = "terminal_tail_closed"
	streamStopCanceled        streamStopReason = "client_canceled"
	streamStopPartialTail     streamStopReason = "incomplete_terminal_tail"
	streamStopWriteTimeout    streamStopReason = "downstream_write_timeout"
	streamStopMissingTerminal streamStopReason = "missing_terminal"
)

// The watcher never accesses response bytes or the ResponseWriter. Its cancellation/Close
// unblocks a pending upstream Read, and stop joins it before the handler can return.
type httpStreamGuard struct {
	mu            sync.Mutex
	lastEvent     time.Time
	lastOutput    time.Time
	watchOutput   bool
	hasOutput     bool
	firstTerminal time.Time
	reason        streamStopReason
	timeouts      httpStreamTimeouts
	wake          chan struct{}
	done          chan struct{}
	exited        chan struct{}
	stopOnce      sync.Once
}

func newHTTPStreamGuard(ctx context.Context, timeouts httpStreamTimeouts, abort func()) *httpStreamGuard {
	now := time.Now()
	guard := &httpStreamGuard{
		lastEvent: now, lastOutput: now, timeouts: timeouts,
		wake: make(chan struct{}, 1), done: make(chan struct{}), exited: make(chan struct{}),
	}
	go guard.run(ctx, abort)
	return guard
}

func (g *httpStreamGuard) observe(progress, output, terminal bool) {
	if !progress && !output && !terminal {
		return
	}
	g.mu.Lock()
	if g.reason == streamStopNone {
		now := time.Now()
		if progress {
			g.lastEvent = now
		}
		if output {
			g.lastOutput = now
			g.hasOutput = true
		}
		if terminal && g.firstTerminal.IsZero() {
			g.firstTerminal = now
		}
	}
	g.mu.Unlock()
	select {
	case g.wake <- struct{}{}:
	default:
	}
}

// Error-envelope inspection and non-SSE bodies use only the transport idle timer.
func (g *httpStreamGuard) enableOutputDeadline() {
	g.mu.Lock()
	g.watchOutput = true
	g.mu.Unlock()
	select {
	case g.wake <- struct{}{}:
	default:
	}
}

func (g *httpStreamGuard) stop() streamStopReason {
	g.stopOnce.Do(func() { close(g.done) })
	<-g.exited
	g.mu.Lock()
	defer g.mu.Unlock()
	return g.reason
}

func (g *httpStreamGuard) run(ctx context.Context, abort func()) {
	defer close(g.exited)
	timer := time.NewTimer(time.Hour)
	defer timer.Stop()
	for {
		g.mu.Lock()
		deadline, reason := g.lastEvent.Add(g.timeouts.idle), streamStopIdle
		if outputDeadline := g.lastOutput.Add(g.timeouts.outputIdle); g.watchOutput && outputDeadline.Before(deadline) {
			deadline, reason = outputDeadline, streamStopFirstOutput
			if g.hasOutput {
				reason = streamStopOutputIdle
			}
		}
		if !g.firstTerminal.IsZero() {
			deadline, reason = g.firstTerminal.Add(g.timeouts.terminalTail), streamStopTail
		}
		remaining := time.Until(deadline)
		if remaining <= 0 {
			g.reason = reason
			g.mu.Unlock()
			abort()
			return
		}
		g.mu.Unlock()
		if !timer.Stop() {
			select {
			case <-timer.C:
			default:
			}
		}
		timer.Reset(remaining)
		select {
		case <-g.done:
			return
		case <-g.wake:
		case <-timer.C:
		case <-ctx.Done():
			g.mu.Lock()
			g.reason = streamStopCanceled
			g.mu.Unlock()
			abort()
			return
		}
	}
}
