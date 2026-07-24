package channels

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
)

func TestTelegramParsesPrivateTextAndSendsWithoutLeakingToken(t *testing.T) {
	var sent map[string]any
	var edited map[string]any
	server := httptest.NewServer(http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		if request.URL.Path == "/bot123456:secret-token/getMe" {
			_ = json.NewEncoder(response).Encode(map[string]any{"ok": true, "result": map[string]any{"id": 123456, "is_bot": true, "username": "CSA_Test_Bot"}})
			return
		}
		if request.URL.Path == "/bot123456:secret-token/sendMessage" {
			if err := json.NewDecoder(request.Body).Decode(&sent); err != nil {
				t.Fatal(err)
			}
			_ = json.NewEncoder(response).Encode(map[string]any{"ok": true, "result": map[string]any{"message_id": 1}})
			return
		}
		if request.URL.Path == "/bot123456:secret-token/editMessageText" {
			if err := json.NewDecoder(request.Body).Decode(&edited); err != nil {
				t.Fatal(err)
			}
			_ = json.NewEncoder(response).Encode(map[string]any{"ok": true, "result": map[string]any{"message_id": 1}})
			return
		}
		http.NotFound(response, request)
	}))
	defer server.Close()
	channel := NewTelegram("123456:secret-token", nil)
	channel.apiBase = server.URL
	username, err := channel.BotUsername(context.Background())
	if err != nil || username != "CSA_Test_Bot" {
		t.Fatalf("bot username = %q, err = %v", username, err)
	}
	update := telegramUpdate{UpdateID: 99, Message: &telegramMessage{
		MessageID: 7,
		Date:      10,
		Text:      "hello",
		From:      telegramUser{ID: 42},
		Chat:      telegramChat{ID: 42, Type: "private"},
	}}
	inbound, ok := channel.toInbound(update)
	if !ok || inbound.ChatType != "private" || inbound.PlatformEventID != "99" || inbound.Text != "hello" {
		t.Fatalf("inbound = %#v, ok = %v", inbound, ok)
	}
	if err := channel.Send(context.Background(), model.OutboundMessage{
		MessageID:      "m1",
		ConversationID: "42",
		ThreadID:       "42",
		ReplyTo:        "7",
		Text:           "reply",
	}); err != nil {
		t.Fatal(err)
	}
	if sent["chat_id"] != "42" || sent["text"] != "reply" {
		t.Fatalf("sent payload = %#v", sent)
	}
	progressID, err := channel.SendProgress(context.Background(), model.OutboundMessage{
		MessageID:      "m2:progress",
		ConversationID: "42",
		ThreadID:       "42",
		ReplyTo:        "7",
		Text:           "first paragraph",
	}, "")
	if err != nil || progressID != "1" {
		t.Fatalf("initial progress ID = %q, err = %v", progressID, err)
	}
	progressID, err = channel.SendProgress(context.Background(), model.OutboundMessage{
		MessageID:      "m2:progress",
		ConversationID: "42",
		Text:           "first paragraph\n\nsecond paragraph",
	}, progressID)
	if err != nil || progressID != "1" {
		t.Fatalf("edited progress ID = %q, err = %v", progressID, err)
	}
	if edited["message_id"] != float64(1) || edited["text"] != "first paragraph\n\nsecond paragraph" {
		t.Fatalf("edited payload = %#v", edited)
	}
}

func TestTelegramDownloadsLargestPhotoWithCaption(t *testing.T) {
	image := []byte{0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46}
	server := httptest.NewServer(http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		switch request.URL.Path {
		case "/bot123456:secret-token/getFile":
			_ = json.NewEncoder(response).Encode(map[string]any{
				"ok":     true,
				"result": map[string]any{"file_id": "large", "file_path": "photos/test.jpg", "file_size": len(image)},
			})
		case "/file/bot123456:secret-token/photos/test.jpg":
			response.Header().Set("Content-Type", "image/jpeg")
			_, _ = response.Write(image)
		default:
			http.NotFound(response, request)
		}
	}))
	defer server.Close()

	channel := NewTelegram("123456:secret-token", nil)
	channel.apiBase = server.URL
	update := telegramUpdate{UpdateID: 100, Message: &telegramMessage{
		MessageID: 8,
		Date:      11,
		Caption:   "分析图片",
		Photo: []telegramPhotoSize{
			{FileID: "small", FileUniqueID: "same", Width: 90, Height: 90, FileSize: 100},
			{FileID: "large", FileUniqueID: "same", Width: 900, Height: 900, FileSize: 1000},
		},
		From: telegramUser{ID: 42},
		Chat: telegramChat{ID: 42, Type: "private"},
	}}
	inbound, ok := channel.toInbound(update)
	if !ok || inbound.Text != "分析图片" || len(inbound.Attachments) != 1 {
		t.Fatalf("inbound = %#v, ok = %v", inbound, ok)
	}
	if inbound.Attachments[0].PlatformFileID != "large" {
		t.Fatalf("selected file = %q", inbound.Attachments[0].PlatformFileID)
	}
	if err := channel.downloadInboundAttachments(context.Background(), &inbound); err != nil {
		t.Fatal(err)
	}
	if string(inbound.Attachments[0].Data) != string(image) {
		t.Fatalf("downloaded image = %x", inbound.Attachments[0].Data)
	}
}

func TestTelegramRejectsDeclaredOversizeImageWithoutNetworkRequest(t *testing.T) {
	channel := NewTelegram("123456:secret-token", nil)
	inbound := model.InboundMessage{Attachments: []model.InboundAttachment{{
		Kind:           "image",
		PlatformFileID: "oversize",
		SizeBytes:      20*1024*1024 + 1,
	}}}
	err := channel.downloadInboundAttachments(context.Background(), &inbound)
	var permanent permanentAttachmentError
	if !errors.As(err, &permanent) {
		t.Fatalf("oversize error = %T %v", err, err)
	}
}

func TestTelegramSendsArtifactWithSendPhotoMultipart(t *testing.T) {
	image := []byte{0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x01}
	var received []byte
	var chatID, replyParameters string
	server := httptest.NewServer(http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		if request.URL.Path != "/bot123456:secret-token/sendPhoto" {
			http.NotFound(response, request)
			return
		}
		if err := request.ParseMultipartForm(1 << 20); err != nil {
			t.Fatal(err)
		}
		chatID = request.FormValue("chat_id")
		replyParameters = request.FormValue("reply_parameters")
		file, _, err := request.FormFile("photo")
		if err != nil {
			t.Fatal(err)
		}
		defer file.Close()
		received, err = io.ReadAll(file)
		if err != nil {
			t.Fatal(err)
		}
		_ = json.NewEncoder(response).Encode(map[string]any{"ok": true, "result": map[string]any{"message_id": 91}})
	}))
	defer server.Close()

	channel := NewTelegram("123456:secret-token", nil)
	channel.apiBase = server.URL
	platformID, err := channel.SendAttachment(context.Background(), model.OutboundMessage{
		MessageID: "artifact-message", ConversationID: "42", ThreadID: "42", ReplyTo: "7",
	}, model.OutboundAttachment{
		ArtifactID: "version-id", MIMEType: "image/png", FileName: "result.png",
		SizeBytes: int64(len(image)), Data: image,
	})
	if err != nil || platformID != "91" {
		t.Fatalf("platform ID = %q, err = %v", platformID, err)
	}
	if chatID != "42" || !bytes.Equal(received, image) || !strings.Contains(replyParameters, `"message_id":7`) {
		t.Fatalf("multipart chat=%q reply=%q data=%x", chatID, replyParameters, received)
	}
}
