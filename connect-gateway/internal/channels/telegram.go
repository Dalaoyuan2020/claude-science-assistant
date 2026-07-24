package channels

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"path/filepath"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
)

const telegramCursorKey = "telegram.update_offset"

type Telegram struct {
	token     string
	accountID string
	apiBase   string
	client    *http.Client
	cursors   CursorStore
	health    *healthState
}

func NewTelegram(token string, cursors CursorStore) *Telegram {
	accountID := strings.SplitN(token, ":", 2)[0]
	return &Telegram{
		token:     token,
		accountID: accountID,
		apiBase:   "https://api.telegram.org",
		client:    &http.Client{Timeout: 40 * time.Second},
		cursors:   cursors,
		health:    newHealth("telegram", token != "", "Bot API 长轮询"),
	}
}

func (t *Telegram) ID() string { return "telegram" }

func (t *Telegram) Health(ctx context.Context) model.ChannelHealth {
	value := t.health.snapshot()
	if t.cursors != nil {
		if paired, ok := t.cursors.(interface {
			ChannelPaired(context.Context, string) (bool, error)
		}); ok {
			value.Paired, _ = paired.ChannelPaired(ctx, "telegram")
		}
	}
	return value
}

func (t *Telegram) Run(ctx context.Context, handler Handler) error {
	if t.token == "" {
		return errors.New("Telegram is not configured")
	}
	offset := int64(0)
	if t.cursors != nil {
		stored, _ := t.cursors.GetMeta(ctx, telegramCursorKey)
		offset, _ = strconv.ParseInt(stored, 10, 64)
	}
	t.health.running(true, "Bot API 长轮询已启动", "")
	defer t.health.running(false, "Bot API 长轮询已停止", "")
	backoff := time.Second
	for {
		if ctx.Err() != nil {
			return nil
		}
		updates, err := t.getUpdates(ctx, offset)
		if err != nil {
			t.health.running(true, "Bot API 正在重试", safeChannelError(err))
			if !sleepContext(ctx, backoff) {
				return nil
			}
			if backoff < 30*time.Second {
				backoff *= 2
			}
			continue
		}
		backoff = time.Second
		t.health.running(true, "Bot API 长轮询正常", "")
		retryBatch := false
		for _, update := range updates {
			next := update.UpdateID + 1
			inbound, ok := t.toInbound(update)
			if ok {
				t.health.event()
				if inbound.ChatType == "private" {
					skipHandler := false
					if err := t.downloadInboundAttachments(ctx, &inbound); err != nil {
						var permanent permanentAttachmentError
						if errors.As(err, &permanent) {
							_ = t.Send(ctx, model.OutboundMessage{
								MessageID:      "system:" + inbound.PlatformEventID,
								ConversationID: inbound.ConversationID,
								ThreadID:       inbound.ThreadID,
								ReplyTo:        inbound.ReplyTo,
								Text:           "图片未进入队列：" + permanent.Error(),
							})
							skipHandler = true
						} else {
							t.health.running(true, "Telegram 图片正在重试", safeChannelError(err))
							retryBatch = true
							break
						}
					}
					if !skipHandler {
						if err := handler(ctx, inbound); err != nil {
							t.health.running(true, "消息处理正在重试", safeChannelError(err))
							retryBatch = true
							break
						}
					}
				}
			}
			if next > offset {
				offset = next
			}
			if t.cursors != nil {
				_ = t.cursors.SetMeta(ctx, telegramCursorKey, strconv.FormatInt(offset, 10))
			}
		}
		if retryBatch && !sleepContext(ctx, time.Second) {
			return nil
		}
	}
}

func sleepContext(ctx context.Context, duration time.Duration) bool {
	timer := time.NewTimer(duration)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-timer.C:
		return true
	}
}

type telegramResponse[T any] struct {
	OK          bool   `json:"ok"`
	Description string `json:"description"`
	Result      T      `json:"result"`
}

type telegramUpdate struct {
	UpdateID int64            `json:"update_id"`
	Message  *telegramMessage `json:"message"`
}

type telegramMessage struct {
	MessageID       int64               `json:"message_id"`
	MessageThreadID int64               `json:"message_thread_id"`
	Date            int64               `json:"date"`
	Text            string              `json:"text"`
	Caption         string              `json:"caption"`
	Photo           []telegramPhotoSize `json:"photo"`
	Document        *telegramDocument   `json:"document"`
	From            telegramUser        `json:"from"`
	Chat            telegramChat        `json:"chat"`
}

type telegramPhotoSize struct {
	FileID       string `json:"file_id"`
	FileUniqueID string `json:"file_unique_id"`
	Width        int64  `json:"width"`
	Height       int64  `json:"height"`
	FileSize     int64  `json:"file_size"`
}

type telegramDocument struct {
	FileID       string `json:"file_id"`
	FileUniqueID string `json:"file_unique_id"`
	FileName     string `json:"file_name"`
	MIMEType     string `json:"mime_type"`
	FileSize     int64  `json:"file_size"`
}

type permanentAttachmentError struct {
	message string
}

func (e permanentAttachmentError) Error() string { return e.message }

type telegramFile struct {
	FileID   string `json:"file_id"`
	FilePath string `json:"file_path"`
	FileSize int64  `json:"file_size"`
}

type telegramUser struct {
	ID       int64  `json:"id"`
	IsBot    bool   `json:"is_bot"`
	Username string `json:"username"`
}

type telegramChat struct {
	ID   int64  `json:"id"`
	Type string `json:"type"`
}

func (t *Telegram) getUpdates(ctx context.Context, offset int64) ([]telegramUpdate, error) {
	payload := map[string]any{
		"offset":          offset,
		"timeout":         25,
		"allowed_updates": []string{"message"},
	}
	var response telegramResponse[[]telegramUpdate]
	if err := t.call(ctx, "getUpdates", payload, &response); err != nil {
		return nil, err
	}
	return response.Result, nil
}

func (t *Telegram) BotUsername(ctx context.Context) (string, error) {
	if t.token == "" {
		return "", errors.New("Telegram is not configured")
	}
	var response telegramResponse[telegramUser]
	if err := t.call(ctx, "getMe", map[string]any{}, &response); err != nil {
		return "", err
	}
	username := strings.TrimSpace(response.Result.Username)
	if username == "" || len(username) > 64 {
		return "", errors.New("Telegram bot username is unavailable")
	}
	for _, ch := range username {
		if !((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || (ch >= '0' && ch <= '9') || ch == '_') {
			return "", errors.New("Telegram bot username is invalid")
		}
	}
	return username, nil
}

func (t *Telegram) toInbound(update telegramUpdate) (model.InboundMessage, bool) {
	message := update.Message
	if message == nil || message.From.IsBot {
		return model.InboundMessage{}, false
	}
	text := strings.TrimSpace(message.Text)
	if text == "" {
		text = strings.TrimSpace(message.Caption)
	}
	var attachments []model.InboundAttachment
	if len(message.Photo) > 0 {
		largest := message.Photo[0]
		for _, candidate := range message.Photo[1:] {
			if candidate.FileSize > largest.FileSize ||
				(candidate.FileSize == largest.FileSize && candidate.Width*candidate.Height > largest.Width*largest.Height) {
				largest = candidate
			}
		}
		attachments = append(attachments, model.InboundAttachment{
			PlatformFileID: largest.FileID,
			FileUniqueID:   largest.FileUniqueID,
			Kind:           "image",
			MIMEType:       "image/jpeg",
			FileName:       "telegram-photo.jpg",
			SizeBytes:      largest.FileSize,
		})
	} else if message.Document != nil && strings.HasPrefix(strings.ToLower(message.Document.MIMEType), "image/") {
		attachments = append(attachments, model.InboundAttachment{
			PlatformFileID: message.Document.FileID,
			FileUniqueID:   message.Document.FileUniqueID,
			Kind:           "image",
			MIMEType:       message.Document.MIMEType,
			FileName:       message.Document.FileName,
			SizeBytes:      message.Document.FileSize,
		})
	}
	if text == "" && len(attachments) == 0 {
		return model.InboundMessage{}, false
	}
	conversationID := strconv.FormatInt(message.Chat.ID, 10)
	threadID := conversationID
	if message.MessageThreadID != 0 {
		threadID = strconv.FormatInt(message.MessageThreadID, 10)
	}
	chatType := "group"
	if message.Chat.Type == "private" {
		chatType = "private"
	}
	createdAt := message.Date * 1000
	if createdAt == 0 {
		createdAt = model.UnixMillis()
	}
	return model.InboundMessage{
		Channel:         "telegram",
		AccountID:       t.accountID,
		PlatformEventID: strconv.FormatInt(update.UpdateID, 10),
		SenderID:        strconv.FormatInt(message.From.ID, 10),
		ConversationID:  conversationID,
		ThreadID:        threadID,
		ReplyTo:         strconv.FormatInt(message.MessageID, 10),
		ChatType:        chatType,
		Text:            text,
		Attachments:     attachments,
		CreatedAt:       createdAt,
	}, true
}

func (t *Telegram) downloadInboundAttachments(ctx context.Context, inbound *model.InboundMessage) error {
	for index := range inbound.Attachments {
		attachment := &inbound.Attachments[index]
		if attachment.SizeBytes > 20*1024*1024 {
			return permanentAttachmentError{message: "图片超过 20 MB 上限"}
		}
		var response telegramResponse[telegramFile]
		if err := t.call(ctx, "getFile", map[string]any{"file_id": attachment.PlatformFileID}, &response); err != nil {
			return errors.New("Telegram image metadata is unavailable")
		}
		filePath := strings.TrimSpace(response.Result.FilePath)
		if filePath == "" || strings.Contains(filePath, "..") {
			return permanentAttachmentError{message: "Telegram 返回了无效图片路径"}
		}
		request, err := http.NewRequestWithContext(ctx, http.MethodGet,
			strings.TrimRight(t.apiBase, "/")+"/file/bot"+t.token+"/"+strings.TrimLeft(filePath, "/"), nil)
		if err != nil {
			return errors.New("build Telegram image request failed")
		}
		download, err := t.client.Do(request)
		if err != nil {
			return errors.New("Telegram image download is unavailable")
		}
		data, readErr := io.ReadAll(io.LimitReader(download.Body, 20*1024*1024+1))
		closeErr := download.Body.Close()
		if download.StatusCode < 200 || download.StatusCode >= 300 {
			return fmt.Errorf("Telegram image download returned HTTP %d", download.StatusCode)
		}
		if readErr != nil || closeErr != nil {
			return errors.New("read Telegram image failed")
		}
		if len(data) == 0 || len(data) > 20*1024*1024 {
			return permanentAttachmentError{message: "图片为空或超过 20 MB 上限"}
		}
		attachment.Data = data
		attachment.SizeBytes = int64(len(data))
		if attachment.FileName == "" {
			attachment.FileName = filepath.Base(filePath)
		}
	}
	return nil
}

func (t *Telegram) Send(ctx context.Context, outbound model.OutboundMessage) error {
	for _, chunk := range splitText(outbound.Text, 3900) {
		payload := map[string]any{
			"chat_id": outbound.ConversationID,
			"text":    chunk,
		}
		if outbound.ThreadID != "" && outbound.ThreadID != outbound.ConversationID {
			if thread, err := strconv.ParseInt(outbound.ThreadID, 10, 64); err == nil {
				payload["message_thread_id"] = thread
			}
		}
		if outbound.ReplyTo != "" {
			if replyTo, err := strconv.ParseInt(outbound.ReplyTo, 10, 64); err == nil {
				payload["reply_parameters"] = map[string]any{"message_id": replyTo, "allow_sending_without_reply": true}
			}
		}
		var response telegramResponse[json.RawMessage]
		if err := t.call(ctx, "sendMessage", payload, &response); err != nil {
			return err
		}
	}
	return nil
}

func (t *Telegram) SendProgress(ctx context.Context, outbound model.OutboundMessage, platformMessageID string) (string, error) {
	if platformMessageID == "" {
		payload := t.messagePayload(outbound)
		var response telegramResponse[telegramMessage]
		if err := t.call(ctx, "sendMessage", payload, &response); err != nil {
			return "", err
		}
		if response.Result.MessageID == 0 {
			return "", errors.New("Telegram did not return a message ID")
		}
		return strconv.FormatInt(response.Result.MessageID, 10), nil
	}
	messageID, err := strconv.ParseInt(platformMessageID, 10, 64)
	if err != nil {
		return "", errors.New("stored Telegram progress message ID is invalid")
	}
	payload := map[string]any{
		"chat_id":    outbound.ConversationID,
		"message_id": messageID,
		"text":       outbound.Text,
	}
	var response telegramResponse[telegramMessage]
	if err := t.call(ctx, "editMessageText", payload, &response); err != nil {
		return "", err
	}
	return platformMessageID, nil
}

func (t *Telegram) SendAttachment(ctx context.Context, outbound model.OutboundMessage, attachment model.OutboundAttachment) (string, error) {
	if attachment.SizeBytes <= 0 || attachment.SizeBytes > 20*1024*1024 || int64(len(attachment.Data)) != attachment.SizeBytes {
		return "", errors.New("Telegram artifact image is empty or too large")
	}
	method := "sendDocument"
	fieldName := "document"
	if attachment.SizeBytes <= 10*1024*1024 && (attachment.MIMEType == "image/jpeg" || attachment.MIMEType == "image/png") {
		method = "sendPhoto"
		fieldName = "photo"
	}
	message, rejected, err := t.callMultipart(ctx, method, fieldName, outbound, attachment)
	if err != nil && rejected && method == "sendPhoto" {
		message, _, err = t.callMultipart(ctx, "sendDocument", "document", outbound, attachment)
	}
	if err != nil {
		return "", err
	}
	if message.MessageID == 0 {
		return "", errors.New("Telegram did not return an artifact message ID")
	}
	return strconv.FormatInt(message.MessageID, 10), nil
}

func (t *Telegram) callMultipart(
	ctx context.Context,
	method string,
	fieldName string,
	outbound model.OutboundMessage,
	attachment model.OutboundAttachment,
) (telegramMessage, bool, error) {
	var body bytes.Buffer
	writer := multipart.NewWriter(&body)
	fields := map[string]string{"chat_id": outbound.ConversationID}
	if outbound.ThreadID != "" && outbound.ThreadID != outbound.ConversationID {
		if _, err := strconv.ParseInt(outbound.ThreadID, 10, 64); err == nil {
			fields["message_thread_id"] = outbound.ThreadID
		}
	}
	if outbound.ReplyTo != "" {
		if replyTo, err := strconv.ParseInt(outbound.ReplyTo, 10, 64); err == nil {
			encoded, _ := json.Marshal(map[string]any{"message_id": replyTo, "allow_sending_without_reply": true})
			fields["reply_parameters"] = string(encoded)
		}
	}
	for name, value := range fields {
		if err := writer.WriteField(name, value); err != nil {
			return telegramMessage{}, false, errors.New("build Telegram artifact fields failed")
		}
	}
	part, err := writer.CreateFormFile(fieldName, attachment.FileName)
	if err != nil {
		return telegramMessage{}, false, errors.New("build Telegram artifact file failed")
	}
	if _, err := part.Write(attachment.Data); err != nil {
		return telegramMessage{}, false, errors.New("write Telegram artifact file failed")
	}
	if err := writer.Close(); err != nil {
		return telegramMessage{}, false, errors.New("finish Telegram artifact request failed")
	}
	request, err := http.NewRequestWithContext(
		ctx,
		http.MethodPost,
		strings.TrimRight(t.apiBase, "/")+"/bot"+t.token+"/"+method,
		&body,
	)
	if err != nil {
		return telegramMessage{}, false, errors.New("build Telegram artifact request failed")
	}
	request.Header.Set("Content-Type", writer.FormDataContentType())
	response, err := t.client.Do(request)
	if err != nil {
		return telegramMessage{}, false, errors.New("Telegram artifact API is unavailable")
	}
	defer response.Body.Close()
	responseBody, err := io.ReadAll(io.LimitReader(response.Body, 1<<20))
	if err != nil {
		return telegramMessage{}, false, errors.New("read Telegram artifact response failed")
	}
	var decoded telegramResponse[telegramMessage]
	if err := json.Unmarshal(responseBody, &decoded); err != nil {
		return telegramMessage{}, false, errors.New("Telegram artifact API returned invalid JSON")
	}
	if response.StatusCode < 200 || response.StatusCode >= 300 || !decoded.OK {
		rejected := response.StatusCode >= 400 && response.StatusCode < 500 && !decoded.OK
		return telegramMessage{}, rejected, fmt.Errorf("Telegram artifact API rejected the request: %s", truncate(decoded.Description, 160))
	}
	return decoded.Result, false, nil
}

func (t *Telegram) messagePayload(outbound model.OutboundMessage) map[string]any {
	payload := map[string]any{
		"chat_id": outbound.ConversationID,
		"text":    outbound.Text,
	}
	if outbound.ThreadID != "" && outbound.ThreadID != outbound.ConversationID {
		if thread, err := strconv.ParseInt(outbound.ThreadID, 10, 64); err == nil {
			payload["message_thread_id"] = thread
		}
	}
	if outbound.ReplyTo != "" {
		if replyTo, err := strconv.ParseInt(outbound.ReplyTo, 10, 64); err == nil {
			payload["reply_parameters"] = map[string]any{"message_id": replyTo, "allow_sending_without_reply": true}
		}
	}
	return payload
}

func (t *Telegram) call(ctx context.Context, method string, payload any, result any) error {
	data, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	url := strings.TrimRight(t.apiBase, "/") + "/bot" + t.token + "/" + method
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(data))
	if err != nil {
		return errors.New("build Telegram request failed")
	}
	request.Header.Set("Content-Type", "application/json")
	response, err := t.client.Do(request)
	if err != nil {
		return errors.New("Telegram API is unavailable")
	}
	defer response.Body.Close()
	body, err := io.ReadAll(io.LimitReader(response.Body, 1<<20))
	if err != nil {
		return errors.New("read Telegram response failed")
	}
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		return fmt.Errorf("Telegram API returned HTTP %d", response.StatusCode)
	}
	if err := json.Unmarshal(body, result); err != nil {
		return errors.New("Telegram API returned invalid JSON")
	}
	var base telegramResponse[json.RawMessage]
	if err := json.Unmarshal(body, &base); err == nil && !base.OK {
		return fmt.Errorf("Telegram API rejected the request: %s", truncate(base.Description, 160))
	}
	return nil
}

func splitText(text string, maxRunes int) []string {
	if utf8.RuneCountInString(text) <= maxRunes {
		return []string{text}
	}
	runes := []rune(text)
	values := make([]string, 0, (len(runes)+maxRunes-1)/maxRunes)
	for len(runes) > 0 {
		count := maxRunes
		if len(runes) < count {
			count = len(runes)
		}
		values = append(values, string(runes[:count]))
		runes = runes[count:]
	}
	return values
}

func truncate(value string, limit int) string {
	value = strings.ReplaceAll(strings.ReplaceAll(value, "\r", " "), "\n", " ")
	if len(value) <= limit {
		return value
	}
	return value[:limit]
}

func safeChannelError(err error) string {
	if err == nil {
		return ""
	}
	return truncate(err.Error(), 200)
}
