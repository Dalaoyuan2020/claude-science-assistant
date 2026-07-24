package channels

import (
	"context"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
)

type Handler func(context.Context, model.InboundMessage) error

type Channel interface {
	ID() string
	Run(context.Context, Handler) error
	Send(context.Context, model.OutboundMessage) error
	Health(context.Context) model.ChannelHealth
}

type CursorStore interface {
	GetMeta(context.Context, string) (string, error)
	SetMeta(context.Context, string, string) error
}
