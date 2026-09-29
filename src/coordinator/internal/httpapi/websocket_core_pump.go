package httpapi

import (
	"context"

	"github.com/coder/websocket"
	"mini-sub2api/src/coordinator/internal/storage"
	"mini-sub2api/src/coordinator/internal/usage"
)

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
		event, ok := usage.ParseWebSocketEvent(payload)
		if !ok {
			s.observeCoreResponse()
			return s.upstreamPumpResult(nil)
		}
		failed, belongsToOldResponse := s.failedServerEvent(event)
		terminalStatus, terminal := websocketTerminalStatus(event.Type)
		if !belongsToOldResponse {
			s.observeCoreResponse()
			s.observeServerEvent(event, terminal)
			if event.ExpectsFailedFooter {
				if previous := s.detachFailedOperation(); previous != nil {
					s.finishOperation(previous, storage.RequestUpstreamErr)
				}
				s.notifyDeadline(deadlineFailureTail)
				s.notifyDeadline(deadlineTurnFinished)
				terminal = false
			}
		}
		writeContext, cancel := context.WithTimeout(s.ctx, s.timeouts.write)
		err = s.publicSocket.Write(writeContext, websocket.MessageText, payload)
		cancel()
		if failed != nil {
			s.finishOperation(failed, storage.RequestUpstreamErr)
		}
		if err != nil {
			return s.pumpResult(storage.RequestDisconnected)
		}
		if !belongsToOldResponse && terminal && s.completeActive(terminalStatus) {
			s.notifyDeadline(deadlineTurnFinished)
		}
	}
}
