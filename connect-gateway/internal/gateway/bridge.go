package gateway

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
)

const localGitExcludeEntry = "/.csa/connect/"

func bridgeRoot(workspacePath string) string {
	return filepath.Join(filepath.Clean(workspacePath), ".csa", "connect", "v1")
}

func ensureWorkspaceBridge(workspacePath string) error {
	root := bridgeRoot(workspacePath)
	for _, name := range []string{"inbox", "outbox", "ack"} {
		if err := os.MkdirAll(filepath.Join(root, name), 0o700); err != nil {
			return fmt.Errorf("create workspace Connect %s: %w", name, err)
		}
	}
	gitDir := filepath.Join(filepath.Clean(workspacePath), ".git")
	if info, err := os.Stat(gitDir); err == nil && info.IsDir() {
		infoDir := filepath.Join(gitDir, "info")
		if err := os.MkdirAll(infoDir, 0o700); err != nil {
			return err
		}
		excludePath := filepath.Join(infoDir, "exclude")
		content, _ := os.ReadFile(excludePath)
		if !containsLine(string(content), localGitExcludeEntry) {
			prefix := ""
			if len(content) > 0 && content[len(content)-1] != '\n' {
				prefix = "\n"
			}
			file, err := os.OpenFile(excludePath, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
			if err != nil {
				return err
			}
			_, writeErr := file.WriteString(prefix + localGitExcludeEntry + "\n")
			closeErr := file.Close()
			if writeErr != nil {
				return writeErr
			}
			if closeErr != nil {
				return closeErr
			}
		}
	}
	return nil
}

func containsLine(content, expected string) bool {
	for _, line := range strings.Split(content, "\n") {
		if strings.TrimSpace(line) == expected {
			return true
		}
	}
	return false
}

func writeJSONAtomic(path string, value any) error {
	data, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		return err
	}
	temporary := path + ".tmp"
	if err := os.WriteFile(temporary, append(data, '\n'), 0o600); err != nil {
		return err
	}
	if err := os.Rename(temporary, path); err != nil {
		_ = os.Remove(temporary)
		return err
	}
	return nil
}

func materializeMessage(message model.StoredMessage) error {
	if message.WorkspacePath == "" || message.BindingID == "" {
		return errors.New("message has no workspace binding")
	}
	if err := ensureWorkspaceBridge(message.WorkspacePath); err != nil {
		return err
	}
	envelope := model.ConnectEnvelopeV1{
		SchemaVersion:   model.SchemaVersion,
		MessageID:       message.ID,
		Channel:         message.Channel,
		PlatformEventID: message.PlatformEventID,
		SenderID:        message.SenderID,
		ConversationID:  message.ConversationID,
		ThreadID:        message.ThreadID,
		BindingID:       message.BindingID,
		Kind:            message.Kind,
		Text:            message.Text,
		Attachments:     message.Attachments,
		ReplyTo:         message.ReplyTo,
		CreatedAt:       time.UnixMilli(message.CreatedAt).UTC().Format(time.RFC3339Nano),
	}
	return writeJSONAtomic(filepath.Join(bridgeRoot(message.WorkspacePath), "inbox", message.ID+".json"), envelope)
}

func writeClaimAck(message model.StoredMessage) error {
	return writeDeliveryAck(message, model.StatusClaimed)
}

func writeDeliveryAck(message model.StoredMessage, status string) error {
	if message.WorkspacePath == "" {
		return nil
	}
	ack := map[string]any{
		"schemaVersion": model.SchemaVersion,
		"messageId":     message.ID,
		"status":        status,
		"updatedAt":     time.Now().UTC().Format(time.RFC3339Nano),
	}
	return writeJSONAtomic(filepath.Join(bridgeRoot(message.WorkspacePath), "ack", message.ID+".json"), ack)
}

func finishWorkspaceMessage(message model.StoredMessage, status string) {
	if message.WorkspacePath == "" {
		return
	}
	_ = os.Remove(filepath.Join(bridgeRoot(message.WorkspacePath), "inbox", message.ID+".json"))
	ack := map[string]any{
		"schemaVersion": model.SchemaVersion,
		"messageId":     message.ID,
		"status":        status,
		"updatedAt":     time.Now().UTC().Format(time.RFC3339Nano),
	}
	_ = writeJSONAtomic(filepath.Join(bridgeRoot(message.WorkspacePath), "ack", message.ID+".json"), ack)
}

func readOutbox(workspacePath string) ([]outboxItem, error) {
	directory := filepath.Join(bridgeRoot(workspacePath), "outbox")
	entries, err := os.ReadDir(directory)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	var values []outboxItem
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(strings.ToLower(entry.Name()), ".json") {
			continue
		}
		path := filepath.Join(directory, entry.Name())
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		var reply model.SandboxReplyV1
		if err := json.Unmarshal(data, &reply); err != nil || reply.SchemaVersion != model.SchemaVersion || reply.MessageID == "" {
			continue
		}
		values = append(values, outboxItem{Path: path, Reply: reply})
	}
	sort.SliceStable(values, func(i, j int) bool {
		if values[i].Reply.MessageID != values[j].Reply.MessageID {
			return values[i].Reply.MessageID < values[j].Reply.MessageID
		}
		if values[i].Reply.Sequence != values[j].Reply.Sequence {
			return values[i].Reply.Sequence < values[j].Reply.Sequence
		}
		return !values[i].Reply.Final && values[j].Reply.Final
	})
	return values, nil
}

type outboxItem struct {
	Path  string
	Reply model.SandboxReplyV1
}
