package usage

import protocolv1 "mini-sub2api/src/protocol/v1/go"

// SSEProgress contains only fixed counters, never provider event names or payloads.
type SSEProgress struct {
	Events, Outputs, Heartbeats uint64
	Classes                     [protocolv1.SSEClassCount]uint64
}

// Progress never flushes an unfinished event or changes accounting state.
func (o *Observer) Progress() SSEProgress { return o.progress }

func increment(value *uint64) {
	if *value != ^uint64(0) {
		*value++
	}
}

func (o *Observer) observeProgress(data []byte) {
	class := protocolv1.ClassifySSEData(data)
	increment(&o.progress.Events)
	increment(&o.progress.Classes[class])
	if class.IsOutput() {
		increment(&o.progress.Outputs)
	}
}
