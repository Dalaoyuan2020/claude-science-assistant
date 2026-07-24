package channels

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"unicode/utf8"

	lark "github.com/larksuite/oapi-sdk-go/v3"
	larkcore "github.com/larksuite/oapi-sdk-go/v3/core"
	"github.com/larksuite/oapi-sdk-go/v3/event/dispatcher"
	larkim "github.com/larksuite/oapi-sdk-go/v3/service/im/v1"
	larkws "github.com/larksuite/oapi-sdk-go/v3/ws"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
)

type Feishu struct {
	appID     string
	appSecret string
	client    *lark.Client
	health    *healthState
}

func NewFeishu(appID, appSecret string) *Feishu {
	return &Feishu{
		appID:     appID,
		appSecret: appSecret,
		client:    lark.NewClient(appID, appSecret, lark.WithLogLevel(larkcore.LogLevelWarn)),
		health:    newHealth("feishu", appID != "" && appSecret != "", "官方长连接"),
	}
}

func (f *Feishu) ID() string { return "feishu" }

func (f *Feishu) Health(ctx context.Context) model.ChannelHealth {
	value := f.health.snapshot()
	return value
}

func (f *Feishu) Run(ctx context.Context, handler Handler) error {
	if f.appID == "" || f.appSecret == "" {
		return errors.New("Feishu is not configured")
	}
	eventHandler := dispatcher.NewEventDispatcher("", "").OnP2MessageReceiveV1(func(eventContext context.Context, event *larkim.P2MessageReceiveV1) error {
		inbound, ok := f.toInbound(event)
		if !ok {
			return nil
		}
		f.health.event()
		return handler(eventContext, inbound)
	})
	client := larkws.NewClient(f.appID, f.appSecret,
		larkws.WithEventHandler(eventHandler),
		larkws.WithLogLevel(larkcore.LogLevelWarn),
	)
	f.health.running(true, "官方长连接正在运行", "")
	err := client.Start(ctx)
	if ctx.Err() != nil {
		f.health.running(false, "官方长连接已停止", "")
		return nil
	}
	f.health.running(false, "官方长连接已停止", safeChannelError(err))
	return err
}

func (f *Feishu) toInbound(event *larkim.P2MessageReceiveV1) (model.InboundMessage, bool) {
	if event == nil || event.Event == nil || event.Event.Message == nil || event.Event.Sender == nil || event.Event.Sender.SenderId == nil {
		return model.InboundMessage{}, false
	}
	message := event.Event.Message
	if deref(message.MessageType) != "text" {
		return model.InboundMessage{}, false
	}
	var body struct {
		Text string `json:"text"`
	}
	if err := json.Unmarshal([]byte(deref(message.Content)), &body); err != nil || strings.TrimSpace(body.Text) == "" {
		return model.InboundMessage{}, false
	}
	sender := event.Event.Sender.SenderId
	senderID := firstNonEmpty(deref(sender.OpenId), deref(sender.UserId), deref(sender.UnionId))
	conversationID := deref(message.ChatId)
	messageID := deref(message.MessageId)
	if senderID == "" || conversationID == "" || messageID == "" {
		return model.InboundMessage{}, false
	}
	threadID := firstNonEmpty(deref(message.ThreadId), deref(message.RootId), conversationID)
	chatType := "group"
	if deref(message.ChatType) == "p2p" {
		chatType = "private"
	}
	createdAt, _ := strconv.ParseInt(deref(message.CreateTime), 10, 64)
	if createdAt == 0 {
		createdAt = model.UnixMillis()
	}
	return model.InboundMessage{
		Channel:         "feishu",
		AccountID:       f.appID,
		PlatformEventID: messageID,
		SenderID:        senderID,
		ConversationID:  conversationID,
		ThreadID:        threadID,
		ReplyTo:         messageID,
		ChatType:        chatType,
		Text:            body.Text,
		CreatedAt:       createdAt,
	}, true
}

func (f *Feishu) Send(ctx context.Context, outbound model.OutboundMessage) error {
	for _, chunk := range splitText(outbound.Text, 3500) {
		content, _ := json.Marshal(map[string]string{"text": chunk})
		if outbound.ReplyTo != "" {
			req := larkim.NewReplyMessageReqBuilder().
				MessageId(outbound.ReplyTo).
				Body(larkim.NewReplyMessageReqBodyBuilder().
					Content(string(content)).
					MsgType("text").
					ReplyInThread(outbound.ThreadID != "" && outbound.ThreadID != outbound.ConversationID).
					Uuid(outbound.MessageID).
					Build()).
				Build()
			response, err := f.client.Im.V1.Message.Reply(ctx, req)
			if err != nil {
				return errors.New("Feishu reply API is unavailable")
			}
			if !response.Success() {
				return fmt.Errorf("Feishu rejected the reply: code %d", response.Code)
			}
			continue
		}
		req := larkim.NewCreateMessageReqBuilder().
			ReceiveIdType("open_id").
			Body(larkim.NewCreateMessageReqBodyBuilder().
				ReceiveId(outbound.SenderID).
				MsgType("text").
				Content(string(content)).
				Uuid(outbound.MessageID).
				Build()).
			Build()
		response, err := f.client.Im.V1.Message.Create(ctx, req)
		if err != nil {
			return errors.New("Feishu message API is unavailable")
		}
		if !response.Success() {
			return fmt.Errorf("Feishu rejected the message: code %d", response.Code)
		}
	}
	return nil
}

func (f *Feishu) SendProgress(ctx context.Context, outbound model.OutboundMessage, platformMessageID string) (string, error) {
	if utf8.RuneCountInString(outbound.Text) > 3500 {
		return "", errors.New("Feishu progress message is too long")
	}
	content, _ := json.Marshal(map[string]string{"text": outbound.Text})
	if platformMessageID != "" {
		req := larkim.NewPatchMessageReqBuilder().
			MessageId(platformMessageID).
			Body(larkim.NewPatchMessageReqBodyBuilder().Content(string(content)).Build()).
			Build()
		response, err := f.client.Im.V1.Message.Patch(ctx, req)
		if err != nil {
			return "", errors.New("Feishu message update API is unavailable")
		}
		if !response.Success() {
			return "", fmt.Errorf("Feishu rejected the message update: code %d", response.Code)
		}
		return platformMessageID, nil
	}
	if outbound.ReplyTo != "" {
		req := larkim.NewReplyMessageReqBuilder().
			MessageId(outbound.ReplyTo).
			Body(larkim.NewReplyMessageReqBodyBuilder().
				Content(string(content)).
				MsgType("text").
				ReplyInThread(outbound.ThreadID != "" && outbound.ThreadID != outbound.ConversationID).
				Uuid(outbound.MessageID).
				Build()).
			Build()
		response, err := f.client.Im.V1.Message.Reply(ctx, req)
		if err != nil {
			return "", errors.New("Feishu reply API is unavailable")
		}
		if !response.Success() {
			return "", fmt.Errorf("Feishu rejected the reply: code %d", response.Code)
		}
		if response.Data == nil || response.Data.MessageId == nil || *response.Data.MessageId == "" {
			return "", errors.New("Feishu did not return a message ID")
		}
		return *response.Data.MessageId, nil
	}
	return "", errors.New("Feishu progress reply requires a source message")
}

func deref(value *string) string {
	if value == nil {
		return ""
	}
	return *value
}

func firstNonEmpty(values ...string) string {
	for _, value := range values {
		if value != "" {
			return value
		}
	}
	return ""
}
