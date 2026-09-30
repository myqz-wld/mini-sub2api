package httpapi

// This affects usage classification only. Core owns control admission and context proof.
func (s *websocketSession) requestInterrupt(responseID string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.active != nil && s.active.responseID == responseID && !s.active.terminalPending {
		s.active.interruptRequested = true
	}
}

func (s *websocketSession) interruptedActive(responseID string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.active != nil && responseID != "" && s.active.responseID == responseID && s.active.interruptRequested
}
