//go:build linux

package main

import (
	"errors"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/config"
)

func startDaemon(configPath string) error {
	if _, err := config.Load(configPath); err != nil {
		return err
	}
	dataDir := filepath.Dir(configPath)
	pidPath := filepath.Join(dataDir, "gateway.pid")
	executable, err := os.Executable()
	if err != nil {
		return err
	}
	executable, err = filepath.EvalSymlinks(executable)
	if err != nil {
		return err
	}
	if pid, ok := ownedRunningPID(pidPath, executable); ok {
		return writeJSON(os.Stdout, map[string]any{"started": false, "running": true, "pid": pid})
	}
	if healthOK() {
		return errors.New("port 9881 is already used by an unmanaged process")
	}
	if err := os.MkdirAll(dataDir, 0o700); err != nil {
		return err
	}
	logFile, err := os.OpenFile(filepath.Join(dataDir, "gateway.log"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	defer logFile.Close()
	command := exec.Command(executable, "serve", "--config", configPath)
	command.Stdin = nil
	command.Stdout = logFile
	command.Stderr = logFile
	command.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := command.Start(); err != nil {
		return fmt.Errorf("start Connect Gateway: %w", err)
	}
	pid := command.Process.Pid
	_ = command.Process.Release()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if healthOK() {
			return writeJSON(os.Stdout, map[string]any{"started": true, "running": true, "pid": pid})
		}
		if err := syscall.Kill(pid, 0); err != nil {
			return errors.New("Connect Gateway exited before becoming healthy")
		}
		time.Sleep(250 * time.Millisecond)
	}
	_ = syscall.Kill(pid, syscall.SIGTERM)
	return errors.New("Connect Gateway did not become healthy within 10 seconds")
}

func stopDaemon(configPath string) error {
	dataDir := filepath.Dir(configPath)
	pidPath := filepath.Join(dataDir, "gateway.pid")
	executable, err := os.Executable()
	if err != nil {
		return err
	}
	executable, _ = filepath.EvalSymlinks(executable)
	pid, ok := ownedRunningPID(pidPath, executable)
	if !ok {
		_ = os.Remove(pidPath)
		return writeJSON(os.Stdout, map[string]any{"stopped": false, "running": false})
	}
	if err := syscall.Kill(pid, syscall.SIGTERM); err != nil && !errors.Is(err, syscall.ESRCH) {
		return err
	}
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if syscall.Kill(pid, 0) != nil {
			break
		}
		time.Sleep(150 * time.Millisecond)
	}
	_ = os.Remove(pidPath)
	return writeJSON(os.Stdout, map[string]any{"stopped": true, "running": false})
}

func ownedRunningPID(pidPath, executable string) (int, bool) {
	data, err := os.ReadFile(pidPath)
	if err != nil {
		return 0, false
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(data)))
	if err != nil || pid <= 0 || syscall.Kill(pid, 0) != nil {
		return 0, false
	}
	actual, err := filepath.EvalSymlinks(filepath.Join("/proc", strconv.Itoa(pid), "exe"))
	if err != nil || actual != executable {
		return 0, false
	}
	return pid, true
}

func healthOK() bool {
	client := &http.Client{Timeout: 800 * time.Millisecond}
	response, err := client.Get("http://127.0.0.1:9881/health")
	if err != nil {
		return false
	}
	defer response.Body.Close()
	return response.StatusCode == http.StatusOK
}
