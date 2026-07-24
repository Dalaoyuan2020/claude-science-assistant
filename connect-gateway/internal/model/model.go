package model

import (
	"crypto/sha256"
	"encoding/hex"
	"strings"
	"time"
)

const (
	SchemaVersion = 1

	StatusReceived           = "received"
	StatusAuthorized         = "authorized"
	StatusBound              = "bound"
	StatusQueued             = "queued"
	StatusClaimed            = "claimed"
	StatusSubmitted          = "submitted"
	StatusDeliveryUnknown    = "delivery_unknown"
	StatusReplied            = "replied"
	StatusNeedsLocalApproval = "needs_local_approval"
	StatusFailed             = "failed"
	StatusExpired            = "expired"
)

type InboundMessage struct {
	Channel         string              `json:"channel"`
	AccountID       string              `json:"accountId"`
	PlatformEventID string              `json:"platformEventId"`
	SenderID        string              `json:"senderId"`
	ConversationID  string              `json:"conversationId"`
	ThreadID        string              `json:"threadId,omitempty"`
	ReplyTo         string              `json:"replyTo,omitempty"`
	ChatType        string              `json:"chatType"`
	Text            string              `json:"text"`
	Attachments     []InboundAttachment `json:"attachments,omitempty"`
	CreatedAt       int64               `json:"createdAt"`
}

type InboundAttachment struct {
	PlatformFileID string       `json:"platformFileId,omitempty"`
	FileUniqueID   string       `json:"fileUniqueId,omitempty"`
	Kind           string       `json:"kind"`
	MIMEType       string       `json:"mimeType,omitempty"`
	FileName       string       `json:"fileName,omitempty"`
	SizeBytes      int64        `json:"sizeBytes,omitempty"`
	Data           []byte       `json:"-"`
	Attachment     AttachmentV2 `json:"-"`
}

type AttachmentV2 struct {
	AttachmentID string `json:"attachmentId"`
	Kind         string `json:"kind"`
	MIMEType     string `json:"mimeType"`
	FileName     string `json:"fileName"`
	SizeBytes    int64  `json:"sizeBytes"`
	SHA256       string `json:"sha256"`
	State        string `json:"state"`
	StorageKey   string `json:"-"`
}

type OutboundMessage struct {
	MessageID      string               `json:"messageId"`
	SenderID       string               `json:"senderId"`
	ConversationID string               `json:"conversationId"`
	ThreadID       string               `json:"threadId,omitempty"`
	ReplyTo        string               `json:"replyTo,omitempty"`
	Text           string               `json:"text"`
	Attachments    []OutboundAttachment `json:"attachments,omitempty"`
}

type OutboundAttachment struct {
	ArtifactID string `json:"artifactId"`
	MIMEType   string `json:"mimeType"`
	FileName   string `json:"fileName"`
	SizeBytes  int64  `json:"sizeBytes"`
	SHA256     string `json:"sha256"`
	Data       []byte `json:"-"`
}

type ConnectEnvelopeV1 struct {
	SchemaVersion   int            `json:"schemaVersion"`
	MessageID       string         `json:"messageId"`
	Channel         string         `json:"channel"`
	PlatformEventID string         `json:"platformEventId"`
	SenderID        string         `json:"senderId"`
	ConversationID  string         `json:"conversationId"`
	ThreadID        string         `json:"threadId,omitempty"`
	BindingID       string         `json:"bindingId"`
	Kind            string         `json:"kind"`
	Text            string         `json:"text"`
	Attachments     []AttachmentV2 `json:"attachments,omitempty"`
	ReplyTo         string         `json:"replyTo,omitempty"`
	CreatedAt       string         `json:"createdAt"`
}

type SandboxReplyV1 struct {
	SchemaVersion int      `json:"schemaVersion"`
	MessageID     string   `json:"messageId"`
	Status        string   `json:"status"`
	Text          string   `json:"text"`
	ArtifactRefs  []string `json:"artifactRefs,omitempty"`
	Sequence      int      `json:"sequence,omitempty"`
	Final         bool     `json:"final,omitempty"`
	CreatedAt     string   `json:"createdAt,omitempty"`
}

type StoredMessage struct {
	ID              string         `json:"messageId"`
	Channel         string         `json:"channel"`
	PlatformEventID string         `json:"platformEventId"`
	SenderID        string         `json:"senderId"`
	ConversationID  string         `json:"conversationId"`
	ThreadID        string         `json:"threadId"`
	BindingID       string         `json:"bindingId,omitempty"`
	WorkspacePath   string         `json:"workspacePath,omitempty"`
	Kind            string         `json:"kind"`
	Text            string         `json:"text"`
	Attachments     []AttachmentV2 `json:"attachments,omitempty"`
	ReplyTo         string         `json:"replyTo"`
	Direction       string         `json:"direction"`
	Status          string         `json:"status"`
	LastError       string         `json:"lastError,omitempty"`
	CreatedAt       int64          `json:"createdAt"`
	UpdatedAt       int64          `json:"updatedAt"`
}

type Route struct {
	RouteKey        string `json:"routeKey"`
	Channel         string `json:"channel"`
	AccountID       string `json:"accountId"`
	SenderID        string `json:"senderId"`
	ConversationID  string `json:"conversationId"`
	ThreadID        string `json:"threadId"`
	BindingID       string `json:"bindingId,omitempty"`
	WorkspacePath   string `json:"workspacePath,omitempty"`
	NativeFrameID   string `json:"nativeFrameId,omitempty"`
	PairedAt        int64  `json:"pairedAt"`
	LastMessageAt   int64  `json:"lastMessageAt,omitempty"`
	PendingMessages int64  `json:"pendingMessages"`
}

type Counts struct {
	Authorized         int64 `json:"authorized"`
	Queued             int64 `json:"queued"`
	Claimed            int64 `json:"claimed"`
	Replied            int64 `json:"replied"`
	NeedsLocalApproval int64 `json:"needsLocalApproval"`
	Failed             int64 `json:"failed"`
}

type DeliveryAttempt struct {
	AttemptID         string `json:"attemptId"`
	MessageID         string `json:"messageId"`
	Stage             string `json:"stage"`
	Status            string `json:"status"`
	ContentSHA256     string `json:"contentSha256,omitempty"`
	PlatformMessageID string `json:"platformMessageId,omitempty"`
	LeaseUntil        int64  `json:"leaseUntil,omitempty"`
	CreatedAt         int64  `json:"createdAt"`
	UpdatedAt         int64  `json:"updatedAt"`
}

type ChannelHealth struct {
	ID          string `json:"id"`
	Configured  bool   `json:"configured"`
	Running     bool   `json:"running"`
	Paired      bool   `json:"paired"`
	Detail      string `json:"detail"`
	LastError   string `json:"lastError,omitempty"`
	UpdatedAt   int64  `json:"updatedAt"`
	LastEventAt int64  `json:"lastEventAt,omitempty"`
}

type RuntimeStatus struct {
	SchemaVersion int             `json:"schemaVersion"`
	Running       bool            `json:"running"`
	PID           int             `json:"pid"`
	MCPReady      bool            `json:"mcpReady"`
	MCPURL        string          `json:"mcpUrl"`
	Capabilities  map[string]bool `json:"capabilities"`
	Counts        Counts          `json:"counts"`
	Channels      []ChannelHealth `json:"channels"`
	UpdatedAt     int64           `json:"updatedAt"`
	Error         string          `json:"error,omitempty"`
}

type ResearchEvent struct {
	ID        string         `json:"id"`
	Type      string         `json:"type"`
	MessageID string         `json:"messageId,omitempty"`
	BindingID string         `json:"bindingId,omitempty"`
	Channel   string         `json:"channel,omitempty"`
	Metadata  map[string]any `json:"metadata,omitempty"`
	CreatedAt int64          `json:"createdAt"`
}

func UnixMillis() int64 {
	return time.Now().UTC().UnixMilli()
}

func RouteKey(channel, accountID, senderID, conversationID, threadID string) string {
	if strings.TrimSpace(threadID) == "" {
		threadID = conversationID
	}
	value := strings.Join([]string{channel, accountID, senderID, conversationID, threadID}, "\x1f")
	digest := sha256.Sum256([]byte(value))
	return hex.EncodeToString(digest[:16])
}
