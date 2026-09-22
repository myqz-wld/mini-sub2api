package httpapi

import (
	"fmt"
	"time"

	"mini-sub2api/src/coordinator/internal/diagnostics"
	"mini-sub2api/src/coordinator/internal/usage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

type httpStreamDiagnostics struct {
	started      time.Time
	bytes        uint64
	progress     usage.SSEProgress
	firstByte    int64
	lastByte     int64
	firstEvent   int64
	lastEvent    int64
	firstOutput  int64
	lastOutput   int64
	bytesWritten uint64
	writeTime    time.Duration
	failurePhase string
	failure      diagnostics.ErrorInfo
}

func newHTTPStreamDiagnostics() httpStreamDiagnostics {
	return httpStreamDiagnostics{started: time.Now(), firstByte: -1, lastByte: -1,
		firstEvent: -1, lastEvent: -1, firstOutput: -1, lastOutput: -1,
		failurePhase: "none", failure: diagnostics.Error(nil)}
}

func (d *httpStreamDiagnostics) observe(bytes int, progress usage.SSEProgress) {
	now := time.Since(d.started).Milliseconds()
	if bytes > 0 {
		d.bytes += uint64(bytes)
		d.lastByte = now
		if d.firstByte < 0 {
			d.firstByte = now
		}
	}
	if progress.Events != d.progress.Events {
		d.lastEvent = now
		if d.firstEvent < 0 {
			d.firstEvent = now
		}
	}
	if progress.Outputs != d.progress.Outputs {
		d.lastOutput = now
		if d.firstOutput < 0 {
			d.firstOutput = now
		}
	}
	d.progress = progress
}

// All labels and values are fixed categories or numeric observations, never raw JSON.
func (d httpStreamDiagnostics) String() string {
	p := d.progress
	return fmt.Sprintf("elapsed_ms=%d bytes=%d events=%d outputs=%d heartbeats=%d status=%d text=%d reasoning=%d tool=%d media=%d item=%d terminal=%d empty=%d other=%d invalid=%d first_byte_ms=%d last_byte_ms=%d first_event_ms=%d last_event_ms=%d first_output_ms=%d last_output_ms=%d bytes_written=%d write_ms=%d failure_phase=%s %s",
		time.Since(d.started).Milliseconds(), d.bytes, p.Events, p.Outputs, p.Heartbeats,
		p.Classes[protocolv1.SSEStatus], p.Classes[protocolv1.SSEText], p.Classes[protocolv1.SSEReasoning],
		p.Classes[protocolv1.SSETool], p.Classes[protocolv1.SSEMedia], p.Classes[protocolv1.SSEItem],
		p.Classes[protocolv1.SSETerminal], p.Classes[protocolv1.SSEEmpty], p.Classes[protocolv1.SSEOther], p.Classes[protocolv1.SSEInvalid],
		d.firstByte, d.lastByte, d.firstEvent, d.lastEvent, d.firstOutput, d.lastOutput, d.bytesWritten, d.writeTime.Milliseconds(), d.failurePhase, d.failure)
}
