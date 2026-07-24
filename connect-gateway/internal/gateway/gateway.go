package gateway

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/store"
)

const maxInboundBytes = 16 * 1024
const maxAttachmentBytes = 20 * 1024 * 1024
const maxInboundAttachments = 4
const maxProgressRunes = 3400
const staleClaimTimeout = 5 * time.Minute

const staleClaimNotice = "Claude Science 真身未在 5 分钟内响应。消息已保留在队列中，页面恢复后会继续处理。"

type Sender interface {
	Send(context.Context, model.OutboundMessage) error
}

type ProgressSender interface {
	SendProgress(context.Context, model.OutboundMessage, string) (string, error)
}

type AttachmentSender interface {
	SendAttachment(context.Context, model.OutboundMessage, model.OutboundAttachment) (string, error)
}

type progressState struct {
	PlatformMessageID string `json:"platformMessageId"`
	Sequence          int    `json:"sequence"`
	Text              string `json:"text"`
	UpdatedAt         int64  `json:"updatedAt"`
}

type Gateway struct {
	store            *store.Store
	senders          map[string]Sender
	artifactResolver ArtifactResolver
	mu               sync.RWMutex
	progressMu       sync.Mutex
}

func New(dataStore *store.Store) *Gateway {
	return &Gateway{
		store:            dataStore,
		senders:          make(map[string]Sender),
		artifactResolver: defaultArtifactResolver(),
	}
}

func (g *Gateway) RegisterSender(channel string, sender Sender) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.senders[channel] = sender
}

func (g *Gateway) Store() *store.Store {
	return g.store
}

func (g *Gateway) HandleInbound(ctx context.Context, inbound model.InboundMessage) error {
	if err := validateInbound(inbound); err != nil {
		return err
	}
	if inbound.ChatType != "private" {
		return nil
	}
	text := strings.TrimSpace(inbound.Text)
	parts := strings.Fields(text)
	if len(parts) == 2 && (strings.EqualFold(parts[0], "/pair") || strings.EqualFold(parts[0], "/start")) {
		paired, err := g.store.ConsumePairingCode(ctx, inbound.Channel, inbound.AccountID, inbound.SenderID, inbound.ConversationID, parts[1])
		if err != nil {
			return err
		}
		if !paired {
			return g.sendText(ctx, inbound.Channel, inbound, "配对码无效或已过期。请在 CSA Connect 面板重新生成。")
		}
		return g.sendText(ctx, inbound.Channel, inbound, "CSA Connect 配对成功。发送科研问题后，消息会进入已绑定项目的安全队列。")
	}
	paired, err := g.store.IsPaired(ctx, inbound.Channel, inbound.AccountID, inbound.SenderID)
	if err != nil {
		return err
	}
	if !paired {
		return g.sendText(ctx, inbound.Channel, inbound, "此账号尚未配对。请先在 CSA Connect 面板生成一次性配对码。")
	}
	_ = g.store.TouchIdentity(ctx, inbound.Channel, inbound.AccountID, inbound.SenderID, inbound.ConversationID)
	switch strings.ToLower(text) {
	case "/help":
		return g.sendText(ctx, inbound.Channel, inbound, "可用命令：\n/status - 查看队列状态\n/help - 显示帮助\n\n普通文本会进入 Claude Science 项目队列。外部聊天不能直接执行安装或系统命令。")
	case "/status":
		counts, err := g.store.Counts(ctx)
		if err != nil {
			return err
		}
		return g.sendText(ctx, inbound.Channel, inbound, fmt.Sprintf("CSA Connect 正常。等待绑定 %d，排队 %d，处理中 %d，需要本地审批 %d。", counts.Authorized, counts.Queued, counts.Claimed, counts.NeedsLocalApproval))
	default:
		if strings.HasPrefix(text, "/") {
			return g.sendText(ctx, inbound.Channel, inbound, "未知命令。发送 /help 查看首版支持的安全命令。")
		}
	}
	if err := g.prepareInboundAttachments(&inbound); err != nil {
		return g.sendText(ctx, inbound.Channel, inbound, "图片未进入队列："+err.Error())
	}
	message, duplicate, err := g.store.Receive(ctx, inbound)
	if err != nil {
		return err
	}
	if duplicate {
		return nil
	}
	g.emitEvent(ctx, "connect.message.received", message, map[string]any{"status": message.Status})
	if message.Status == model.StatusAuthorized {
		return g.sendText(ctx, inbound.Channel, inbound, "消息已收到，但这个聊天线程尚未绑定 Claude Science 项目。请在 CSA Connect 面板完成项目绑定。")
	}
	if err := materializeMessage(message); err != nil {
		g.emitEvent(ctx, "connect.delivery.failed", message, map[string]any{"transport": "workspaceFiles", "errorClass": "materialize"})
		// The MCP queue remains available even when workspace materialization fails.
	}
	g.emitEvent(ctx, "connect.message.queued", message, map[string]any{"transport": "mcpQueue", "workspaceFallback": true})
	return g.sendQueueAcknowledgement(ctx, inbound, message)
}

func (g *Gateway) sendQueueAcknowledgement(ctx context.Context, inbound model.InboundMessage, message model.StoredMessage) error {
	const acknowledgement = "消息已进入 Claude Science 项目队列，等待当前会话领取。"
	g.mu.RLock()
	sender := g.senders[inbound.Channel]
	g.mu.RUnlock()
	progressSender, ok := sender.(ProgressSender)
	if !ok {
		return g.sendText(ctx, inbound.Channel, inbound, acknowledgement)
	}
	g.progressMu.Lock()
	defer g.progressMu.Unlock()
	outbound := model.OutboundMessage{
		MessageID:      message.ID + ":progress",
		SenderID:       message.SenderID,
		ConversationID: message.ConversationID,
		ThreadID:       message.ThreadID,
		ReplyTo:        message.ReplyTo,
		Text:           acknowledgement,
	}
	platformMessageID, err := progressSender.SendProgress(ctx, outbound, "")
	if err != nil {
		return err
	}
	state := progressState{
		PlatformMessageID: platformMessageID,
		Sequence:          0,
		Text:              acknowledgement,
		UpdatedAt:         model.UnixMillis(),
	}
	encoded, _ := json.Marshal(state)
	return g.store.SetMeta(ctx, "progress.message."+message.ID, string(encoded))
}

func validateInbound(inbound model.InboundMessage) error {
	if inbound.Channel != "feishu" && inbound.Channel != "telegram" {
		return errors.New("unsupported inbound channel")
	}
	if strings.TrimSpace(inbound.PlatformEventID) == "" || strings.TrimSpace(inbound.SenderID) == "" || strings.TrimSpace(inbound.ConversationID) == "" {
		return errors.New("inbound message is missing identity fields")
	}
	if !utf8.ValidString(inbound.Text) || len(inbound.Text) > maxInboundBytes || strings.ContainsRune(inbound.Text, '\x00') {
		return errors.New("inbound text is invalid or too large")
	}
	if strings.TrimSpace(inbound.Text) == "" && len(inbound.Attachments) == 0 {
		return errors.New("inbound message has no text or attachment")
	}
	if len(inbound.Attachments) > maxInboundAttachments {
		return errors.New("inbound message has too many attachments")
	}
	for _, attachment := range inbound.Attachments {
		if attachment.Kind != "image" || len(attachment.Data) == 0 || len(attachment.Data) > maxAttachmentBytes {
			return errors.New("inbound attachment is unsupported or too large")
		}
	}
	return nil
}

func (g *Gateway) prepareInboundAttachments(inbound *model.InboundMessage) error {
	for index := range inbound.Attachments {
		item := &inbound.Attachments[index]
		mimeType, extension, ok := detectImageType(item.Data)
		if !ok {
			return errors.New("仅支持 JPEG、PNG 或 WebP 图片")
		}
		contentDigest := sha256.Sum256(item.Data)
		identityDigest := sha256.Sum256([]byte(strings.Join([]string{
			inbound.Channel,
			inbound.PlatformEventID,
			strconv.Itoa(index),
		}, "\x1f")))
		attachmentID := hex.EncodeToString(identityDigest[:16])
		fileName := safeAttachmentName(item.FileName, attachmentID+extension)
		attachment := model.AttachmentV2{
			AttachmentID: attachmentID,
			Kind:         "image",
			MIMEType:     mimeType,
			FileName:     fileName,
			SizeBytes:    int64(len(item.Data)),
			SHA256:       hex.EncodeToString(contentDigest[:]),
			State:        "available",
			StorageKey:   attachmentID + extension,
		}
		if err := g.store.WriteAttachmentAtomic(attachment, item.Data); err != nil {
			return errors.New("图片保存失败")
		}
		item.Attachment = attachment
		item.Data = nil
	}
	return nil
}

func detectImageType(data []byte) (string, string, bool) {
	if len(data) >= 3 && data[0] == 0xff && data[1] == 0xd8 && data[2] == 0xff {
		return "image/jpeg", ".jpg", true
	}
	if len(data) >= 8 && string(data[:8]) == "\x89PNG\r\n\x1a\n" {
		return "image/png", ".png", true
	}
	if len(data) >= 12 && string(data[:4]) == "RIFF" && string(data[8:12]) == "WEBP" {
		return "image/webp", ".webp", true
	}
	return "", "", false
}

func safeAttachmentName(value, fallback string) string {
	value = filepath.Base(strings.TrimSpace(value))
	value = strings.Map(func(char rune) rune {
		if char < 32 || char == '/' || char == '\\' || char == ':' {
			return -1
		}
		return char
	}, value)
	if value == "" || value == "." {
		return fallback
	}
	if len(value) > 120 {
		value = value[:120]
	}
	return value
}

func (g *Gateway) sendText(ctx context.Context, channel string, inbound model.InboundMessage, text string) error {
	return g.send(ctx, channel, model.OutboundMessage{
		MessageID:      "system:" + inbound.PlatformEventID,
		SenderID:       inbound.SenderID,
		ConversationID: inbound.ConversationID,
		ThreadID:       inbound.ThreadID,
		ReplyTo:        inbound.ReplyTo,
		Text:           text,
	})
}

func (g *Gateway) send(ctx context.Context, channel string, outbound model.OutboundMessage) error {
	g.mu.RLock()
	sender := g.senders[channel]
	g.mu.RUnlock()
	if sender == nil {
		return fmt.Errorf("%s channel is not running", channel)
	}
	return sender.Send(ctx, outbound)
}

func (g *Gateway) BindRoute(ctx context.Context, routeKey, workspacePath, nativeFrameID string) (model.Route, error) {
	route, pending, err := g.store.UpsertBinding(ctx, routeKey, workspacePath, nativeFrameID)
	if err != nil {
		return model.Route{}, err
	}
	if err := ensureWorkspaceBridge(route.WorkspacePath); err != nil {
		return model.Route{}, err
	}
	for _, message := range pending {
		if message.BindingID != route.BindingID {
			continue
		}
		if err := materializeMessage(message); err != nil {
			g.emitEvent(ctx, "connect.delivery.failed", message, map[string]any{"transport": "workspaceFiles", "errorClass": "materialize"})
		}
		g.emitEvent(ctx, "connect.message.queued", message, map[string]any{"transport": "mcpQueue", "workspaceFallback": true, "queuedByBinding": true})
	}
	return route, nil
}

func (g *Gateway) ListPending(ctx context.Context, workspacePath string, limit int) ([]model.StoredMessage, error) {
	return g.store.ListPending(ctx, filepath.Clean(workspacePath), limit)
}

func (g *Gateway) Claim(ctx context.Context, messageID, workspacePath string) (model.StoredMessage, error) {
	message, err := g.store.Claim(ctx, messageID, filepath.Clean(workspacePath))
	if err != nil {
		return model.StoredMessage{}, err
	}
	_ = writeClaimAck(message)
	g.emitEvent(ctx, "connect.message.claimed", message, map[string]any{"transport": "mcpQueue"})
	return message, nil
}

func (g *Gateway) Requeue(ctx context.Context, messageID string) error {
	message, err := g.store.Message(ctx, strings.TrimSpace(messageID))
	if err != nil {
		return err
	}
	if message.Direction != "inbound" || message.Status != model.StatusClaimed {
		return errors.New("message is not claimed")
	}
	if err := g.store.UpdateMessageStatus(ctx, message.ID, model.StatusQueued, ""); err != nil {
		return err
	}
	if message.WorkspacePath != "" {
		_ = os.Remove(filepath.Join(bridgeRoot(message.WorkspacePath), "ack", message.ID+".json"))
	}
	return nil
}

func (g *Gateway) MarkDelivery(ctx context.Context, messageID, attemptID, status string) error {
	if status != model.StatusSubmitted && status != model.StatusDeliveryUnknown {
		return errors.New("delivery status must be submitted or delivery_unknown")
	}
	message, err := g.store.Message(ctx, strings.TrimSpace(messageID))
	if err != nil {
		return err
	}
	if message.Direction != "inbound" || (message.Status != model.StatusClaimed &&
		message.Status != model.StatusSubmitted && message.Status != model.StatusDeliveryUnknown) {
		return errors.New("message is not available for delivery update")
	}
	if err := g.store.RecordDeliveryAttempt(ctx, model.DeliveryAttempt{
		AttemptID: strings.TrimSpace(attemptID),
		MessageID: message.ID,
		Stage:     "browser_inject",
		Status:    status,
	}); err != nil {
		return err
	}
	if err := g.store.UpdateMessageStatus(ctx, message.ID, status, ""); err != nil {
		return err
	}
	message.Status = status
	_ = writeDeliveryAck(message, status)
	g.emitEvent(ctx, "connect.message."+status, message, map[string]any{"attemptId": attemptID})
	return nil
}

func (g *Gateway) Expire(ctx context.Context, messageID string) error {
	message, err := g.store.Message(ctx, strings.TrimSpace(messageID))
	if err != nil {
		return err
	}
	if message.Direction != "inbound" || !messageCanReceiveReply(message.Status) {
		return errors.New("message is not awaiting a reply")
	}
	if err := g.store.UpdateMessageStatus(ctx, message.ID, model.StatusExpired, ""); err != nil {
		return err
	}
	finishWorkspaceMessage(message, model.StatusExpired)
	g.emitEvent(ctx, "connect.message.expired", message, map[string]any{"reason": "local operator"})
	return nil
}

func (g *Gateway) RecoverStaleClaims(ctx context.Context, timeout time.Duration) (int, error) {
	cutoff := time.Now().Add(-timeout).UnixMilli()
	messages, err := g.store.ListClaimedBefore(ctx, cutoff, 100)
	if err != nil {
		return 0, err
	}
	recovered := 0
	for _, message := range messages {
		updated, err := g.store.RequeueClaimIfStale(ctx, message.ID, cutoff)
		if err != nil {
			return recovered, err
		}
		if !updated {
			continue
		}
		recovered++
		if message.WorkspacePath != "" {
			_ = os.Remove(filepath.Join(bridgeRoot(message.WorkspacePath), "ack", message.ID+".json"))
		}
		outbound := model.OutboundMessage{
			MessageID:      "timeout:" + message.ID,
			SenderID:       message.SenderID,
			ConversationID: message.ConversationID,
			ThreadID:       message.ThreadID,
			ReplyTo:        message.ReplyTo,
			Text:           staleClaimNotice,
		}
		if err := g.send(ctx, message.Channel, outbound); err != nil {
			_ = g.store.UpdateMessageStatus(ctx, message.ID, model.StatusQueued, "timeout notice delivery failed")
			g.emitEvent(ctx, "connect.delivery.failed", message, map[string]any{"transport": message.Channel, "errorClass": "timeout_notice"})
			continue
		}
		_, _ = g.store.RecordOutbound(ctx, message, staleClaimNotice, model.StatusReplied)
		g.emitEvent(ctx, "connect.message.requeued", message, map[string]any{"reason": "claim_timeout"})
	}
	return recovered, nil
}

func (g *Gateway) SendReply(ctx context.Context, messageID, text, status string) error {
	text = strings.TrimSpace(text)
	if text == "" || len(text) > maxInboundBytes || !utf8.ValidString(text) {
		return errors.New("reply text is empty, invalid, or too large")
	}
	if status == "" {
		status = model.StatusReplied
	}
	if status != model.StatusReplied && status != model.StatusNeedsLocalApproval {
		return errors.New("reply status must be replied or needs_local_approval")
	}
	message, err := g.store.Message(ctx, messageID)
	if err != nil {
		return err
	}
	if message.Direction != "inbound" || !messageCanReceiveReply(message.Status) {
		return errors.New("message is not available for reply")
	}
	outbound := model.OutboundMessage{
		MessageID:      message.ID,
		SenderID:       message.SenderID,
		ConversationID: message.ConversationID,
		ThreadID:       message.ThreadID,
		ReplyTo:        message.ReplyTo,
		Text:           text,
	}
	if err := g.send(ctx, message.Channel, outbound); err != nil {
		_ = g.store.UpdateMessageStatus(ctx, message.ID, message.Status, "channel delivery failed")
		g.emitEvent(ctx, "connect.delivery.failed", message, map[string]any{"transport": message.Channel, "errorClass": "send"})
		return err
	}
	if err := g.store.UpdateMessageStatus(ctx, message.ID, status, ""); err != nil {
		return err
	}
	_, _ = g.store.RecordOutbound(ctx, message, text, status)
	finishWorkspaceMessage(message, status)
	g.emitEvent(ctx, "connect.response.sent", message, map[string]any{"status": status})
	return nil
}

func messageCanReceiveReply(status string) bool {
	return status == model.StatusQueued || status == model.StatusClaimed ||
		status == model.StatusSubmitted || status == model.StatusDeliveryUnknown
}

func (g *Gateway) SendProgress(ctx context.Context, messageID, text string, sequence int, final bool, status string) (bool, error) {
	text = strings.TrimSpace(text)
	if text == "" || !utf8.ValidString(text) || utf8.RuneCountInString(text) > maxProgressRunes {
		return false, fmt.Errorf("progress text must contain 1 to %d valid characters", maxProgressRunes)
	}
	if sequence <= 0 {
		return false, errors.New("progress sequence must be greater than zero")
	}
	if status == "" {
		status = model.StatusReplied
	}
	if status != model.StatusReplied && status != model.StatusNeedsLocalApproval {
		return false, errors.New("progress status must be replied or needs_local_approval")
	}

	g.progressMu.Lock()
	defer g.progressMu.Unlock()

	message, err := g.store.Message(ctx, messageID)
	if err != nil {
		return false, err
	}
	if message.Direction != "inbound" || !messageCanReceiveReply(message.Status) {
		return false, errors.New("message is not available for progress delivery")
	}

	stateKey := "progress.message." + message.ID
	state := progressState{}
	if raw, getErr := g.store.GetMeta(ctx, stateKey); getErr == nil && raw != "" {
		_ = json.Unmarshal([]byte(raw), &state)
	}
	if sequence <= state.Sequence {
		return false, nil
	}

	g.mu.RLock()
	sender := g.senders[message.Channel]
	g.mu.RUnlock()
	progressSender, ok := sender.(ProgressSender)
	if !ok {
		return false, fmt.Errorf("%s channel does not support progress updates", message.Channel)
	}
	platformMessageID := state.PlatformMessageID
	if state.PlatformMessageID == "" || state.Text != text {
		outbound := model.OutboundMessage{
			MessageID:      message.ID + ":progress",
			SenderID:       message.SenderID,
			ConversationID: message.ConversationID,
			ThreadID:       message.ThreadID,
			ReplyTo:        message.ReplyTo,
			Text:           text,
		}
		platformMessageID, err = progressSender.SendProgress(ctx, outbound, state.PlatformMessageID)
		if err != nil {
			_ = g.store.UpdateMessageStatus(ctx, message.ID, message.Status, "channel progress delivery failed")
			g.emitEvent(ctx, "connect.delivery.failed", message, map[string]any{"transport": message.Channel, "errorClass": "progress"})
			return false, err
		}
	}
	state = progressState{
		PlatformMessageID: platformMessageID,
		Sequence:          sequence,
		Text:              text,
		UpdatedAt:         model.UnixMillis(),
	}
	encoded, _ := json.Marshal(state)
	if err := g.store.SetMeta(ctx, stateKey, string(encoded)); err != nil {
		return false, err
	}
	if !final {
		g.emitEvent(ctx, "connect.response.progress", message, map[string]any{"sequence": sequence})
		return true, nil
	}
	if err := g.store.UpdateMessageStatus(ctx, message.ID, status, ""); err != nil {
		return false, err
	}
	_, _ = g.store.RecordOutbound(ctx, message, text, status)
	finishWorkspaceMessage(message, status)
	g.emitEvent(ctx, "connect.response.sent", message, map[string]any{"status": status, "semiStream": true, "sequence": sequence})
	return true, nil
}

func (g *Gateway) ScanOutboxes(ctx context.Context) error {
	return g.scanOutboxes(ctx, nil)
}

func (g *Gateway) ScanOutboxesForSender(ctx context.Context, senderID string) error {
	senderID = strings.TrimSpace(senderID)
	return g.scanOutboxes(ctx, func(route model.Route) bool {
		return route.SenderID == senderID
	})
}

func (g *Gateway) scanOutboxes(ctx context.Context, routeFilter func(model.Route) bool) error {
	routes, err := g.store.ListRoutes(ctx)
	if err != nil {
		return err
	}
	for _, route := range routes {
		if route.WorkspacePath == "" {
			continue
		}
		if routeFilter != nil && !routeFilter(route) {
			continue
		}
		items, err := readOutbox(route.WorkspacePath)
		if err != nil {
			continue
		}
		for _, item := range items {
			status := item.Reply.Status
			if status == "completed" {
				status = model.StatusReplied
			}
			var sendErr error
			if item.Reply.Final && len(item.Reply.ArtifactRefs) > 0 {
				sequence := item.Reply.Sequence
				if sequence <= 0 {
					sequence = 1
				}
				_, sendErr = g.SendProgress(
					ctx,
					item.Reply.MessageID,
					item.Reply.Text,
					sequence,
					false,
					status,
				)
				if sendErr == nil {
					sendErr = g.sendReplyArtifacts(ctx, item.Reply.MessageID, item.Reply.ArtifactRefs)
				}
				if sendErr == nil {
					_, sendErr = g.SendProgress(
						ctx,
						item.Reply.MessageID,
						item.Reply.Text,
						sequence+1,
						true,
						status,
					)
				}
			} else if item.Reply.Sequence > 0 || item.Reply.Final {
				_, sendErr = g.SendProgress(
					ctx,
					item.Reply.MessageID,
					item.Reply.Text,
					item.Reply.Sequence,
					item.Reply.Final,
					status,
				)
			} else {
				sendErr = g.SendReply(ctx, item.Reply.MessageID, item.Reply.Text, status)
			}
			if sendErr == nil {
				_ = os.Remove(item.Path)
			}
		}
	}
	return nil
}

func (g *Gateway) sendReplyArtifacts(ctx context.Context, messageID string, references []string) error {
	if len(references) == 0 {
		return nil
	}
	if len(references) > maxOutboundArtifacts {
		return errors.New("reply contains too many artifact images")
	}
	message, err := g.store.Message(ctx, strings.TrimSpace(messageID))
	if err != nil {
		return err
	}
	if message.Direction != "inbound" || !messageCanReceiveReply(message.Status) {
		return errors.New("message is not available for artifact delivery")
	}
	g.mu.RLock()
	sender := g.senders[message.Channel]
	g.mu.RUnlock()
	attachmentSender, ok := sender.(AttachmentSender)
	if !ok {
		return fmt.Errorf("%s channel does not support artifact delivery", message.Channel)
	}
	seen := make(map[string]struct{}, len(references))
	for _, reference := range references {
		reference = strings.TrimSpace(reference)
		if _, exists := seen[reference]; exists {
			continue
		}
		seen[reference] = struct{}{}
		artifact, err := g.artifactResolver.Resolve(ctx, reference, message.CreatedAt-10*time.Minute.Milliseconds())
		if err != nil {
			g.emitEvent(ctx, "connect.delivery.failed", message, map[string]any{"transport": message.Channel, "errorClass": "artifact_resolve"})
			return err
		}
		stage := message.Channel + "_artifact:" + artifact.ArtifactID
		attempt, lookupErr := g.store.DeliveryAttempt(ctx, message.ID, stage)
		switch {
		case lookupErr == nil && attempt.Status == model.StatusSubmitted:
			continue
		case lookupErr == nil && attempt.Status == model.StatusDeliveryUnknown:
			return errors.New("artifact delivery is unknown and will not be retried automatically")
		case lookupErr == nil && attempt.Status == "started":
			attempt.Status = model.StatusDeliveryUnknown
			_ = g.store.RecordDeliveryAttempt(ctx, attempt)
			return errors.New("artifact delivery was interrupted and will not be retried automatically")
		case lookupErr != nil && !errors.Is(lookupErr, sql.ErrNoRows):
			return lookupErr
		}
		attempt = model.DeliveryAttempt{
			AttemptID:     "artifact:" + message.ID + ":" + artifact.ArtifactID,
			MessageID:     message.ID,
			Stage:         stage,
			Status:        "started",
			ContentSHA256: artifact.SHA256,
		}
		if err := g.store.RecordDeliveryAttempt(ctx, attempt); err != nil {
			return err
		}
		outbound := model.OutboundMessage{
			MessageID:      message.ID + ":artifact:" + artifact.ArtifactID,
			SenderID:       message.SenderID,
			ConversationID: message.ConversationID,
			ThreadID:       message.ThreadID,
			ReplyTo:        message.ReplyTo,
			Attachments:    []model.OutboundAttachment{artifact},
		}
		platformMessageID, sendErr := attachmentSender.SendAttachment(ctx, outbound, artifact)
		if sendErr != nil {
			attempt.Status = model.StatusDeliveryUnknown
			_ = g.store.RecordDeliveryAttempt(ctx, attempt)
			g.emitEvent(ctx, "connect.delivery.failed", message, map[string]any{"transport": message.Channel, "errorClass": "artifact_send"})
			return sendErr
		}
		attempt.Status = model.StatusSubmitted
		attempt.PlatformMessageID = platformMessageID
		if err := g.store.RecordDeliveryAttempt(ctx, attempt); err != nil {
			return err
		}
		g.emitEvent(ctx, "connect.response.artifact_sent", message, map[string]any{"artifactId": artifact.ArtifactID})
	}
	return nil
}

func (g *Gateway) emitEvent(ctx context.Context, eventType string, message model.StoredMessage, metadata map[string]any) {
	data, _ := json.Marshal(metadata)
	_ = g.store.EmitEvent(ctx, model.ResearchEvent{
		Type:      eventType,
		MessageID: message.ID,
		BindingID: message.BindingID,
		Channel:   message.Channel,
	}, string(data))
}

func (g *Gateway) RunMaintenance(ctx context.Context, retentionDays int) {
	outboxTicker := time.NewTicker(time.Second)
	claimTicker := time.NewTicker(15 * time.Second)
	cleanupTicker := time.NewTicker(12 * time.Hour)
	defer outboxTicker.Stop()
	defer claimTicker.Stop()
	defer cleanupTicker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-outboxTicker.C:
			_ = g.ScanOutboxes(ctx)
		case <-claimTicker.C:
			_, _ = g.RecoverStaleClaims(ctx, staleClaimTimeout)
		case <-cleanupTicker.C:
			_, _ = g.store.Cleanup(ctx, retentionDays)
		}
	}
}
