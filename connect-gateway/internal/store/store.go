package store

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
	"github.com/google/uuid"
	_ "github.com/ncruces/go-sqlite3/driver"
)

const schema = `
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS pairing_codes (
  channel TEXT PRIMARY KEY,
  code_hash TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS identities (
  channel TEXT NOT NULL,
  account_id TEXT NOT NULL,
  sender_id TEXT NOT NULL,
  conversation_id TEXT NOT NULL,
  paired_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  PRIMARY KEY(channel, account_id, sender_id)
);

CREATE TABLE IF NOT EXISTS bindings (
  id TEXT PRIMARY KEY,
  route_key TEXT NOT NULL UNIQUE,
  channel TEXT NOT NULL,
  account_id TEXT NOT NULL,
  sender_id TEXT NOT NULL,
  conversation_id TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  workspace_path TEXT NOT NULL,
  native_frame_id TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY,
  schema_version INTEGER NOT NULL,
  platform_event_id TEXT NOT NULL,
  channel TEXT NOT NULL,
  account_id TEXT NOT NULL,
  sender_id TEXT NOT NULL,
  conversation_id TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  binding_id TEXT NOT NULL DEFAULT '',
  kind TEXT NOT NULL,
  text TEXT NOT NULL,
  reply_to TEXT NOT NULL DEFAULT '',
  direction TEXT NOT NULL,
  status TEXT NOT NULL,
  last_error TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(channel, platform_event_id, direction)
);

CREATE INDEX IF NOT EXISTS ix_messages_status ON messages(status, created_at);
CREATE INDEX IF NOT EXISTS ix_messages_binding ON messages(binding_id, status, created_at);

CREATE TABLE IF NOT EXISTS delivery_attempts (
  attempt_id TEXT PRIMARY KEY,
  message_id TEXT NOT NULL,
  stage TEXT NOT NULL,
  status TEXT NOT NULL,
  content_sha256 TEXT NOT NULL DEFAULT '',
  platform_message_id TEXT NOT NULL DEFAULT '',
  lease_until INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(message_id, stage),
  FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS ix_delivery_attempts_message ON delivery_attempts(message_id, stage);

CREATE TABLE IF NOT EXISTS attachments (
  id TEXT PRIMARY KEY,
  message_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  mime_type TEXT NOT NULL,
  file_name TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  sha256 TEXT NOT NULL,
  storage_key TEXT NOT NULL UNIQUE,
  state TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS ix_attachments_message ON attachments(message_id, created_at);

CREATE TABLE IF NOT EXISTS research_events (
  id TEXT PRIMARY KEY,
  type TEXT NOT NULL,
  message_id TEXT NOT NULL DEFAULT '',
  binding_id TEXT NOT NULL DEFAULT '',
  channel TEXT NOT NULL DEFAULT '',
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS ix_research_events_created ON research_events(created_at);
`

type Store struct {
	db      *sql.DB
	dataDir string
}

func Open(path string) (*Store, error) {
	dataDir := filepath.Dir(path)
	if err := os.MkdirAll(dataDir, 0o700); err != nil {
		return nil, fmt.Errorf("create Connect data directory: %w", err)
	}
	db, err := sql.Open("sqlite3", path)
	if err != nil {
		return nil, fmt.Errorf("open Connect database: %w", err)
	}
	db.SetMaxOpenConns(4)
	db.SetMaxIdleConns(2)
	if _, err := db.Exec(schema); err != nil {
		_ = db.Close()
		return nil, fmt.Errorf("initialize Connect database: %w", err)
	}
	if err := os.MkdirAll(filepath.Join(dataDir, "attachments"), 0o700); err != nil {
		_ = db.Close()
		return nil, fmt.Errorf("create Connect attachment directory: %w", err)
	}
	return &Store{db: db, dataDir: dataDir}, nil
}

func (s *Store) Close() error {
	return s.db.Close()
}

func (s *Store) WriteAttachmentAtomic(attachment model.AttachmentV2, data []byte) error {
	if !validStorageKey(attachment.StorageKey) || len(data) == 0 {
		return errors.New("invalid attachment storage key or content")
	}
	destination := filepath.Join(s.dataDir, "attachments", attachment.StorageKey)
	temporary := destination + ".tmp"
	if err := os.WriteFile(temporary, data, 0o600); err != nil {
		return err
	}
	if err := os.Rename(temporary, destination); err != nil {
		_ = os.Remove(temporary)
		return err
	}
	return nil
}

func validStorageKey(value string) bool {
	if len(value) < 16 || len(value) > 96 || strings.Contains(value, "..") {
		return false
	}
	for _, char := range value {
		if !((char >= 'a' && char <= 'z') || (char >= '0' && char <= '9') || char == '.' || char == '-') {
			return false
		}
	}
	return true
}

func (s *Store) AttachmentPath(ctx context.Context, attachmentID string) (model.AttachmentV2, string, error) {
	var value model.AttachmentV2
	err := s.db.QueryRowContext(ctx, `
SELECT id, kind, mime_type, file_name, size_bytes, sha256, state, storage_key
FROM attachments WHERE id=?`, strings.TrimSpace(attachmentID)).Scan(
		&value.AttachmentID, &value.Kind, &value.MIMEType, &value.FileName,
		&value.SizeBytes, &value.SHA256, &value.State, &value.StorageKey)
	if err != nil {
		return value, "", err
	}
	if !validStorageKey(value.StorageKey) {
		return value, "", errors.New("stored attachment key is invalid")
	}
	return value, filepath.Join(s.dataDir, "attachments", value.StorageKey), nil
}

func (s *Store) GetMeta(ctx context.Context, key string) (string, error) {
	var value string
	err := s.db.QueryRowContext(ctx, `SELECT value FROM meta WHERE key = ?`, key).Scan(&value)
	if errors.Is(err, sql.ErrNoRows) {
		return "", nil
	}
	return value, err
}

func (s *Store) SetMeta(ctx context.Context, key, value string) error {
	_, err := s.db.ExecContext(ctx, `
INSERT INTO meta(key, value) VALUES(?, ?)
ON CONFLICT(key) DO UPDATE SET value = excluded.value`, key, value)
	return err
}

func (s *Store) CreatePairingCode(ctx context.Context, channel string, ttl time.Duration) (string, int64, error) {
	if channel != "feishu" && channel != "telegram" {
		return "", 0, errors.New("unsupported channel")
	}
	const alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"
	random := make([]byte, 8)
	if _, err := rand.Read(random); err != nil {
		return "", 0, err
	}
	code := make([]byte, len(random))
	for index, value := range random {
		code[index] = alphabet[int(value)%len(alphabet)]
	}
	now := model.UnixMillis()
	expires := now + ttl.Milliseconds()
	digest := sha256.Sum256(code)
	_, err := s.db.ExecContext(ctx, `
INSERT INTO pairing_codes(channel, code_hash, expires_at, created_at) VALUES(?, ?, ?, ?)
ON CONFLICT(channel) DO UPDATE SET code_hash=excluded.code_hash, expires_at=excluded.expires_at, created_at=excluded.created_at`,
		channel, hex.EncodeToString(digest[:]), expires, now)
	if err != nil {
		return "", 0, err
	}
	return string(code), expires, nil
}

func (s *Store) ConsumePairingCode(ctx context.Context, channel, accountID, senderID, conversationID, code string) (bool, error) {
	digest := sha256.Sum256([]byte(strings.ToUpper(strings.TrimSpace(code))))
	want := hex.EncodeToString(digest[:])
	now := model.UnixMillis()
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return false, err
	}
	defer tx.Rollback()
	var stored string
	var expires int64
	if err := tx.QueryRowContext(ctx, `SELECT code_hash, expires_at FROM pairing_codes WHERE channel = ?`, channel).Scan(&stored, &expires); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return false, nil
		}
		return false, err
	}
	if stored != want || expires < now {
		return false, nil
	}
	if _, err := tx.ExecContext(ctx, `DELETE FROM pairing_codes WHERE channel = ?`, channel); err != nil {
		return false, err
	}
	if _, err := tx.ExecContext(ctx, `DELETE FROM identities WHERE channel = ?`, channel); err != nil {
		return false, err
	}
	if _, err := tx.ExecContext(ctx, `
INSERT INTO identities(channel, account_id, sender_id, conversation_id, paired_at, last_seen_at)
VALUES(?, ?, ?, ?, ?, ?)`, channel, accountID, senderID, conversationID, now, now); err != nil {
		return false, err
	}
	if err := tx.Commit(); err != nil {
		return false, err
	}
	return true, nil
}

func (s *Store) IsPaired(ctx context.Context, channel, accountID, senderID string) (bool, error) {
	var count int
	err := s.db.QueryRowContext(ctx, `
SELECT COUNT(*) FROM identities WHERE channel = ? AND account_id = ? AND sender_id = ?`,
		channel, accountID, senderID).Scan(&count)
	return count == 1, err
}

func (s *Store) TouchIdentity(ctx context.Context, channel, accountID, senderID, conversationID string) error {
	_, err := s.db.ExecContext(ctx, `
UPDATE identities SET conversation_id = ?, last_seen_at = ?
WHERE channel = ? AND account_id = ? AND sender_id = ?`,
		conversationID, model.UnixMillis(), channel, accountID, senderID)
	return err
}

func (s *Store) ChannelPaired(ctx context.Context, channel string) (bool, error) {
	var count int
	err := s.db.QueryRowContext(ctx, `SELECT COUNT(*) FROM identities WHERE channel = ?`, channel).Scan(&count)
	return count > 0, err
}

func (s *Store) Receive(ctx context.Context, inbound model.InboundMessage) (model.StoredMessage, bool, error) {
	threadID := strings.TrimSpace(inbound.ThreadID)
	if threadID == "" {
		threadID = inbound.ConversationID
	}
	routeKey := model.RouteKey(inbound.Channel, inbound.AccountID, inbound.SenderID, inbound.ConversationID, threadID)
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return model.StoredMessage{}, false, err
	}
	defer tx.Rollback()
	var bindingID string
	err = tx.QueryRowContext(ctx, `SELECT id FROM bindings WHERE route_key = ?`, routeKey).Scan(&bindingID)
	if err != nil && !errors.Is(err, sql.ErrNoRows) {
		return model.StoredMessage{}, false, err
	}
	status := model.StatusAuthorized
	if bindingID != "" {
		status = model.StatusQueued
	}
	now := inbound.CreatedAt
	if now <= 0 {
		now = model.UnixMillis()
	}
	kind := "chat"
	attachments := make([]model.AttachmentV2, 0, len(inbound.Attachments))
	for _, inboundAttachment := range inbound.Attachments {
		if inboundAttachment.Attachment.AttachmentID != "" {
			attachments = append(attachments, inboundAttachment.Attachment)
		}
	}
	if len(attachments) > 0 {
		if strings.TrimSpace(inbound.Text) == "" {
			kind = "image"
		} else {
			kind = "mixed"
		}
	}
	message := model.StoredMessage{
		ID:              uuid.NewString(),
		Channel:         inbound.Channel,
		PlatformEventID: inbound.PlatformEventID,
		SenderID:        inbound.SenderID,
		ConversationID:  inbound.ConversationID,
		ThreadID:        threadID,
		BindingID:       bindingID,
		Kind:            kind,
		Text:            inbound.Text,
		Attachments:     attachments,
		ReplyTo:         inbound.ReplyTo,
		Direction:       "inbound",
		Status:          status,
		CreatedAt:       now,
		UpdatedAt:       now,
	}
	result, err := tx.ExecContext(ctx, `
INSERT OR IGNORE INTO messages(
  id, schema_version, platform_event_id, channel, account_id, sender_id,
  conversation_id, thread_id, binding_id, kind, text, reply_to, direction,
  status, created_at, updated_at
) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		message.ID, model.SchemaVersion, message.PlatformEventID, message.Channel, inbound.AccountID,
		message.SenderID, message.ConversationID, message.ThreadID, message.BindingID, message.Kind,
		message.Text, message.ReplyTo, message.Direction, message.Status, message.CreatedAt, message.UpdatedAt)
	if err != nil {
		return model.StoredMessage{}, false, err
	}
	rows, err := result.RowsAffected()
	if err != nil {
		return model.StoredMessage{}, false, err
	}
	if rows == 0 {
		if err := tx.Commit(); err != nil {
			return model.StoredMessage{}, false, err
		}
		existing, err := s.messageByPlatformEvent(ctx, inbound.Channel, inbound.PlatformEventID)
		return existing, true, err
	}
	for _, attachment := range attachments {
		_, err := tx.ExecContext(ctx, `
INSERT INTO attachments(
  id, message_id, kind, mime_type, file_name, size_bytes, sha256,
  storage_key, state, created_at
) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
			attachment.AttachmentID, message.ID, attachment.Kind, attachment.MIMEType,
			attachment.FileName, attachment.SizeBytes, attachment.SHA256,
			attachment.StorageKey, attachment.State, now)
		if err != nil {
			return model.StoredMessage{}, false, err
		}
	}
	if err := tx.Commit(); err != nil {
		return model.StoredMessage{}, false, err
	}
	if bindingID != "" {
		binding, err := s.BindingByID(ctx, bindingID)
		if err == nil {
			message.WorkspacePath = binding.WorkspacePath
		}
	}
	return message, false, nil
}

func (s *Store) messageByPlatformEvent(ctx context.Context, channel, eventID string) (model.StoredMessage, error) {
	row := s.db.QueryRowContext(ctx, messageSelect+` WHERE m.channel = ? AND m.platform_event_id = ? AND m.direction = 'inbound'`, channel, eventID)
	value, err := scanMessage(row)
	if err == nil {
		value.Attachments, err = s.attachmentsForMessage(ctx, value.ID)
	}
	return value, err
}

const messageSelect = `
SELECT m.id, m.channel, m.platform_event_id, m.sender_id, m.conversation_id,
       m.thread_id, m.binding_id, COALESCE(b.workspace_path, ''), m.kind, m.text,
       m.reply_to, m.direction, m.status, m.last_error, m.created_at, m.updated_at
FROM messages m LEFT JOIN bindings b ON b.id = m.binding_id`

type rowScanner interface {
	Scan(dest ...any) error
}

func scanMessage(row rowScanner) (model.StoredMessage, error) {
	var value model.StoredMessage
	err := row.Scan(&value.ID, &value.Channel, &value.PlatformEventID, &value.SenderID,
		&value.ConversationID, &value.ThreadID, &value.BindingID, &value.WorkspacePath,
		&value.Kind, &value.Text, &value.ReplyTo, &value.Direction, &value.Status,
		&value.LastError, &value.CreatedAt, &value.UpdatedAt)
	return value, err
}

func (s *Store) Message(ctx context.Context, messageID string) (model.StoredMessage, error) {
	value, err := scanMessage(s.db.QueryRowContext(ctx, messageSelect+` WHERE m.id = ?`, messageID))
	if err == nil {
		value.Attachments, err = s.attachmentsForMessage(ctx, value.ID)
	}
	return value, err
}

func (s *Store) attachmentsForMessage(ctx context.Context, messageID string) ([]model.AttachmentV2, error) {
	rows, err := s.db.QueryContext(ctx, `
SELECT id, kind, mime_type, file_name, size_bytes, sha256, state, storage_key
FROM attachments WHERE message_id=? ORDER BY created_at, id`, messageID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var values []model.AttachmentV2
	for rows.Next() {
		var value model.AttachmentV2
		if err := rows.Scan(&value.AttachmentID, &value.Kind, &value.MIMEType,
			&value.FileName, &value.SizeBytes, &value.SHA256, &value.State,
			&value.StorageKey); err != nil {
			return nil, err
		}
		values = append(values, value)
	}
	return values, rows.Err()
}

func (s *Store) ListPending(ctx context.Context, workspacePath string, limit int) ([]model.StoredMessage, error) {
	if limit <= 0 || limit > 100 {
		limit = 20
	}
	rows, err := s.db.QueryContext(ctx, messageSelect+`
 WHERE m.direction = 'inbound' AND m.status = ? AND b.workspace_path = ?
 ORDER BY m.created_at ASC LIMIT ?`, model.StatusQueued, filepath.Clean(workspacePath), limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var values []model.StoredMessage
	for rows.Next() {
		value, err := scanMessage(rows)
		if err != nil {
			return nil, err
		}
		value.Attachments, err = s.attachmentsForMessage(ctx, value.ID)
		if err != nil {
			return nil, err
		}
		values = append(values, value)
	}
	return values, rows.Err()
}

func (s *Store) ListClaimedBefore(ctx context.Context, cutoff int64, limit int) ([]model.StoredMessage, error) {
	if limit <= 0 || limit > 100 {
		limit = 20
	}
	rows, err := s.db.QueryContext(ctx, messageSelect+`
 WHERE m.direction = 'inbound' AND m.status = ? AND m.updated_at <= ?
 ORDER BY m.updated_at ASC LIMIT ?`, model.StatusClaimed, cutoff, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var values []model.StoredMessage
	for rows.Next() {
		value, err := scanMessage(rows)
		if err != nil {
			return nil, err
		}
		value.Attachments, err = s.attachmentsForMessage(ctx, value.ID)
		if err != nil {
			return nil, err
		}
		values = append(values, value)
	}
	return values, rows.Err()
}

func (s *Store) RequeueClaimIfStale(ctx context.Context, messageID string, cutoff int64) (bool, error) {
	result, err := s.db.ExecContext(ctx, `
UPDATE messages SET status = ?, updated_at = ?, last_error = ''
WHERE id = ? AND direction = 'inbound' AND status = ? AND updated_at <= ?`,
		model.StatusQueued, model.UnixMillis(), messageID, model.StatusClaimed, cutoff)
	if err != nil {
		return false, err
	}
	rows, _ := result.RowsAffected()
	return rows == 1, nil
}

func (s *Store) Claim(ctx context.Context, messageID, workspacePath string) (model.StoredMessage, error) {
	now := model.UnixMillis()
	result, err := s.db.ExecContext(ctx, `
UPDATE messages SET status = ?, updated_at = ?, last_error = ''
WHERE id = ? AND status = ? AND binding_id IN (SELECT id FROM bindings WHERE workspace_path = ?)`,
		model.StatusClaimed, now, messageID, model.StatusQueued, filepath.Clean(workspacePath))
	if err != nil {
		return model.StoredMessage{}, err
	}
	rows, _ := result.RowsAffected()
	if rows != 1 {
		return model.StoredMessage{}, errors.New("message is not queued for this workspace")
	}
	return s.Message(ctx, messageID)
}

func (s *Store) UpdateMessageStatus(ctx context.Context, messageID, status, lastError string) error {
	switch status {
	case model.StatusQueued, model.StatusClaimed, model.StatusSubmitted, model.StatusDeliveryUnknown,
		model.StatusReplied, model.StatusNeedsLocalApproval, model.StatusFailed, model.StatusExpired:
	default:
		return errors.New("invalid message status")
	}
	result, err := s.db.ExecContext(ctx, `UPDATE messages SET status = ?, last_error = ?, updated_at = ? WHERE id = ?`,
		status, lastError, model.UnixMillis(), messageID)
	if err != nil {
		return err
	}
	rows, _ := result.RowsAffected()
	if rows != 1 {
		return sql.ErrNoRows
	}
	return nil
}

func (s *Store) RecordDeliveryAttempt(ctx context.Context, attempt model.DeliveryAttempt) error {
	attempt.AttemptID = strings.TrimSpace(attempt.AttemptID)
	attempt.MessageID = strings.TrimSpace(attempt.MessageID)
	attempt.Stage = strings.TrimSpace(attempt.Stage)
	if attempt.AttemptID == "" || attempt.MessageID == "" || attempt.Stage == "" {
		return errors.New("delivery attempt identity is incomplete")
	}
	if attempt.Status != "started" && attempt.Status != model.StatusSubmitted && attempt.Status != model.StatusDeliveryUnknown {
		return errors.New("invalid delivery attempt status")
	}
	now := model.UnixMillis()
	if attempt.CreatedAt == 0 {
		attempt.CreatedAt = now
	}
	attempt.UpdatedAt = now
	result, err := s.db.ExecContext(ctx, `
INSERT INTO delivery_attempts(
  attempt_id, message_id, stage, status, content_sha256, platform_message_id,
  lease_until, created_at, updated_at
) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(message_id, stage) DO UPDATE SET
  status=excluded.status,
  content_sha256=CASE WHEN excluded.content_sha256='' THEN delivery_attempts.content_sha256 ELSE excluded.content_sha256 END,
  platform_message_id=CASE WHEN excluded.platform_message_id='' THEN delivery_attempts.platform_message_id ELSE excluded.platform_message_id END,
  lease_until=excluded.lease_until,
  updated_at=excluded.updated_at
WHERE delivery_attempts.attempt_id=excluded.attempt_id`,
		attempt.AttemptID, attempt.MessageID, attempt.Stage, attempt.Status,
		attempt.ContentSHA256, attempt.PlatformMessageID, attempt.LeaseUntil,
		attempt.CreatedAt, attempt.UpdatedAt)
	if err != nil {
		return err
	}
	rows, _ := result.RowsAffected()
	if rows != 1 {
		return errors.New("delivery attempt conflicts with an existing attempt")
	}
	return nil
}

func (s *Store) DeliveryAttempt(ctx context.Context, messageID, stage string) (model.DeliveryAttempt, error) {
	var value model.DeliveryAttempt
	err := s.db.QueryRowContext(ctx, `
SELECT attempt_id, message_id, stage, status, content_sha256, platform_message_id,
       lease_until, created_at, updated_at
FROM delivery_attempts WHERE message_id=? AND stage=?`, strings.TrimSpace(messageID), strings.TrimSpace(stage)).Scan(
		&value.AttemptID, &value.MessageID, &value.Stage, &value.Status,
		&value.ContentSHA256, &value.PlatformMessageID, &value.LeaseUntil,
		&value.CreatedAt, &value.UpdatedAt)
	return value, err
}

func (s *Store) RecordOutbound(ctx context.Context, source model.StoredMessage, text, status string) (model.StoredMessage, error) {
	now := model.UnixMillis()
	value := model.StoredMessage{
		ID:              uuid.NewString(),
		Channel:         source.Channel,
		PlatformEventID: "out:" + source.ID + ":" + uuid.NewString(),
		SenderID:        source.SenderID,
		ConversationID:  source.ConversationID,
		ThreadID:        source.ThreadID,
		BindingID:       source.BindingID,
		WorkspacePath:   source.WorkspacePath,
		Kind:            "chat",
		Text:            text,
		ReplyTo:         source.ID,
		Direction:       "outbound",
		Status:          status,
		CreatedAt:       now,
		UpdatedAt:       now,
	}
	_, err := s.db.ExecContext(ctx, `
INSERT INTO messages(
  id, schema_version, platform_event_id, channel, account_id, sender_id,
  conversation_id, thread_id, binding_id, kind, text, reply_to, direction,
  status, created_at, updated_at
) VALUES(?, ?, ?, ?, '', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		value.ID, model.SchemaVersion, value.PlatformEventID, value.Channel, value.SenderID,
		value.ConversationID, value.ThreadID, value.BindingID, value.Kind, value.Text,
		value.ReplyTo, value.Direction, value.Status, value.CreatedAt, value.UpdatedAt)
	return value, err
}

func (s *Store) UpsertBinding(ctx context.Context, routeKey, workspacePath, nativeFrameID string) (model.Route, []model.StoredMessage, error) {
	workspacePath = filepath.Clean(strings.TrimSpace(workspacePath))
	if !filepath.IsAbs(workspacePath) {
		return model.Route{}, nil, errors.New("workspace path must be absolute")
	}
	info, err := os.Stat(workspacePath)
	if err != nil || !info.IsDir() {
		return model.Route{}, nil, errors.New("workspace path does not exist or is not a directory")
	}
	parts, err := s.routeParts(ctx, routeKey)
	if err != nil {
		return model.Route{}, nil, err
	}
	now := model.UnixMillis()
	bindingID := uuid.NewString()
	_, err = s.db.ExecContext(ctx, `
INSERT INTO bindings(id, route_key, channel, account_id, sender_id, conversation_id, thread_id, workspace_path, native_frame_id, created_at, updated_at)
VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(route_key) DO UPDATE SET workspace_path=excluded.workspace_path, native_frame_id=excluded.native_frame_id, updated_at=excluded.updated_at`,
		bindingID, routeKey, parts.Channel, parts.AccountID, parts.SenderID, parts.ConversationID,
		parts.ThreadID, workspacePath, nativeFrameID, now, now)
	if err != nil {
		return model.Route{}, nil, err
	}
	binding, err := s.BindingByRoute(ctx, routeKey)
	if err != nil {
		return model.Route{}, nil, err
	}
	_, err = s.db.ExecContext(ctx, `
UPDATE messages SET binding_id = ?, status = ?, updated_at = ?
WHERE channel = ? AND account_id = ? AND sender_id = ? AND conversation_id = ? AND thread_id = ?
  AND status = ?`,
		binding.BindingID, model.StatusQueued, now, parts.Channel, parts.AccountID, parts.SenderID,
		parts.ConversationID, parts.ThreadID, model.StatusAuthorized)
	if err != nil {
		return model.Route{}, nil, err
	}
	pending, err := s.ListPending(ctx, workspacePath, 100)
	return binding, pending, err
}

type routeParts struct {
	Channel        string
	AccountID      string
	SenderID       string
	ConversationID string
	ThreadID       string
}

func (s *Store) routeParts(ctx context.Context, routeKey string) (routeParts, error) {
	// Resolve the small personal route list in Go so this remains portable across
	// SQLite builds that do not expose hashing extensions.
	rows, queryErr := s.db.QueryContext(ctx, `
SELECT channel, account_id, sender_id, conversation_id, thread_id
FROM messages WHERE direction = 'inbound'
GROUP BY channel, account_id, sender_id, conversation_id, thread_id`)
	if queryErr != nil {
		return routeParts{}, queryErr
	}
	defer rows.Close()
	for rows.Next() {
		var candidate routeParts
		if err := rows.Scan(&candidate.Channel, &candidate.AccountID, &candidate.SenderID, &candidate.ConversationID, &candidate.ThreadID); err != nil {
			return routeParts{}, err
		}
		if model.RouteKey(candidate.Channel, candidate.AccountID, candidate.SenderID, candidate.ConversationID, candidate.ThreadID) == routeKey {
			return candidate, nil
		}
	}
	return routeParts{}, errors.New("route was not found; send a message from that chat first")
}

func (s *Store) BindingByID(ctx context.Context, bindingID string) (model.Route, error) {
	return scanRoute(s.db.QueryRowContext(ctx, bindingSelect+` WHERE b.id = ?`, bindingID))
}

func (s *Store) BindingByRoute(ctx context.Context, routeKey string) (model.Route, error) {
	return scanRoute(s.db.QueryRowContext(ctx, bindingSelect+` WHERE b.route_key = ?`, routeKey))
}

const bindingSelect = `
SELECT b.route_key, b.channel, b.account_id, b.sender_id, b.conversation_id, b.thread_id,
       b.id, b.workspace_path, b.native_frame_id,
       COALESCE(i.paired_at, 0),
       COALESCE((SELECT MAX(m.created_at) FROM messages m WHERE m.channel=b.channel AND m.sender_id=b.sender_id AND m.conversation_id=b.conversation_id AND m.thread_id=b.thread_id), 0),
       COALESCE((SELECT COUNT(*) FROM messages m WHERE m.binding_id=b.id AND m.status IN ('queued','claimed')), 0)
FROM bindings b
LEFT JOIN identities i ON i.channel=b.channel AND i.account_id=b.account_id AND i.sender_id=b.sender_id`

func scanRoute(row rowScanner) (model.Route, error) {
	var value model.Route
	err := row.Scan(&value.RouteKey, &value.Channel, &value.AccountID, &value.SenderID,
		&value.ConversationID, &value.ThreadID, &value.BindingID, &value.WorkspacePath,
		&value.NativeFrameID, &value.PairedAt, &value.LastMessageAt, &value.PendingMessages)
	return value, err
}

func (s *Store) ListRoutes(ctx context.Context) ([]model.Route, error) {
	rows, err := s.db.QueryContext(ctx, `
SELECT channel, account_id, sender_id, conversation_id, thread_id,
       MAX(created_at) AS last_message_at
FROM messages WHERE direction='inbound'
GROUP BY channel, account_id, sender_id, conversation_id, thread_id
ORDER BY last_message_at DESC`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var routes []model.Route
	for rows.Next() {
		var route model.Route
		if err := rows.Scan(&route.Channel, &route.AccountID, &route.SenderID, &route.ConversationID, &route.ThreadID, &route.LastMessageAt); err != nil {
			return nil, err
		}
		route.RouteKey = model.RouteKey(route.Channel, route.AccountID, route.SenderID, route.ConversationID, route.ThreadID)
		if binding, err := s.BindingByRoute(ctx, route.RouteKey); err == nil {
			route.BindingID = binding.BindingID
			route.WorkspacePath = binding.WorkspacePath
			route.NativeFrameID = binding.NativeFrameID
			route.PairedAt = binding.PairedAt
			route.PendingMessages = binding.PendingMessages
		}
		routes = append(routes, route)
	}
	return routes, rows.Err()
}

func (s *Store) History(ctx context.Context, offset, limit int) ([]model.StoredMessage, error) {
	if offset < 0 {
		offset = 0
	}
	if limit <= 0 || limit > 200 {
		limit = 50
	}
	rows, err := s.db.QueryContext(ctx, messageSelect+` ORDER BY m.created_at DESC LIMIT ? OFFSET ?`, limit, offset)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var values []model.StoredMessage
	for rows.Next() {
		value, err := scanMessage(rows)
		if err != nil {
			return nil, err
		}
		value.Attachments, err = s.attachmentsForMessage(ctx, value.ID)
		if err != nil {
			return nil, err
		}
		values = append(values, value)
	}
	return values, rows.Err()
}

func (s *Store) Counts(ctx context.Context) (model.Counts, error) {
	var values model.Counts
	rows, err := s.db.QueryContext(ctx, `SELECT status, COUNT(*) FROM messages WHERE direction='inbound' GROUP BY status`)
	if err != nil {
		return values, err
	}
	defer rows.Close()
	for rows.Next() {
		var status string
		var count int64
		if err := rows.Scan(&status, &count); err != nil {
			return values, err
		}
		switch status {
		case model.StatusAuthorized:
			values.Authorized = count
		case model.StatusQueued:
			values.Queued = count
		case model.StatusClaimed:
			values.Claimed = count
		case model.StatusSubmitted, model.StatusDeliveryUnknown:
			values.Claimed += count
		case model.StatusReplied:
			values.Replied = count
		case model.StatusNeedsLocalApproval:
			values.NeedsLocalApproval = count
		case model.StatusFailed:
			values.Failed = count
		}
	}
	return values, rows.Err()
}

func (s *Store) ClearHistory(ctx context.Context) (int64, error) {
	storageKeys, err := s.terminalAttachmentStorageKeys(ctx, "")
	if err != nil {
		return 0, err
	}
	result, err := s.db.ExecContext(ctx, `DELETE FROM messages WHERE status IN (?, ?, ?)`,
		model.StatusReplied, model.StatusFailed, model.StatusExpired)
	if err != nil {
		return 0, err
	}
	rows, err := result.RowsAffected()
	if err != nil {
		return 0, err
	}
	return rows, s.removeAttachmentFiles(storageKeys)
}

func (s *Store) Cleanup(ctx context.Context, retentionDays int) (int64, error) {
	cutoff := time.Now().UTC().Add(-time.Duration(retentionDays) * 24 * time.Hour).UnixMilli()
	storageKeys, err := s.terminalAttachmentStorageKeys(ctx, " AND m.updated_at < ?", cutoff)
	if err != nil {
		return 0, err
	}
	result, err := s.db.ExecContext(ctx, `
DELETE FROM messages WHERE updated_at < ? AND status IN (?, ?, ?)`,
		cutoff, model.StatusReplied, model.StatusFailed, model.StatusExpired)
	if err != nil {
		return 0, err
	}
	rows, err := result.RowsAffected()
	if err != nil {
		return 0, err
	}
	return rows, s.removeAttachmentFiles(storageKeys)
}

func (s *Store) terminalAttachmentStorageKeys(ctx context.Context, cutoffClause string, args ...any) ([]string, error) {
	query := `
SELECT a.storage_key
FROM attachments a JOIN messages m ON m.id = a.message_id
WHERE m.status IN (?, ?, ?)` + cutoffClause
	queryArgs := []any{model.StatusReplied, model.StatusFailed, model.StatusExpired}
	queryArgs = append(queryArgs, args...)
	rows, err := s.db.QueryContext(ctx, query, queryArgs...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var storageKeys []string
	for rows.Next() {
		var storageKey string
		if err := rows.Scan(&storageKey); err != nil {
			return nil, err
		}
		if validStorageKey(storageKey) {
			storageKeys = append(storageKeys, storageKey)
		}
	}
	return storageKeys, rows.Err()
}

func (s *Store) removeAttachmentFiles(storageKeys []string) error {
	for _, storageKey := range storageKeys {
		path := filepath.Join(s.dataDir, "attachments", storageKey)
		if err := os.Remove(path); err != nil && !errors.Is(err, os.ErrNotExist) {
			return fmt.Errorf("remove expired Connect attachment: %w", err)
		}
	}
	return nil
}

func (s *Store) EmitEvent(ctx context.Context, event model.ResearchEvent, metadataJSON string) error {
	if event.ID == "" {
		event.ID = uuid.NewString()
	}
	if event.CreatedAt == 0 {
		event.CreatedAt = model.UnixMillis()
	}
	_, err := s.db.ExecContext(ctx, `
INSERT INTO research_events(id, type, message_id, binding_id, channel, metadata_json, created_at)
VALUES(?, ?, ?, ?, ?, ?, ?)`, event.ID, event.Type, event.MessageID, event.BindingID,
		event.Channel, metadataJSON, event.CreatedAt)
	return err
}

func (s *Store) ResearchEvents(ctx context.Context, limit int) ([]model.ResearchEvent, error) {
	if limit <= 0 || limit > 500 {
		limit = 100
	}
	rows, err := s.db.QueryContext(ctx, `
SELECT id, type, message_id, binding_id, channel, created_at
FROM research_events ORDER BY created_at DESC LIMIT ?`, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var values []model.ResearchEvent
	for rows.Next() {
		var value model.ResearchEvent
		if err := rows.Scan(&value.ID, &value.Type, &value.MessageID, &value.BindingID, &value.Channel, &value.CreatedAt); err != nil {
			return nil, err
		}
		values = append(values, value)
	}
	return values, rows.Err()
}
