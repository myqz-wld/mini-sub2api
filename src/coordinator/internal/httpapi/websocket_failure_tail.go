package httpapi

import (
	"time"

	"mini-sub2api/src/coordinator/internal/storage"
	"mini-sub2api/src/coordinator/internal/usage"
)

const websocketFailureTail = time.Second

// A retired stream may read one bounded failure tail for usage, but never admit
// another create. Expiring this record must not clear websocketSession.retired.
type websocketFailedOperation struct {
	operation  *websocketOperation
	generation uint64
	deadline   time.Time
}

func (s *websocketSession) detachFailedOperation() *websocketOperation {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.active == nil {
		return nil
	}
	var previous *websocketOperation
	if s.failed != nil {
		previous = s.failed.operation
	}
	s.failed = &websocketFailedOperation{operation: s.active, generation: s.generation, deadline: time.Now().Add(websocketFailureTail)}
	s.failedResponseObserved = true
	close(s.active.terminalReady)
	s.active = nil
	return previous
}

func (s *websocketSession) failedServerEvent(event usage.WebSocketEvent) (*websocketOperation, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if failed := s.failed; failed != nil {
		id := failed.operation.responseID
		matches := id != "" && event.ResponseID == id
		unbound := id == "" && s.generation == failed.generation && event.Type == "response.failed"
		repeatedError := s.active == nil && s.generation == failed.generation && event.Type == "error"
		if matches || unbound || repeatedError {
			if event.Type != "response.failed" {
				return nil, true
			}
			if event.Usage != nil {
				value := *event.Usage
				failed.operation.usage = &value
			}
			s.failed = nil
			if s.generation == failed.generation {
				s.failedResponseObserved = false
			}
			return failed.operation, true
		}
	}
	// API-key frames remain transparent even when an old response is unassociated.
	// Such a frame must not finalize or contribute usage to a newer known response.
	if s.active != nil && s.active.responseID != "" && event.ResponseID != "" && s.active.responseID != event.ResponseID {
		return nil, true
	}
	return nil, false
}

func (s *websocketSession) failureTailDeadline() (time.Time, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.failed == nil {
		return time.Time{}, false
	}
	return s.failed.deadline, true
}

func (s *websocketSession) takeFailedOperation() *websocketOperation {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.failed == nil {
		return nil
	}
	operation := s.failed.operation
	s.failed = nil
	return operation
}

func (s *websocketSession) expireFailureTail() {
	s.mu.Lock()
	var operation *websocketOperation
	if s.failed != nil && !time.Now().Before(s.failed.deadline) {
		operation = s.failed.operation
		s.failed = nil
	}
	s.mu.Unlock()
	if operation != nil {
		s.finishOperation(operation, storage.RequestUpstreamErr)
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
