package config

import (
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const DefaultListenAddress = "127.0.0.1:9881"

type FeishuConfig struct {
	Enabled   bool   `json:"enabled"`
	AppID     string `json:"appId"`
	AppSecret string `json:"appSecret"`
}

type TelegramConfig struct {
	Enabled  bool   `json:"enabled"`
	BotToken string `json:"botToken"`
}

type Channels struct {
	Feishu   FeishuConfig   `json:"feishu"`
	Telegram TelegramConfig `json:"telegram"`
}

type Config struct {
	SchemaVersion int      `json:"schemaVersion"`
	ListenAddress string   `json:"listenAddress"`
	MCPToken      string   `json:"mcpToken"`
	RetentionDays int      `json:"retentionDays"`
	Channels      Channels `json:"channels"`
}

func DefaultDataDir() (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".local", "share", "claude-science-api-bridge", "connect"), nil
}

func DefaultPath() (string, error) {
	dir, err := DefaultDataDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "config.json"), nil
}

func New() (Config, error) {
	token, err := GenerateToken()
	if err != nil {
		return Config{}, err
	}
	return Config{
		SchemaVersion: 1,
		ListenAddress: DefaultListenAddress,
		MCPToken:      token,
		RetentionDays: 30,
	}, nil
}

func GenerateToken() (string, error) {
	value := make([]byte, 32)
	if _, err := rand.Read(value); err != nil {
		return "", fmt.Errorf("generate MCP token: %w", err)
	}
	return base64.RawURLEncoding.EncodeToString(value), nil
}

func Load(path string) (Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return Config{}, err
	}
	var cfg Config
	if err := json.Unmarshal(data, &cfg); err != nil {
		return Config{}, fmt.Errorf("parse connect config: %w", err)
	}
	if err := cfg.Validate(); err != nil {
		return Config{}, err
	}
	return cfg, nil
}

func Save(path string, cfg Config) error {
	if cfg.SchemaVersion == 0 {
		cfg.SchemaVersion = 1
	}
	if cfg.ListenAddress == "" {
		cfg.ListenAddress = DefaultListenAddress
	}
	if cfg.RetentionDays == 0 {
		cfg.RetentionDays = 30
	}
	if cfg.MCPToken == "" {
		token, err := GenerateToken()
		if err != nil {
			return err
		}
		cfg.MCPToken = token
	}
	if err := cfg.Validate(); err != nil {
		return err
	}
	data, err := json.MarshalIndent(cfg, "", "  ")
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
	if err := os.Chmod(temporary, 0o600); err != nil {
		_ = os.Remove(temporary)
		return err
	}
	if err := os.Rename(temporary, path); err != nil {
		_ = os.Remove(temporary)
		return err
	}
	return os.Chmod(path, 0o600)
}

func (cfg Config) Validate() error {
	if cfg.SchemaVersion != 1 {
		return fmt.Errorf("unsupported config schema version %d", cfg.SchemaVersion)
	}
	if cfg.ListenAddress != DefaultListenAddress {
		return errors.New("Connect Gateway must listen on 127.0.0.1:9881")
	}
	if len(cfg.MCPToken) < 32 || strings.ContainsAny(cfg.MCPToken, "\r\n\x00") {
		return errors.New("MCP token is missing or invalid")
	}
	if cfg.RetentionDays < 1 || cfg.RetentionDays > 3650 {
		return errors.New("retentionDays must be between 1 and 3650")
	}
	if cfg.Channels.Feishu.Enabled {
		if !strings.HasPrefix(cfg.Channels.Feishu.AppID, "cli_") || len(cfg.Channels.Feishu.AppSecret) < 16 {
			return errors.New("Feishu App ID or App Secret is invalid")
		}
	}
	if cfg.Channels.Telegram.Enabled {
		parts := strings.SplitN(cfg.Channels.Telegram.BotToken, ":", 2)
		if len(parts) != 2 || len(parts[0]) < 5 || len(parts[1]) < 10 {
			return errors.New("Telegram Bot Token is invalid")
		}
	}
	return nil
}
