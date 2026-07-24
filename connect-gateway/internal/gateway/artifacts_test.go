package gateway

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/store"
)

type staticArtifactResolver struct {
	attachment model.OutboundAttachment
	err        error
}

func (resolver staticArtifactResolver) Resolve(context.Context, string, int64) (model.OutboundAttachment, error) {
	return resolver.attachment, resolver.err
}

func TestClaudeScienceArtifactResolverReadsManagedImageByVersionID(t *testing.T) {
	root := t.TempDir()
	artifactRoot := filepath.Join(root, "artifacts")
	databasePath := filepath.Join(root, "operon-cli.db")
	artifactID := "97a29003-e0c4-428c-b87b-2d160ae5a3a0"
	versionID := "54809de1-8d3d-4e54-87c2-d882159c2d69"
	relativePath := filepath.Join("proj_test", artifactID, "result.png")
	fullPath := filepath.Join(artifactRoot, relativePath)
	if err := os.MkdirAll(filepath.Dir(fullPath), 0o700); err != nil {
		t.Fatal(err)
	}
	image := []byte{0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x01}
	if err := os.WriteFile(fullPath, image, 0o600); err != nil {
		t.Fatal(err)
	}
	digest := sha256.Sum256(image)
	checksum := hex.EncodeToString(digest[:])
	database, err := sql.Open("sqlite3", databasePath)
	if err != nil {
		t.Fatal(err)
	}
	_, err = database.Exec(`CREATE TABLE artifacts (
  id TEXT PRIMARY KEY, filename TEXT, created_at INTEGER, latest_version_id TEXT
)`)
	if err == nil {
		_, err = database.Exec(`CREATE TABLE artifact_versions (
  id TEXT PRIMARY KEY, artifact_id TEXT, content_type TEXT, size_bytes INTEGER,
  checksum TEXT, storage_path TEXT
)`)
	}
	if err == nil {
		_, err = database.Exec(
			`INSERT INTO artifacts(id, filename, created_at, latest_version_id) VALUES(?, ?, ?, ?)`,
			artifactID, "result.png", model.UnixMillis(), versionID,
		)
	}
	if err == nil {
		_, err = database.Exec(
			`INSERT INTO artifact_versions(id, artifact_id, content_type, size_bytes, checksum, storage_path)
VALUES(?, ?, ?, ?, ?, ?)`,
			versionID, artifactID, "image/png", len(image), checksum, relativePath,
		)
	}
	if closeErr := database.Close(); err != nil || closeErr != nil {
		t.Fatalf("seed artifact DB err=%v close=%v", err, closeErr)
	}

	resolver := &claudeScienceArtifactResolver{databasePath: databasePath, artifactRoot: artifactRoot}
	attachment, err := resolver.Resolve(context.Background(), versionID, model.UnixMillis()-time.Minute.Milliseconds())
	if err != nil {
		t.Fatal(err)
	}
	if attachment.ArtifactID != versionID || attachment.MIMEType != "image/png" || attachment.SHA256 != checksum {
		t.Fatalf("resolved attachment = %#v", attachment)
	}
}

func TestFinalOutboxSendsArtifactOnceAndRemovesMarkerPayload(t *testing.T) {
	ctx := context.Background()
	root := t.TempDir()
	dataStore, err := store.Open(filepath.Join(root, "connect.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer dataStore.Close()
	connectGateway := New(dataStore)
	sender := &memorySender{}
	connectGateway.RegisterSender("telegram", sender)
	workspace := pairAndBind(t, ctx, dataStore, connectGateway, sender, root)

	message := inbound("artifact-reply-event", "请生成一张图")
	if err := connectGateway.HandleInbound(ctx, message); err != nil {
		t.Fatal(err)
	}
	pending, err := connectGateway.ListPending(ctx, workspace, 10)
	if err != nil || len(pending) != 1 {
		t.Fatalf("pending=%d err=%v", len(pending), err)
	}
	claimed, err := connectGateway.Claim(ctx, pending[0].ID, workspace)
	if err != nil {
		t.Fatal(err)
	}
	versionID := "54809de1-8d3d-4e54-87c2-d882159c2d69"
	connectGateway.artifactResolver = staticArtifactResolver{attachment: model.OutboundAttachment{
		ArtifactID: versionID,
		MIMEType:   "image/png",
		FileName:   "result.png",
		SizeBytes:  9,
		SHA256:     "f2dc9da7d1a0da2a072634bedf67965e3a342ee413921dc852856739bae0628d",
		Data:       []byte{0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x01},
	}}
	outbox := filepath.Join(bridgeRoot(workspace), "outbox")
	reply := model.SandboxReplyV1{
		SchemaVersion: 1,
		MessageID:     claimed.ID,
		Status:        model.StatusReplied,
		Text:          "图表已经生成。",
		ArtifactRefs:  []string{versionID, versionID},
		Sequence:      1,
		Final:         true,
	}
	if err := writeJSONAtomic(filepath.Join(outbox, claimed.ID+".json"), reply); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.ScanOutboxes(ctx); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.ScanOutboxes(ctx); err != nil {
		t.Fatal(err)
	}
	if len(sender.attachments) != 1 || sender.attachments[0].ArtifactID != versionID {
		t.Fatalf("sent attachments = %#v", sender.attachments)
	}
	attempt, err := dataStore.DeliveryAttempt(ctx, claimed.ID, "telegram_artifact:"+versionID)
	if err != nil || attempt.Status != model.StatusSubmitted || attempt.PlatformMessageID != "platform-artifact-1" {
		t.Fatalf("artifact attempt=%#v err=%v", attempt, err)
	}
	stored, err := dataStore.Message(ctx, claimed.ID)
	if err != nil || stored.Status != model.StatusReplied {
		t.Fatalf("message status=%q err=%v", stored.Status, err)
	}
	entries, err := os.ReadDir(outbox)
	if err != nil || len(entries) != 0 {
		t.Fatalf("outbox entries=%d err=%v", len(entries), err)
	}
}
