package store

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
)

func TestCleanupRemovesExpiredAttachmentFileButKeepsActiveFile(t *testing.T) {
	ctx := context.Background()
	dataStore, err := Open(filepath.Join(t.TempDir(), "connect.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer dataStore.Close()

	expired := receiveAttachmentMessage(t, ctx, dataStore, "expired-event", "expired-image.jpg")
	active := receiveAttachmentMessage(t, ctx, dataStore, "active-event", "active-image.jpg")
	if err := dataStore.UpdateMessageStatus(ctx, expired.ID, model.StatusReplied, ""); err != nil {
		t.Fatal(err)
	}
	old := time.Now().Add(-48 * time.Hour).UnixMilli()
	if _, err := dataStore.db.ExecContext(ctx, `UPDATE messages SET updated_at=? WHERE id=?`, old, expired.ID); err != nil {
		t.Fatal(err)
	}

	removed, err := dataStore.Cleanup(ctx, 1)
	if err != nil || removed != 1 {
		t.Fatalf("cleanup removed = %d, err = %v", removed, err)
	}
	if _, err := os.Stat(filepath.Join(dataStore.dataDir, "attachments", "expired-image.jpg")); !os.IsNotExist(err) {
		t.Fatalf("expired attachment still exists, err = %v", err)
	}
	if _, err := os.Stat(filepath.Join(dataStore.dataDir, "attachments", "active-image.jpg")); err != nil {
		t.Fatalf("active attachment was removed: %v", err)
	}
	if _, err := dataStore.Message(ctx, active.ID); err != nil {
		t.Fatalf("active message was removed: %v", err)
	}
}

func receiveAttachmentMessage(t *testing.T, ctx context.Context, dataStore *Store, eventID, storageKey string) model.StoredMessage {
	t.Helper()
	attachment := model.AttachmentV2{
		AttachmentID: eventID + "-attachment",
		Kind:         "image",
		MIMEType:     "image/jpeg",
		FileName:     storageKey,
		SizeBytes:    4,
		SHA256:       "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
		State:        "available",
		StorageKey:   storageKey,
	}
	if err := dataStore.WriteAttachmentAtomic(attachment, []byte{0xff, 0xd8, 0xff, 0xe0}); err != nil {
		t.Fatal(err)
	}
	message, duplicate, err := dataStore.Receive(ctx, model.InboundMessage{
		Channel:         "telegram",
		AccountID:       "bot",
		PlatformEventID: eventID,
		SenderID:        "user",
		ConversationID:  "chat",
		ChatType:        "private",
		Attachments:     []model.InboundAttachment{{Attachment: attachment}},
	})
	if err != nil || duplicate {
		t.Fatalf("receive message duplicate = %v, err = %v", duplicate, err)
	}
	return message
}
