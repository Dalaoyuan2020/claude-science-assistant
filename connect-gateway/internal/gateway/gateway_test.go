package gateway

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/store"
)

type memorySender struct {
	values         []model.OutboundMessage
	progressValues []model.OutboundMessage
	progressIDs    []string
	attachments    []model.OutboundAttachment
}

func (sender *memorySender) Send(ctx context.Context, value model.OutboundMessage) error {
	sender.values = append(sender.values, value)
	return nil
}

func (sender *memorySender) SendProgress(ctx context.Context, value model.OutboundMessage, platformMessageID string) (string, error) {
	sender.progressValues = append(sender.progressValues, value)
	sender.progressIDs = append(sender.progressIDs, platformMessageID)
	if platformMessageID == "" {
		return "platform-progress-1", nil
	}
	return platformMessageID, nil
}

func (sender *memorySender) SendAttachment(ctx context.Context, value model.OutboundMessage, attachment model.OutboundAttachment) (string, error) {
	sender.attachments = append(sender.attachments, attachment)
	return "platform-artifact-1", nil
}

func pairAndBind(t *testing.T, ctx context.Context, dataStore *store.Store, connectGateway *Gateway, sender *memorySender, root string) string {
	t.Helper()
	code, _, err := dataStore.CreatePairingCode(ctx, "telegram", 10*time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.HandleInbound(ctx, inbound("helper-pair", "/pair "+code)); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.HandleInbound(ctx, inbound("helper-route", "建立测试路由")); err != nil {
		t.Fatal(err)
	}
	routes, err := dataStore.ListRoutes(ctx)
	if err != nil || len(routes) != 1 {
		t.Fatalf("routes = %#v, err = %v", routes, err)
	}
	workspace := filepath.Join(root, "workspace")
	if err := os.MkdirAll(workspace, 0o700); err != nil {
		t.Fatal(err)
	}
	if _, err := connectGateway.BindRoute(ctx, routes[0].RouteKey, workspace, ""); err != nil {
		t.Fatal(err)
	}
	pending, err := connectGateway.ListPending(ctx, workspace, 10)
	if err != nil {
		t.Fatal(err)
	}
	for _, message := range pending {
		if err := connectGateway.Expire(ctx, message.ID); err != nil {
			t.Fatal(err)
		}
	}
	return workspace
}

func TestPairBindClaimReplyAndDeduplicate(t *testing.T) {
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

	code, _, err := dataStore.CreatePairingCode(ctx, "telegram", 10*time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	pair := inbound("pair-1", "/pair "+code)
	if err := connectGateway.HandleInbound(ctx, pair); err != nil {
		t.Fatal(err)
	}
	if len(sender.values) != 1 {
		t.Fatalf("pair response count = %d", len(sender.values))
	}

	first := inbound("event-1", "请帮我分析这个研究问题")
	if err := connectGateway.HandleInbound(ctx, first); err != nil {
		t.Fatal(err)
	}
	routes, err := dataStore.ListRoutes(ctx)
	if err != nil || len(routes) != 1 {
		t.Fatalf("routes = %#v, err = %v", routes, err)
	}
	if routes[0].BindingID != "" {
		t.Fatal("new route should wait for a local binding")
	}

	workspace := filepath.Join(root, "workspace")
	if err := os.MkdirAll(filepath.Join(workspace, ".git", "info"), 0o700); err != nil {
		t.Fatal(err)
	}
	route, err := connectGateway.BindRoute(ctx, routes[0].RouteKey, workspace, "")
	if err != nil {
		t.Fatal(err)
	}
	if route.BindingID == "" {
		t.Fatal("binding ID was not assigned")
	}

	second := inbound("event-2", "继续，给出可验证的下一步")
	if err := connectGateway.HandleInbound(ctx, second); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.HandleInbound(ctx, second); err != nil {
		t.Fatal(err)
	}
	pending, err := connectGateway.ListPending(ctx, workspace, 20)
	if err != nil {
		t.Fatal(err)
	}
	if len(pending) != 2 {
		t.Fatalf("pending count = %d, want 2", len(pending))
	}
	if _, err := os.Stat(filepath.Join(workspace, ".csa", "connect", "v1", "inbox", pending[0].ID+".json")); err != nil {
		t.Fatalf("workspace envelope missing: %v", err)
	}
	exclude, err := os.ReadFile(filepath.Join(workspace, ".git", "info", "exclude"))
	if err != nil || string(exclude) != "/.csa/connect/\n" {
		t.Fatalf("git exclude = %q, err = %v", string(exclude), err)
	}

	claimed, err := connectGateway.Claim(ctx, pending[0].ID, workspace)
	if err != nil {
		t.Fatal(err)
	}
	if claimed.Status != model.StatusClaimed {
		t.Fatalf("claim status = %s", claimed.Status)
	}
	if err := connectGateway.MarkDelivery(ctx, claimed.ID, "browser-inject:"+claimed.ID, model.StatusSubmitted); err != nil {
		t.Fatal(err)
	}
	attempt, err := dataStore.DeliveryAttempt(ctx, claimed.ID, "browser_inject")
	if err != nil || attempt.Status != model.StatusSubmitted {
		t.Fatalf("delivery attempt status = %q, err = %v", attempt.Status, err)
	}
	if err := connectGateway.SendReply(ctx, claimed.ID, "这是经过上下文处理的回复。", model.StatusReplied); err != nil {
		t.Fatal(err)
	}
	if len(sender.values) != 3 || len(sender.progressValues) != 1 {
		t.Fatalf("outbound count = %d, duplicate event may have replied", len(sender.values))
	}
	stored, err := dataStore.Message(ctx, claimed.ID)
	if err != nil || stored.Status != model.StatusReplied {
		t.Fatalf("stored status = %q, err = %v", stored.Status, err)
	}
	if _, err := os.Stat(filepath.Join(workspace, ".csa", "connect", "v1", "inbox", claimed.ID+".json")); !os.IsNotExist(err) {
		t.Fatalf("claimed inbox file should be removed, err = %v", err)
	}
	if err := connectGateway.Expire(ctx, pending[1].ID); err != nil {
		t.Fatal(err)
	}
	expired, err := dataStore.Message(ctx, pending[1].ID)
	if err != nil || expired.Status != model.StatusExpired {
		t.Fatalf("expired status = %q, err = %v", expired.Status, err)
	}
	if _, err := os.Stat(filepath.Join(workspace, ".csa", "connect", "v1", "inbox", pending[1].ID+".json")); !os.IsNotExist(err) {
		t.Fatalf("expired inbox file should be removed, err = %v", err)
	}
	third := inbound("event-3", "验证超时恢复")
	if err := connectGateway.HandleInbound(ctx, third); err != nil {
		t.Fatal(err)
	}
	pending, err = connectGateway.ListPending(ctx, workspace, 20)
	if err != nil || len(pending) != 1 {
		t.Fatalf("timeout pending count = %d, err = %v", len(pending), err)
	}
	stale, err := connectGateway.Claim(ctx, pending[0].ID, workspace)
	if err != nil {
		t.Fatal(err)
	}
	time.Sleep(time.Millisecond)
	recovered, err := connectGateway.RecoverStaleClaims(ctx, 0)
	if err != nil || recovered != 1 {
		t.Fatalf("recovered = %d, err = %v", recovered, err)
	}
	recovered, err = connectGateway.RecoverStaleClaims(ctx, 0)
	if err != nil || recovered != 0 {
		t.Fatalf("duplicate recovered = %d, err = %v", recovered, err)
	}
	staleAfter, err := dataStore.Message(ctx, stale.ID)
	if err != nil || staleAfter.Status != model.StatusQueued {
		t.Fatalf("stale status = %q, err = %v", staleAfter.Status, err)
	}
	if len(sender.values) != 4 {
		t.Fatalf("timeout notice count = %d, want 4 direct sends", len(sender.values))
	}
	if sender.values[3].Text != staleClaimNotice {
		t.Fatalf("timeout notice text = %q", sender.values[3].Text)
	}
	history, err := dataStore.History(ctx, 0, 20)
	if err != nil || len(history) != 5 {
		t.Fatalf("history count = %d, err = %v", len(history), err)
	}
	events, err := dataStore.ResearchEvents(ctx, 50)
	if err != nil || len(events) < 6 {
		t.Fatalf("research events = %d, err = %v", len(events), err)
	}
}

func TestDeliveryUnknownStopsAutomaticClaimRecoveryButStillAcceptsReply(t *testing.T) {
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
	message := inbound("delivery-unknown-event", "只允许提交一次")
	if err := connectGateway.HandleInbound(ctx, message); err != nil {
		t.Fatal(err)
	}
	pending, err := connectGateway.ListPending(ctx, workspace, 10)
	if err != nil || len(pending) != 1 {
		t.Fatalf("pending = %d, err = %v", len(pending), err)
	}
	claimed, err := connectGateway.Claim(ctx, pending[0].ID, workspace)
	if err != nil {
		t.Fatal(err)
	}
	attemptID := "browser-inject:" + claimed.ID
	if err := connectGateway.MarkDelivery(ctx, claimed.ID, attemptID, model.StatusDeliveryUnknown); err != nil {
		t.Fatal(err)
	}
	if recovered, err := connectGateway.RecoverStaleClaims(ctx, 0); err != nil || recovered != 0 {
		t.Fatalf("delivery_unknown recovered = %d, err = %v", recovered, err)
	}
	if err := connectGateway.SendReply(ctx, claimed.ID, "真实回复仍可收敛。", model.StatusReplied); err != nil {
		t.Fatal(err)
	}
	stored, err := dataStore.Message(ctx, claimed.ID)
	if err != nil || stored.Status != model.StatusReplied {
		t.Fatalf("final status = %q, err = %v", stored.Status, err)
	}
}

func TestImageAttachmentIsMaterializedOnceWithoutLeakingStoragePath(t *testing.T) {
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

	imageData := []byte{0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46}
	message := inbound("image-event-1", "请分析图片")
	message.Attachments = []model.InboundAttachment{{
		Kind:     "image",
		MIMEType: "image/jpeg",
		FileName: "../unsafe-name.jpg",
		Data:     append([]byte(nil), imageData...),
	}}
	duplicate := inbound("image-event-1", "请分析图片")
	duplicate.Attachments = []model.InboundAttachment{{
		Kind: "image", MIMEType: "image/jpeg", FileName: "../unsafe-name.jpg",
		Data: append([]byte(nil), imageData...),
	}}
	if err := connectGateway.HandleInbound(ctx, duplicate); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.HandleInbound(ctx, message); err != nil {
		t.Fatal(err)
	}
	pending, err := connectGateway.ListPending(ctx, workspace, 10)
	if err != nil || len(pending) != 1 {
		t.Fatalf("pending image messages = %d, err = %v", len(pending), err)
	}
	stored := pending[0]
	if stored.Kind != "mixed" || len(stored.Attachments) != 1 {
		t.Fatalf("stored image message = %#v", stored)
	}
	attachment := stored.Attachments[0]
	if attachment.MIMEType != "image/jpeg" || attachment.FileName != "unsafe-name.jpg" || attachment.State != "available" {
		t.Fatalf("attachment metadata = %#v", attachment)
	}
	_, attachmentPath, err := dataStore.AttachmentPath(ctx, attachment.AttachmentID)
	if err != nil {
		t.Fatal(err)
	}
	if data, err := os.ReadFile(attachmentPath); err != nil || len(data) != 10 {
		t.Fatalf("attachment data length = %d, err = %v", len(data), err)
	}
	envelopePath := filepath.Join(workspace, ".csa", "connect", "v1", "inbox", stored.ID+".json")
	envelope, err := os.ReadFile(envelopePath)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(envelope), `"attachments"`) || strings.Contains(string(envelope), attachment.StorageKey) || strings.Contains(string(envelope), attachmentPath) {
		t.Fatalf("workspace envelope leaked storage details: %s", envelope)
	}
}

func TestTelegramStartDeepLinkPairsLikePairCommand(t *testing.T) {
	ctx := context.Background()
	dataStore, err := store.Open(filepath.Join(t.TempDir(), "connect.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer dataStore.Close()
	connectGateway := New(dataStore)
	sender := &memorySender{}
	connectGateway.RegisterSender("telegram", sender)
	code, _, err := dataStore.CreatePairingCode(ctx, "telegram", 10*time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.HandleInbound(ctx, inbound("start-pair", "/start "+code)); err != nil {
		t.Fatal(err)
	}
	paired, err := dataStore.ChannelPaired(ctx, "telegram")
	if err != nil || !paired {
		t.Fatalf("paired = %v, err = %v", paired, err)
	}
	if len(sender.values) != 1 {
		t.Fatalf("pair response count = %d", len(sender.values))
	}
}

func inbound(eventID, text string) model.InboundMessage {
	return model.InboundMessage{
		Channel:         "telegram",
		AccountID:       "123456",
		PlatformEventID: eventID,
		SenderID:        "9001",
		ConversationID:  "9001",
		ThreadID:        "9001",
		ReplyTo:         "42",
		ChatType:        "private",
		Text:            text,
		CreatedAt:       model.UnixMillis(),
	}
}

func TestGroupAndUnknownCommandNeverEnterQueue(t *testing.T) {
	ctx := context.Background()
	dataStore, err := store.Open(filepath.Join(t.TempDir(), "connect.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer dataStore.Close()
	connectGateway := New(dataStore)
	sender := &memorySender{}
	connectGateway.RegisterSender("telegram", sender)
	code, _, _ := dataStore.CreatePairingCode(ctx, "telegram", time.Minute)
	_ = connectGateway.HandleInbound(ctx, inbound("pair", "/pair "+code))
	group := inbound("group", "group content")
	group.ChatType = "group"
	if err := connectGateway.HandleInbound(ctx, group); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.HandleInbound(ctx, inbound("command", "/shell whoami")); err != nil {
		t.Fatal(err)
	}
	history, _ := dataStore.History(ctx, 0, 10)
	if len(history) != 0 {
		t.Fatalf("unsafe input entered history: %#v", history)
	}
}

func TestProgressUpdatesOnePlatformMessageAndFinalizesOnce(t *testing.T) {
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

	code, _, _ := dataStore.CreatePairingCode(ctx, "telegram", time.Minute)
	if err := connectGateway.HandleInbound(ctx, inbound("progress-pair", "/pair "+code)); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.HandleInbound(ctx, inbound("progress-route", "建立远程研究会话")); err != nil {
		t.Fatal(err)
	}
	routes, _ := dataStore.ListRoutes(ctx)
	workspace := filepath.Join(root, "workspace")
	if err := os.MkdirAll(workspace, 0o700); err != nil {
		t.Fatal(err)
	}
	if _, err := connectGateway.BindRoute(ctx, routes[0].RouteKey, workspace, ""); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.HandleInbound(ctx, inbound("progress-message", "请分段回答")); err != nil {
		t.Fatal(err)
	}
	pending, _ := connectGateway.ListPending(ctx, workspace, 20)
	claimed, err := connectGateway.Claim(ctx, pending[len(pending)-1].ID, workspace)
	if err != nil {
		t.Fatal(err)
	}

	updated, err := connectGateway.SendProgress(ctx, claimed.ID, "第一段。", 1, false, "")
	if err != nil || !updated {
		t.Fatalf("first progress updated = %v, err = %v", updated, err)
	}
	updated, err = connectGateway.SendProgress(ctx, claimed.ID, "第一段。\n\n第二段。", 2, false, "")
	if err != nil || !updated {
		t.Fatalf("second progress updated = %v, err = %v", updated, err)
	}
	updated, err = connectGateway.SendProgress(ctx, claimed.ID, "重复序号不应覆盖。", 2, false, "")
	if err != nil || updated {
		t.Fatalf("duplicate progress updated = %v, err = %v", updated, err)
	}
	updated, err = connectGateway.SendProgress(ctx, claimed.ID, "第一段。\n\n第二段。\n\n完成。", 3, true, model.StatusReplied)
	if err != nil || !updated {
		t.Fatalf("final progress updated = %v, err = %v", updated, err)
	}

	if len(sender.progressValues) != 4 {
		t.Fatalf("progress send count = %d, want queue placeholder plus 3 updates", len(sender.progressValues))
	}
	if sender.progressIDs[0] != "" || sender.progressIDs[1] != "platform-progress-1" || sender.progressIDs[2] != "platform-progress-1" || sender.progressIDs[3] != "platform-progress-1" {
		t.Fatalf("platform progress IDs = %#v", sender.progressIDs)
	}
	stored, err := dataStore.Message(ctx, claimed.ID)
	if err != nil || stored.Status != model.StatusReplied {
		t.Fatalf("stored status = %q, err = %v", stored.Status, err)
	}
	history, err := dataStore.History(ctx, 0, 20)
	if err != nil {
		t.Fatal(err)
	}
	outboundCount := 0
	for _, item := range history {
		if item.Direction == "outbound" && item.ReplyTo == claimed.ID {
			outboundCount++
		}
	}
	if outboundCount != 1 {
		t.Fatalf("final outbound history count = %d, want 1", outboundCount)
	}
}

func TestScanOutboxesDeliversOrderedProgressSnapshots(t *testing.T) {
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

	code, _, _ := dataStore.CreatePairingCode(ctx, "telegram", time.Minute)
	if err := connectGateway.HandleInbound(ctx, inbound("scan-progress-pair", "/pair "+code)); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.HandleInbound(ctx, inbound("scan-progress-route", "bind route")); err != nil {
		t.Fatal(err)
	}
	routes, _ := dataStore.ListRoutes(ctx)
	workspace := filepath.Join(root, "workspace")
	if err := os.MkdirAll(workspace, 0o700); err != nil {
		t.Fatal(err)
	}
	if _, err := connectGateway.BindRoute(ctx, routes[0].RouteKey, workspace, ""); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.HandleInbound(ctx, inbound("scan-progress-message", "stream this answer")); err != nil {
		t.Fatal(err)
	}
	pending, _ := connectGateway.ListPending(ctx, workspace, 20)
	claimed, err := connectGateway.Claim(ctx, pending[len(pending)-1].ID, workspace)
	if err != nil {
		t.Fatal(err)
	}

	outbox := filepath.Join(bridgeRoot(workspace), "outbox")
	progress := model.SandboxReplyV1{SchemaVersion: 1, MessageID: claimed.ID, Text: "first", Sequence: 1}
	final := model.SandboxReplyV1{SchemaVersion: 1, MessageID: claimed.ID, Status: model.StatusReplied, Text: "first\n\nfinal", Sequence: 2, Final: true}
	if err := writeJSONAtomic(filepath.Join(outbox, claimed.ID+".00000001.progress.json"), progress); err != nil {
		t.Fatal(err)
	}
	if err := writeJSONAtomic(filepath.Join(outbox, claimed.ID+".json"), final); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.ScanOutboxes(ctx); err != nil {
		t.Fatal(err)
	}

	if len(sender.progressValues) != 3 {
		t.Fatalf("progress send count = %d, want queue placeholder plus 2 updates", len(sender.progressValues))
	}
	if sender.progressValues[1].Text != "first" || sender.progressValues[2].Text != "first\n\nfinal" {
		t.Fatalf("progress texts = %#v", sender.progressValues)
	}
	if sender.progressIDs[0] != "" || sender.progressIDs[1] != "platform-progress-1" || sender.progressIDs[2] != "platform-progress-1" {
		t.Fatalf("progress IDs = %#v", sender.progressIDs)
	}
	stored, err := dataStore.Message(ctx, claimed.ID)
	if err != nil || stored.Status != model.StatusReplied {
		t.Fatalf("stored status = %q, err = %v", stored.Status, err)
	}
	entries, err := os.ReadDir(outbox)
	if err != nil || len(entries) != 0 {
		t.Fatalf("outbox entries = %d, err = %v", len(entries), err)
	}
}

func TestIdenticalFinalProgressCompletesWithoutEditingTelegramAgain(t *testing.T) {
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

	code, _, _ := dataStore.CreatePairingCode(ctx, "telegram", time.Minute)
	if err := connectGateway.HandleInbound(ctx, inbound("same-final-pair", "/pair "+code)); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.HandleInbound(ctx, inbound("same-final-route", "bind route")); err != nil {
		t.Fatal(err)
	}
	routes, _ := dataStore.ListRoutes(ctx)
	workspace := filepath.Join(root, "workspace")
	if err := os.MkdirAll(workspace, 0o700); err != nil {
		t.Fatal(err)
	}
	if _, err := connectGateway.BindRoute(ctx, routes[0].RouteKey, workspace, ""); err != nil {
		t.Fatal(err)
	}
	if err := connectGateway.HandleInbound(ctx, inbound("same-final-message", "answer once")); err != nil {
		t.Fatal(err)
	}
	pending, _ := connectGateway.ListPending(ctx, workspace, 20)
	claimed, err := connectGateway.Claim(ctx, pending[len(pending)-1].ID, workspace)
	if err != nil {
		t.Fatal(err)
	}

	if updated, err := connectGateway.SendProgress(ctx, claimed.ID, "complete text", 1, false, ""); err != nil || !updated {
		t.Fatalf("progress updated = %v, err = %v", updated, err)
	}
	if updated, err := connectGateway.SendProgress(ctx, claimed.ID, "complete text", 2, true, model.StatusReplied); err != nil || !updated {
		t.Fatalf("final updated = %v, err = %v", updated, err)
	}
	if len(sender.progressValues) != 2 {
		t.Fatalf("Telegram sends/edits = %d, want one queue placeholder and one content edit", len(sender.progressValues))
	}
	stored, err := dataStore.Message(ctx, claimed.ID)
	if err != nil || stored.Status != model.StatusReplied {
		t.Fatalf("stored status = %q, err = %v", stored.Status, err)
	}
}
