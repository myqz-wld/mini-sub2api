package httpapi

import (
	"context"
	"errors"
	"sync"
	"sync/atomic"
	"time"

	"github.com/coder/websocket"

	"mini-sub2api/src/coordinator/internal/storage"
	"mini-sub2api/src/coordinator/internal/usage"
	protocolv1 "mini-sub2api/src/protocol/v1/go"
)

type websocketTimeouts struct {
	firstFrame time.Duration
	interTurn  time.Duration
	write      time.Duration
}

func defaultWebSocketTimeouts() websocketTimeouts {
	return websocketTimeouts{
		firstFrame: 30 * time.Second,
		interTurn:  300 * time.Second,
		write:      120 * time.Second,
	}
}

type websocketDeadlineEvent int

const (
	deadlineFirstFrame websocketDeadlineEvent = iota
	deadlineTurnStarted
	deadlineTurnFinished
)

type websocketPumpResult struct {
	terminalStatus string
	failure        protocolv1.FailureMetadata
	hasFailure     bool
	closeCode      websocket.StatusCode
	hasCloseCode   bool
}

const (
	exitCauseUnset int32 = iota
	exitCauseClient
	exitCauseUpstream
)

type websocketOperation struct {
	requestID         string
	kind              string
	started           time.Time
	ttfb              *time.Duration
	usage             *storage.TokenUsage
	terminalPending   bool
	terminalReady     chan struct{}
	providerRequestID *string
}

type websocketSession struct {
	handler           *Handler
	route             storage.Route
	publicSocket      *websocket.Conn
	coreSocket        *websocket.Conn
	timeouts          websocketTimeouts
	ctx               context.Context
	cancel            context.CancelFunc
	deadlines         chan websocketDeadlineEvent
	stopping          atomic.Bool
	exitCause         atomic.Int32
	stopOnce          sync.Once
	mu                sync.Mutex
	active            *websocketOperation
	providerRequestID *string
}

var errOverlappingResponse = errors.New("a response is already active")

func newWebSocketSession(
	handler *Handler,
	route storage.Route,
	publicSocket, coreSocket *websocket.Conn,
	providerRequestID *string,
) *websocketSession {
	ctx, cancel := context.WithCancel(context.Background())
	return &websocketSession{
		handler: handler, route: route, publicSocket: publicSocket, coreSocket: coreSocket,
		timeouts: handler.wsTimeouts, ctx: ctx, cancel: cancel,
		deadlines:         make(chan websocketDeadlineEvent, 8),
		providerRequestID: cloneString(providerRequestID),
	}
}

func (s *websocketSession) run() {
	results := make(chan websocketPumpResult, 2)
	go func() { results <- s.clientPump() }()
	go func() { results <- s.corePump() }()

	timer := time.NewTimer(s.timeouts.firstFrame)
	defer timer.Stop()
	timerChannel := timer.C
	for {
		select {
		case event := <-s.deadlines:
			switch event {
			case deadlineFirstFrame, deadlineTurnStarted:
				stopWebSocketTimer(timer)
				timerChannel = nil
			case deadlineTurnFinished:
				stopWebSocketTimer(timer)
				timer.Reset(s.timeouts.interTurn)
				timerChannel = timer.C
			}
		case result := <-results:
			status := s.causalStatus(result.terminalStatus)
			if s.stopping.Load() {
				status = storage.RequestUpstreamErr
			} else if status == storage.RequestUpstreamErr {
				s.closePublicForCoreFailure(result)
			}
			s.cancel()
			_ = s.publicSocket.CloseNow()
			_ = s.coreSocket.CloseNow()
			s.finishActive(status)
			return
		case <-timerChannel:
			_ = s.publicSocket.Close(websocket.StatusPolicyViolation, "")
			s.cancel()
			_ = s.coreSocket.CloseNow()
			s.finishActive(storage.RequestDisconnected)
			return
		}
	}
}

func stopWebSocketTimer(timer *time.Timer) {
	if !timer.Stop() {
		select {
		case <-timer.C:
		default:
		}
	}
}

func (s *websocketSession) stop() {
	s.stopOnce.Do(func() {
		s.stopping.Store(true)
		s.cancel()
		_ = s.publicSocket.CloseNow()
		_ = s.coreSocket.CloseNow()
	})
}

func (s *websocketSession) clientPump() websocketPumpResult {
	firstFrame := true
	for {
		messageType, payload, err := s.publicSocket.Read(s.ctx)
		if err != nil {
			return s.pumpResult(storage.RequestDisconnected)
		}
		if firstFrame {
			firstFrame = false
			s.notifyDeadline(deadlineFirstFrame)
		}
		if messageType != websocket.MessageText {
			s.recordExitCause(storage.RequestDisconnected)
			_ = s.publicSocket.Close(websocket.StatusUnsupportedData, "")
			return s.pumpResult(storage.RequestDisconnected)
		}
		event, ok := parseClientApplicationEvent(payload)
		if !ok {
			s.recordExitCause(storage.RequestDisconnected)
			_ = s.publicSocket.Close(websocket.StatusProtocolError, "")
			return s.pumpResult(storage.RequestDisconnected)
		}
		if event.eventType == "response.create" {
			if err := s.beginOperation(event.operationKind); err != nil {
				status := websocket.StatusInternalError
				if errors.Is(err, errOverlappingResponse) || errors.Is(err, storage.ErrUnauthorized) {
					status = websocket.StatusPolicyViolation
				}
				s.recordExitCause(storage.RequestDisconnected)
				_ = s.publicSocket.Close(status, "")
				return s.pumpResult(storage.RequestDisconnected)
			}
			s.notifyDeadline(deadlineTurnStarted)
		} else if !s.hasActiveOperation() {
			s.recordExitCause(storage.RequestDisconnected)
			_ = s.publicSocket.Close(websocket.StatusPolicyViolation, "")
			return s.pumpResult(storage.RequestDisconnected)
		}
		writeContext, cancel := context.WithTimeout(s.ctx, s.timeouts.write)
		err = s.coreSocket.Write(writeContext, websocket.MessageText, payload)
		cancel()
		if err != nil {
			return s.upstreamPumpResult(err)
		}
	}
}

func (s *websocketSession) corePump() websocketPumpResult {
	for {
		messageType, payload, err := s.coreSocket.Read(s.ctx)
		if err != nil {
			return s.upstreamPumpResult(err)
		}
		if messageType != websocket.MessageText {
			return s.upstreamPumpResult(nil)
		}
		if providerRequestID, control, valid := parseProviderRequestIDControl(payload); control {
			if !valid {
				return s.upstreamPumpResult(nil)
			}
			s.observeProviderRequestID(providerRequestID)
			continue
		}
		s.observeCoreResponse()
		event, ok := usage.ParseWebSocketEvent(payload)
		if !ok {
			return s.upstreamPumpResult(nil)
		}
		terminalStatus, terminal := websocketTerminalStatus(event.Type)
		s.observeServerEvent(event, terminal)
		writeContext, cancel := context.WithTimeout(s.ctx, s.timeouts.write)
		err = s.publicSocket.Write(writeContext, websocket.MessageText, payload)
		cancel()
		if err != nil {
			return s.pumpResult(storage.RequestDisconnected)
		}
		if terminal && s.completeActive(terminalStatus) {
			s.notifyDeadline(deadlineTurnFinished)
		}
	}
}

func (s *websocketSession) beginOperation(kind string) error {
	for {
		s.mu.Lock()
		if s.active != nil && s.active.terminalPending {
			ready := s.active.terminalReady
			s.mu.Unlock()
			select {
			case <-ready:
				continue
			case <-s.ctx.Done():
				return s.ctx.Err()
			}
		}
		if s.active != nil {
			s.mu.Unlock()
			return errOverlappingResponse
		}
		if s.ctx.Err() != nil || s.stopping.Load() {
			s.mu.Unlock()
			return context.Canceled
		}
		requestID, err := newRequestID()
		if err != nil {
			s.mu.Unlock()
			return err
		}
		operation := &websocketOperation{
			requestID: requestID, kind: kind, started: s.handler.clock().UTC(),
			terminalReady:     make(chan struct{}),
			providerRequestID: cloneString(s.providerRequestID),
		}
		s.active = operation
		err = s.handler.store.StartWebSocketOperation(s.ctx, s.route, requestID, kind)
		if err != nil {
			s.active = nil
		}
		s.mu.Unlock()
		return err
	}
}

func (s *websocketSession) observeProviderRequestID(value string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.providerRequestID = cloneString(&value)
	if s.active != nil {
		s.active.providerRequestID = cloneString(&value)
	}
}

func (s *websocketSession) hasActiveOperation() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.active != nil
}

func (s *websocketSession) observeServerEvent(event usage.WebSocketEvent, terminal bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.active == nil {
		return
	}
	if event.Usage != nil {
		value := *event.Usage
		s.active.usage = &value
	}
	if terminal {
		s.active.terminalPending = true
	}
}

func (s *websocketSession) observeCoreResponse() {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.active == nil || s.active.ttfb != nil {
		return
	}
	value := s.handler.clock().UTC().Sub(s.active.started)
	if value < 0 {
		value = 0
	}
	s.active.ttfb = &value
}

func (s *websocketSession) completeActive(status string) bool {
	operation := s.takeActive()
	if operation == nil {
		return false
	}
	s.finishOperation(operation, status)
	return true
}

func (s *websocketSession) finishActive(status string) {
	if operation := s.takeActive(); operation != nil {
		s.finishOperation(operation, status)
	}
}

func (s *websocketSession) takeActive() *websocketOperation {
	s.mu.Lock()
	defer s.mu.Unlock()
	operation := s.active
	if operation == nil {
		return nil
	}
	s.active = nil
	close(operation.terminalReady)
	return operation
}

func (s *websocketSession) finishOperation(operation *websocketOperation, status string) {
	completed := s.handler.clock().UTC()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	err := s.handler.store.FinalizeRequest(ctx, operation.requestID, storage.RequestResult{
		CompletedAt:       completed,
		Status:            status,
		TTFB:              operation.ttfb,
		Duration:          completed.Sub(operation.started),
		Usage:             operation.usage,
		ProviderRequestID: operation.providerRequestID,
	})
	if err != nil {
		s.handler.logger.Printf("request %s history finalization failed: %v", operation.requestID, err)
	}
}

func cloneString(value *string) *string {
	if value == nil {
		return nil
	}
	cloned := *value
	return &cloned
}

func (s *websocketSession) notifyDeadline(event websocketDeadlineEvent) {
	select {
	case s.deadlines <- event:
	case <-s.ctx.Done():
	}
}

func (s *websocketSession) pumpResult(status string) websocketPumpResult {
	s.recordExitCause(status)
	return websocketPumpResult{terminalStatus: status}
}

func (s *websocketSession) upstreamPumpResult(err error) websocketPumpResult {
	s.recordExitCause(storage.RequestUpstreamErr)
	if metadata, ok := parseCoreFailureClose(err); ok {
		return websocketPumpResult{
			terminalStatus: storage.RequestUpstreamErr, failure: metadata, hasFailure: true,
		}
	}
	if code, ok := passthroughCoreClose(err); ok {
		return websocketPumpResult{
			terminalStatus: storage.RequestUpstreamErr, closeCode: code, hasCloseCode: true,
		}
	}
	return websocketPumpResult{
		terminalStatus: storage.RequestUpstreamErr,
		failure:        s.currentDeliveryFailure(),
		hasFailure:     true,
	}
}

func (s *websocketSession) currentDeliveryFailure() protocolv1.FailureMetadata {
	s.mu.Lock()
	defer s.mu.Unlock()
	metadata := protocolv1.FailureMetadata{
		RetryAdvice: protocolv1.RetrySafe, Phase: protocolv1.PhaseWebSocketRelay,
		DeliveryState: protocolv1.DeliveryNotDelivered,
	}
	if s.active == nil {
		return metadata
	}
	if s.active.ttfb != nil {
		metadata.RetryAdvice = protocolv1.RetryNever
		metadata.DeliveryState = protocolv1.DeliveryDelivered
		return metadata
	}
	metadata.RetryAdvice = protocolv1.RetryAmbiguous
	metadata.DeliveryState = protocolv1.DeliveryPossiblyDelivered
	return metadata
}

func (s *websocketSession) closePublicForCoreFailure(result websocketPumpResult) {
	if result.hasFailure {
		if reason, ok := failureCloseReason(result.failure); ok {
			_ = s.publicSocket.Close(websocket.StatusCode(protocolv1.FailureCloseCode), reason)
			return
		}
	}
	if result.hasCloseCode {
		_ = s.publicSocket.Close(result.closeCode, "")
		return
	}
	metadata := s.currentDeliveryFailure()
	if reason, ok := failureCloseReason(metadata); ok {
		_ = s.publicSocket.Close(websocket.StatusCode(protocolv1.FailureCloseCode), reason)
		return
	}
	_ = s.publicSocket.Close(websocket.StatusServiceRestart, "")
}

func (s *websocketSession) recordExitCause(status string) {
	cause := int32(exitCauseUpstream)
	if status == storage.RequestDisconnected {
		cause = exitCauseClient
	}
	s.exitCause.CompareAndSwap(exitCauseUnset, cause)
}

func (s *websocketSession) causalStatus(fallback string) string {
	switch s.exitCause.Load() {
	case exitCauseClient:
		return storage.RequestDisconnected
	case exitCauseUpstream:
		return storage.RequestUpstreamErr
	default:
		return fallback
	}
}

func websocketTerminalStatus(eventType string) (string, bool) {
	switch eventType {
	case "response.completed":
		return storage.RequestCompleted, true
	case "response.failed", "response.incomplete", "error":
		return storage.RequestUpstreamErr, true
	default:
		return "", false
	}
}
