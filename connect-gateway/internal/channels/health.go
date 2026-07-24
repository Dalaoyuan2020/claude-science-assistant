package channels

import (
	"sync"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
)

type healthState struct {
	mu    sync.RWMutex
	value model.ChannelHealth
}

func newHealth(id string, configured bool, detail string) *healthState {
	return &healthState{value: model.ChannelHealth{
		ID:         id,
		Configured: configured,
		Detail:     detail,
		UpdatedAt:  model.UnixMillis(),
	}}
}

func (h *healthState) snapshot() model.ChannelHealth {
	h.mu.RLock()
	defer h.mu.RUnlock()
	return h.value
}

func (h *healthState) running(running bool, detail, lastError string) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.value.Running = running
	if detail != "" {
		h.value.Detail = detail
	}
	h.value.LastError = lastError
	h.value.UpdatedAt = model.UnixMillis()
}

func (h *healthState) event() {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.value.LastEventAt = model.UnixMillis()
	h.value.UpdatedAt = h.value.LastEventAt
}
