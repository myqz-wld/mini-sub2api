package httpapi

import (
	"sync"
	"time"
)

const maxWebSocketsPerKey = 8

type websocketManager struct {
	mu        sync.Mutex
	limit     int
	perKey    map[string]int
	sessions  map[*websocketSession]struct{}
	closing   bool
	drained   chan struct{}
	drainOnce sync.Once
}

func newWebSocketManager(limit int) *websocketManager {
	return &websocketManager{
		limit: limit, perKey: make(map[string]int), sessions: make(map[*websocketSession]struct{}),
		drained: make(chan struct{}),
	}
}

func (m *websocketManager) acquire(apiKeyID string) bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.closing || m.perKey[apiKeyID] >= m.limit {
		return false
	}
	m.perKey[apiKeyID]++
	return true
}

func (m *websocketManager) release(apiKeyID string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	remaining := m.perKey[apiKeyID] - 1
	if remaining <= 0 {
		delete(m.perKey, apiKeyID)
		return
	}
	m.perKey[apiKeyID] = remaining
}

func (m *websocketManager) register(session *websocketSession) bool {
	m.mu.Lock()
	if m.closing {
		m.mu.Unlock()
		return false
	}
	m.sessions[session] = struct{}{}
	m.mu.Unlock()
	return true
}

func (m *websocketManager) unregister(session *websocketSession) {
	m.mu.Lock()
	delete(m.sessions, session)
	if m.closing && len(m.sessions) == 0 {
		m.drainOnce.Do(func() { close(m.drained) })
	}
	m.mu.Unlock()
}

func (m *websocketManager) shutdown() {
	m.mu.Lock()
	m.closing = true
	sessions := make([]*websocketSession, 0, len(m.sessions))
	for session := range m.sessions {
		sessions = append(sessions, session)
	}
	if len(sessions) == 0 {
		m.drainOnce.Do(func() { close(m.drained) })
	}
	m.mu.Unlock()
	for _, session := range sessions {
		session.stop()
	}
	select {
	case <-m.drained:
	case <-time.After(10 * time.Second):
	}
}

func (h *Handler) ShutdownWebSockets() {
	h.websockets.shutdown()
}
