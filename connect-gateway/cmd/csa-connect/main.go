package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/channels"
	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/config"
	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/gateway"
	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/mcpserver"
	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/registration"
	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/store"
)

const version = "0.1.0"

func main() {
	log.SetFlags(log.LstdFlags | log.LUTC)
	if err := run(os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, sanitizeError(err))
		os.Exit(1)
	}
}

func run(args []string) error {
	if len(args) == 0 {
		return errors.New("usage: csa-connect <serve|start|stop|apply-config|status|pair-code|telegram-info|routes|bind|claim-message|mark-delivery|requeue-message|expire-message|history|attachment-read|clear-history|events|connector-info|install-skill|simulate-inbound|scan-outbox|feishu-register-begin|feishu-register-poll|version>")
	}
	command := args[0]
	if command == "version" || command == "--version" {
		return writeJSON(os.Stdout, map[string]any{"name": "csa-connect", "version": version})
	}
	flags := flag.NewFlagSet(command, flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	defaultConfig, err := config.DefaultPath()
	if err != nil {
		return err
	}
	configPath := flags.String("config", defaultConfig, "Connect config path")
	switch command {
	case "serve":
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return serve(*configPath)
	case "start":
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return startDaemon(*configPath)
	case "stop":
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return stopDaemon(*configPath)
	case "apply-config":
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return applyConfig(*configPath, os.Stdin)
	case "status":
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return status(*configPath)
	case "pair-code":
		channel := flags.String("channel", "", "feishu or telegram")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return pairCode(*configPath, *channel)
	case "telegram-info":
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return telegramInfo(*configPath)
	case "routes":
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return routes(*configPath)
	case "bind":
		routeKey := flags.String("route", "", "route key")
		workspace := flags.String("workspace", "", "absolute WSL workspace path")
		frameID := flags.String("frame", "", "optional native frame ID")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return bind(*configPath, *routeKey, *workspace, *frameID)
	case "claim-message":
		messageID := flags.String("message", "", "queued message ID")
		workspace := flags.String("workspace", "", "absolute WSL workspace path")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return claimMessage(*configPath, *messageID, *workspace)
	case "requeue-message":
		messageID := flags.String("message", "", "claimed message ID")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return requeueMessage(*configPath, *messageID)
	case "mark-delivery":
		messageID := flags.String("message", "", "claimed message ID")
		attemptID := flags.String("attempt", "", "stable delivery attempt ID")
		status := flags.String("status", "", "submitted or delivery_unknown")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return markDelivery(*configPath, *messageID, *attemptID, *status)
	case "expire-message":
		messageID := flags.String("message", "", "queued or claimed message ID")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return expireMessage(*configPath, *messageID)
	case "history":
		offset := flags.Int("offset", 0, "history offset")
		limit := flags.Int("limit", 50, "history limit")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return history(*configPath, *offset, *limit)
	case "attachment-read":
		attachmentID := flags.String("attachment", "", "attachment ID")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return attachmentRead(*configPath, *attachmentID)
	case "clear-history":
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return clearHistory(*configPath)
	case "events":
		limit := flags.Int("limit", 100, "event limit")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return events(*configPath, *limit)
	case "connector-info":
		reveal := flags.Bool("reveal-token", false, "include the local Bearer token")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return connectorInfo(*configPath, *reveal)
	case "install-skill":
		source := flags.String("source", "", "source skill directory")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return installSkill(*source)
	case "simulate-inbound":
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return simulateInbound(*configPath, os.Stdin)
	case "scan-outbox":
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		return scanOutbox(*configPath)
	case "feishu-register-begin":
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		result, err := registration.BeginFeishu(context.Background(), registration.DefaultFeishuBaseURL)
		if err != nil {
			return err
		}
		return writeJSON(os.Stdout, result)
	case "feishu-register-poll":
		deviceCode := flags.String("device-code", "", "Feishu device registration code")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		code := strings.TrimSpace(*deviceCode)
		if code == "" {
			data, readErr := io.ReadAll(io.LimitReader(os.Stdin, 4<<10))
			if readErr != nil {
				return errors.New("read Feishu registration device code failed")
			}
			code = strings.TrimSpace(string(data))
		}
		result, err := registration.PollFeishu(context.Background(), registration.DefaultFeishuBaseURL, code)
		if err != nil {
			return err
		}
		return writeJSON(os.Stdout, result)
	default:
		return fmt.Errorf("unknown command %q", command)
	}
}

func telegramInfo(configPath string) error {
	cfg, err := config.Load(configPath)
	if err != nil {
		return err
	}
	if !cfg.Channels.Telegram.Enabled || strings.TrimSpace(cfg.Channels.Telegram.BotToken) == "" {
		return errors.New("Telegram is not configured")
	}
	username, err := channels.NewTelegram(cfg.Channels.Telegram.BotToken, nil).BotUsername(context.Background())
	if err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"username": username})
}

func claimMessage(configPath, messageID, workspace string) error {
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	message, err := gateway.New(dataStore).Claim(context.Background(), strings.TrimSpace(messageID), strings.TrimSpace(workspace))
	if err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"message": message})
}

func requeueMessage(configPath, messageID string) error {
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	if err := gateway.New(dataStore).Requeue(context.Background(), strings.TrimSpace(messageID)); err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"requeued": true, "messageId": messageID})
}

func markDelivery(configPath, messageID, attemptID, status string) error {
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	if err := gateway.New(dataStore).MarkDelivery(context.Background(), strings.TrimSpace(messageID), strings.TrimSpace(attemptID), strings.TrimSpace(status)); err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"updated": true, "messageId": messageID, "status": status})
}

func expireMessage(configPath, messageID string) error {
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	if err := gateway.New(dataStore).Expire(context.Background(), strings.TrimSpace(messageID)); err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"expired": true, "messageId": messageID})
}

func applyConfig(path string, input io.Reader) error {
	data, err := io.ReadAll(io.LimitReader(input, 64<<10))
	if err != nil {
		return err
	}
	var cfg config.Config
	if err := json.Unmarshal(data, &cfg); err != nil {
		return errors.New("Connect configuration JSON is invalid")
	}
	if cfg.MCPToken == "" {
		if current, err := config.Load(path); err == nil {
			cfg.MCPToken = current.MCPToken
		}
	}
	if err := config.Save(path, cfg); err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"ok": true, "restartRequired": true})
}

func openStore(configPath string) (*store.Store, error) {
	return store.Open(filepath.Join(filepath.Dir(configPath), "connect.db"))
}

func pairCode(configPath, channel string) error {
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	code, expiresAt, err := dataStore.CreatePairingCode(context.Background(), channel, 10*time.Minute)
	if err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"channel": channel, "code": code, "expiresAt": expiresAt})
}

func routes(configPath string) error {
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	values, err := dataStore.ListRoutes(context.Background())
	if err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"routes": values})
}

func bind(configPath, routeKey, workspace, frameID string) error {
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	connectGateway := gateway.New(dataStore)
	route, err := connectGateway.BindRoute(context.Background(), strings.TrimSpace(routeKey), strings.TrimSpace(workspace), strings.TrimSpace(frameID))
	if err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"route": route})
}

func history(configPath string, offset, limit int) error {
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	values, err := dataStore.History(context.Background(), offset, limit)
	if err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"messages": values})
}

func attachmentRead(configPath, attachmentID string) error {
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	attachment, path, err := dataStore.AttachmentPath(context.Background(), strings.TrimSpace(attachmentID))
	if err != nil {
		return errors.New("attachment is unavailable")
	}
	if attachment.State != "available" || attachment.SizeBytes <= 0 || attachment.SizeBytes > 20*1024*1024 {
		return errors.New("attachment is not available for delivery")
	}
	file, err := os.Open(path)
	if err != nil {
		return errors.New("attachment content is unavailable")
	}
	defer file.Close()
	written, err := io.CopyN(os.Stdout, file, attachment.SizeBytes)
	if errors.Is(err, io.EOF) && written == attachment.SizeBytes {
		err = nil
	}
	if err != nil || written != attachment.SizeBytes {
		return errors.New("attachment content length does not match metadata")
	}
	return nil
}

func clearHistory(configPath string) error {
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	deleted, err := dataStore.ClearHistory(context.Background())
	if err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"deleted": deleted})
}

func events(configPath string, limit int) error {
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	values, err := dataStore.ResearchEvents(context.Background(), limit)
	if err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"events": values})
}

func connectorInfo(configPath string, reveal bool) error {
	cfg, err := config.Load(configPath)
	if err != nil {
		return err
	}
	value := map[string]any{
		"name":          "CSA Connect",
		"url":           "http://127.0.0.1:9881/mcp",
		"transport":     "streamable-http",
		"authorization": "Bearer token required",
	}
	if reveal {
		value["bearerToken"] = cfg.MCPToken
	}
	return writeJSON(os.Stdout, value)
}

func status(configPath string) error {
	dataDir := filepath.Dir(configPath)
	runtimePath := filepath.Join(dataDir, "runtime.json")
	var current model.RuntimeStatus
	if data, err := os.ReadFile(runtimePath); err == nil {
		_ = json.Unmarshal(data, &current)
	}
	if current.PID <= 0 || !processExists(current.PID) || model.UnixMillis()-current.UpdatedAt > 15_000 {
		current.Running = false
		current.MCPReady = false
	}
	dataStore, err := openStore(configPath)
	if err == nil {
		current.Counts, _ = dataStore.Counts(context.Background())
		_ = dataStore.Close()
	}
	if current.SchemaVersion == 0 {
		current.SchemaVersion = 1
		current.MCPURL = "http://127.0.0.1:9881/mcp"
		current.Capabilities = map[string]bool{"mcpQueue": true, "workspaceFiles": true, "nativeInject": false}
	}
	return writeJSON(os.Stdout, current)
}

func serve(configPath string) error {
	cfg, err := config.Load(configPath)
	if err != nil {
		return err
	}
	dataDir := filepath.Dir(configPath)
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	connectGateway := gateway.New(dataStore)
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	var activeChannels []channels.Channel
	if cfg.Channels.Feishu.Enabled {
		activeChannels = append(activeChannels, channels.NewFeishu(cfg.Channels.Feishu.AppID, cfg.Channels.Feishu.AppSecret))
	}
	if cfg.Channels.Telegram.Enabled {
		activeChannels = append(activeChannels, channels.NewTelegram(cfg.Channels.Telegram.BotToken, dataStore))
	}
	for _, channel := range activeChannels {
		connectGateway.RegisterSender(channel.ID(), channel)
		go runChannel(ctx, channel, connectGateway.HandleInbound)
	}
	go connectGateway.RunMaintenance(ctx, cfg.RetentionDays)

	mux := http.NewServeMux()
	mux.Handle("/mcp", mcpserver.NewHandler(connectGateway, cfg.MCPToken))
	mux.HandleFunc("/health", func(response http.ResponseWriter, request *http.Request) {
		response.Header().Set("Content-Type", "application/json")
		response.Header().Set("Cache-Control", "no-store")
		_ = json.NewEncoder(response).Encode(map[string]any{"status": "ok", "service": "csa-connect", "version": version})
	})
	server := &http.Server{
		Addr:              cfg.ListenAddress,
		Handler:           mux,
		ReadHeaderTimeout: 5 * time.Second,
		IdleTimeout:       2 * time.Minute,
	}
	serverErrors := make(chan error, 1)
	go func() {
		if err := server.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			serverErrors <- err
		}
	}()
	statusWriter := &runtimeWriter{
		path:     filepath.Join(dataDir, "runtime.json"),
		gateway:  connectGateway,
		channels: activeChannels,
	}
	go statusWriter.run(ctx)
	_ = os.WriteFile(filepath.Join(dataDir, "gateway.pid"), []byte(strconv.Itoa(os.Getpid())+"\n"), 0o600)
	defer os.Remove(filepath.Join(dataDir, "gateway.pid"))

	select {
	case <-ctx.Done():
	case err := <-serverErrors:
		return err
	}
	shutdownContext, shutdownCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer shutdownCancel()
	return server.Shutdown(shutdownContext)
}

func runChannel(ctx context.Context, channel channels.Channel, handler channels.Handler) {
	backoff := time.Second
	for ctx.Err() == nil {
		if err := channel.Run(ctx, handler); err == nil || ctx.Err() != nil {
			return
		}
		timer := time.NewTimer(backoff)
		select {
		case <-ctx.Done():
			timer.Stop()
			return
		case <-timer.C:
		}
		if backoff < 30*time.Second {
			backoff *= 2
		}
	}
}

type runtimeWriter struct {
	path     string
	gateway  *gateway.Gateway
	channels []channels.Channel
	mu       sync.Mutex
}

func (writer *runtimeWriter) run(ctx context.Context) {
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		writer.write(ctx)
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}

func (writer *runtimeWriter) write(ctx context.Context) {
	writer.mu.Lock()
	defer writer.mu.Unlock()
	counts, _ := writer.gateway.Store().Counts(ctx)
	health := make([]model.ChannelHealth, 0, len(writer.channels))
	for _, channel := range writer.channels {
		value := channel.Health(ctx)
		value.Paired, _ = writer.gateway.Store().ChannelPaired(ctx, channel.ID())
		health = append(health, value)
	}
	value := model.RuntimeStatus{
		SchemaVersion: 1,
		Running:       true,
		PID:           os.Getpid(),
		MCPReady:      true,
		MCPURL:        "http://127.0.0.1:9881/mcp",
		Capabilities:  map[string]bool{"mcpQueue": true, "workspaceFiles": true, "nativeInject": false},
		Counts:        counts,
		Channels:      health,
		UpdatedAt:     model.UnixMillis(),
	}
	_ = writeJSONAtomic(writer.path, value)
}

type captureSender struct {
	values []model.OutboundMessage
}

func (sender *captureSender) Send(ctx context.Context, value model.OutboundMessage) error {
	sender.values = append(sender.values, value)
	return nil
}

func simulateInbound(configPath string, input io.Reader) error {
	data, err := io.ReadAll(io.LimitReader(input, 64<<10))
	if err != nil {
		return err
	}
	var inbound model.InboundMessage
	if err := json.Unmarshal(data, &inbound); err != nil {
		return errors.New("simulated inbound JSON is invalid")
	}
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	connectGateway := gateway.New(dataStore)
	sender := &captureSender{}
	connectGateway.RegisterSender(inbound.Channel, sender)
	err = connectGateway.HandleInbound(context.Background(), inbound)
	return writeJSON(os.Stdout, map[string]any{"ok": err == nil, "error": errorString(err), "outbound": sender.values})
}

func scanOutbox(configPath string) error {
	dataStore, err := openStore(configPath)
	if err != nil {
		return err
	}
	defer dataStore.Close()
	connectGateway := gateway.New(dataStore)
	connectGateway.RegisterSender("telegram", &captureSender{})
	if err := connectGateway.ScanOutboxesForSender(context.Background(), "csa-local-user"); err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"ok": true})
}

func installSkill(source string) error {
	if strings.TrimSpace(source) == "" {
		return errors.New("skill source path is required")
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	destination := filepath.Join(home, ".claude-science", "skills", "csa-connect")
	if err := copySkillDirectory(filepath.Clean(source), destination); err != nil {
		return err
	}
	return writeJSON(os.Stdout, map[string]any{"installed": true, "path": destination})
}

func copySkillDirectory(source, destination string) error {
	info, err := os.Stat(source)
	if err != nil || !info.IsDir() {
		return errors.New("skill source directory does not exist")
	}
	if _, err := os.Stat(filepath.Join(source, "SKILL.md")); err != nil {
		return errors.New("skill source does not contain SKILL.md")
	}
	if err := os.MkdirAll(destination, 0o700); err != nil {
		return err
	}
	return filepath.Walk(source, func(path string, info os.FileInfo, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		relative, err := filepath.Rel(source, path)
		if err != nil || strings.HasPrefix(relative, "..") {
			return errors.New("invalid skill source path")
		}
		target := filepath.Join(destination, relative)
		if info.Mode()&os.ModeSymlink != 0 {
			return errors.New("skill source may not contain symbolic links")
		}
		if info.IsDir() {
			return os.MkdirAll(target, 0o700)
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		return os.WriteFile(target, data, 0o600)
	})
}

func processExists(pid int) bool {
	if pid <= 0 {
		return false
	}
	_, err := os.Stat(filepath.Join("/proc", strconv.Itoa(pid)))
	return err == nil
}

func writeJSONAtomic(path string, value any) error {
	data, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	temporary := path + ".tmp"
	if err := os.WriteFile(temporary, append(data, '\n'), 0o600); err != nil {
		return err
	}
	return os.Rename(temporary, path)
}

func writeJSON(writer io.Writer, value any) error {
	encoder := json.NewEncoder(writer)
	encoder.SetEscapeHTML(false)
	return encoder.Encode(value)
}

func sanitizeError(err error) string {
	if err == nil {
		return ""
	}
	value := strings.ReplaceAll(strings.ReplaceAll(err.Error(), "\r", " "), "\n", " ")
	if len(value) > 500 {
		value = value[:500]
	}
	return value
}

func errorString(err error) string {
	if err == nil {
		return ""
	}
	return sanitizeError(err)
}
