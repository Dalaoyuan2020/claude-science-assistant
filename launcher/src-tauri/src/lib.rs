use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Manager;

mod connect;

#[cfg(windows)]
use std::os::windows::{ffi::OsStrExt, process::CommandExt};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x00000010;

// Reasoning models such as GLM-5.2 can spend the first dozens of tokens in
// reasoning_content. A tiny cap returns HTTP 200 with an empty final answer and
// makes a valid Key look broken.
const API_KEY_TEST_INITIAL_MAX_TOKENS: u32 = 256;
const API_KEY_TEST_RETRY_MAX_TOKENS: u32 = 1024;

#[cfg(windows)]
#[link(name = "Kernel32")]
extern "system" {
    fn MoveFileExW(existing_file_name: *const u16, new_file_name: *const u16, flags: u32) -> i32;
    fn GetLocalTime(system_time: *mut WindowsSystemTime);
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct WindowsSystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[cfg(windows)]
const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
#[cfg(windows)]
const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

fn background_command(program: &str) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn command_output_with_timeout(
    mut command: Command,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("{label}启动失败：{error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{label}无法读取标准输出"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{label}无法读取错误输出"))?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{label}在 {} 秒内没有响应，已停止本次操作。请检查宿主磁盘空间、WSL VHDX 与发行版状态。",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{label}状态读取失败：{error}"));
            }
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn command_output_with_input_timeout(
    mut command: Command,
    input: &[u8],
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("{label}启动失败：{error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("{label}无法写入标准输入"))?;
    stdin
        .write_all(input)
        .map_err(|error| format!("{label}写入输入失败：{error}"))?;
    drop(stdin);
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{label}无法读取标准输出"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{label}无法读取错误输出"))?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{label}在 {} 秒内没有响应，已停止",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{label}状态读取失败：{error}"));
            }
        }
    };
    Ok(Output {
        status,
        stdout: stdout_reader.join().unwrap_or_default(),
        stderr: stderr_reader.join().unwrap_or_default(),
    })
}

async fn run_blocking<T, F>(job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| format!("background task failed: {error}"))?
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemStatus {
    state: String,
    wsl_installed: bool,
    distro: Option<String>,
    linux_user: Option<String>,
    bridge_running: bool,
    bridge_pid: Option<u32>,
    claude_running: bool,
    claude_pid: Option<u32>,
    bridge_healthy: bool,
    windows_bridge_pid: Option<u32>,
    runtime_ready: bool,
    source_binary_present: bool,
    bridge_venv_present: bool,
    wsl_storage_path: Option<String>,
    wsl_storage_drive: Option<String>,
    wsl_storage_free_gb: Option<f64>,
    wsl_vhdx_size_gb: Option<f64>,
    wsl_root_free_gb: Option<f64>,
    settings_storage_drive: Option<String>,
    settings_storage_free_gb: Option<f64>,
    storage_warning: bool,
    storage_blocked: bool,
    restart_blocked: bool,
    warnings: Vec<String>,
}

#[derive(Debug, Default)]
struct WindowsStorageSnapshot {
    wsl_base_path: Option<String>,
    wsl_drive: Option<String>,
    wsl_drive_free_bytes: Option<u64>,
    vhdx_size_bytes: Option<u64>,
    settings_drive: Option<String>,
    settings_drive_free_bytes: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct WslProbeReport {
    #[serde(default)]
    wsl: WslProbeIdentity,
    #[serde(default)]
    components: WslProbeComponents,
    #[serde(default)]
    storage: WslProbeStorage,
    #[serde(default)]
    runtime: WslProbeRuntime,
}

#[derive(Debug, Default, Deserialize)]
struct WslProbeIdentity {
    #[serde(default)]
    user: String,
}

#[derive(Debug, Default, Deserialize)]
struct WslProbeComponents {
    #[serde(default)]
    source_binary: bool,
    #[serde(default)]
    bridge_venv: bool,
    #[serde(default)]
    tmp_writable: bool,
    #[serde(default)]
    home_writable: bool,
}

#[derive(Debug, Default, Deserialize)]
struct WslProbeStorage {
    root_total_kb: Option<u64>,
    root_free_kb: Option<u64>,
    root_inode_total: Option<u64>,
    root_inode_free: Option<u64>,
    #[serde(default)]
    root_read_only: bool,
    bridge_log_bytes: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct WslProbeRuntime {
    bridge_pid: Option<u32>,
    claude_pid: Option<u32>,
    bridge_source_path: Option<String>,
    bridge_source_matches: Option<bool>,
    #[serde(default)]
    bridge_healthy: bool,
    #[serde(default)]
    bridge_health_responding: bool,
    #[serde(default)]
    bridge_service_active: bool,
    unit_matches_project: Option<bool>,
    #[serde(default)]
    port_9876: bool,
    #[serde(default)]
    port_8765: bool,
    #[serde(default)]
    port_8766: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderCatalogGroup {
    title: String,
    tier: String,
    providers: Vec<ProviderPreset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderPreset {
    id: String,
    name: String,
    meta: String,
    badge: String,
    trust: String,
    protocol: String,
    base_url: Option<String>,
    default_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LauncherSettings {
    selected_provider_id: String,
    custom_base_url: String,
    custom_confirmed: bool,
    #[serde(default)]
    active_api_key_id: Option<String>,
    #[serde(default)]
    api_keys: Vec<StoredApiKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredApiKey {
    id: String,
    provider_id: String,
    label: String,
    base_url: String,
    model: String,
    custom_confirmed: bool,
    #[serde(default)]
    model_aliases: Vec<StoredModelAlias>,
    encrypted_api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredModelAlias {
    id: String,
    display_name: String,
    model: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiKeySummary {
    id: String,
    provider_id: String,
    label: String,
    base_url: String,
    model: String,
    custom_confirmed: bool,
    model_aliases: Vec<StoredModelAlias>,
    has_secret: bool,
    active: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LauncherState {
    selected_provider_id: String,
    custom_base_url: String,
    custom_confirmed: bool,
    active_api_key_id: Option<String>,
    api_keys: Vec<ApiKeySummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BridgeConfigRollback {
    restore: serde_json::Map<String, serde_json::Value>,
    delete: Vec<String>,
}

#[derive(Debug, Clone)]
struct AppliedBridgeConfig {
    distro: String,
    rollback: BridgeConfigRollback,
    previous_status: SystemStatus,
}

#[derive(Debug, Clone)]
struct BridgeRuntimeProfile {
    provider_id: String,
    label: String,
    backend: &'static str,
    api_key_field: &'static str,
    base_url: String,
    upstream_mode: &'static str,
    default_model: String,
    default_fast_model: String,
    requires_explicit_model: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiKeyTestResult {
    ok: bool,
    provider_id: String,
    base_url: String,
    upstream_mode: String,
    selected_model: String,
    reply: String,
    models: Vec<String>,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiKeyAutoMapResult {
    ok: bool,
    provider_id: String,
    base_url: String,
    upstream_mode: String,
    primary_model: String,
    fast_model: String,
    aliases: Vec<StoredModelAlias>,
    models: Vec<String>,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExternalAgentRunResult {
    ok: bool,
    tool: String,
    exit_code: Option<i32>,
    duration_ms: u128,
    stdout: String,
    stderr: String,
    result_text: Option<String>,
    session_id: Option<String>,
    resume_command: Option<String>,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentRequest {
    #[serde(default)]
    schema_version: Option<u32>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    task_kind: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    requested_action: Option<String>,
    #[serde(default)]
    approval_mode: Option<String>,
    #[serde(default)]
    policy_id: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubagentInboxItem {
    request_id: String,
    file_name: String,
    file_path: String,
    modified_ms: u64,
    request: Option<SubagentRequest>,
    parse_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentRunResult {
    run_id: String,
    request_id: String,
    result_dir: String,
    result_json_path: String,
    agent: ExternalAgentRunResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentSessionReplyResult {
    run_id: String,
    request_id: Option<String>,
    parent_run_id: Option<String>,
    result_dir: String,
    result_json_path: String,
    agent: ExternalAgentRunResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentOutboxResult {
    schema_version: u32,
    request_id: String,
    status: String,
    latest_run_id: String,
    session_id: Option<String>,
    result_path: String,
    summary: String,
    next_action: String,
    updated_at: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubagentRunHistoryItem {
    run_id: String,
    kind: String,
    request_id: Option<String>,
    parent_run_id: Option<String>,
    result_dir: String,
    result_json_path: String,
    modified_ms: u64,
    agent: ExternalAgentRunResult,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExternalSessionLaunchResult {
    session_id: String,
    command: String,
    cwd: String,
    terminal: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeSessionMessage {
    id: String,
    session_id: String,
    role: String,
    kind: String,
    content: String,
    created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeSessionHistory {
    session_id: String,
    file_path: String,
    modified_ms: u64,
    messages: Vec<ClaudeSessionMessage>,
    total_messages: usize,
    has_more: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResearchOsState {
    #[serde(default)]
    repositories: Vec<SkillRepository>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillRepository {
    id: String,
    source: String,
    local_path: String,
    created_at: u64,
    last_synced_at: u64,
    last_commit: String,
    #[serde(default)]
    skills: Vec<SkillFeedItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillFeedItem {
    id: String,
    repository_id: String,
    name: String,
    description: String,
    relative_path: String,
    modified_ms: u64,
    fingerprint: String,
    is_new: bool,
}

#[derive(Debug, Default, Deserialize)]
struct SkillFrontMatter {
    name: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredConnectSettings {
    #[serde(default)]
    encrypted_feishu_webhook: String,
    #[serde(default)]
    encrypted_telegram_bot_token: String,
    #[serde(default)]
    telegram_chat_id: String,
    #[serde(default)]
    feishu_updated_at: u64,
    #[serde(default)]
    telegram_updated_at: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectChannelSummary {
    id: String,
    configured: bool,
    detail: String,
    updated_at: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectState {
    feishu: ConnectChannelSummary,
    telegram: ConnectChannelSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectTestResult {
    ok: bool,
    channel: String,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelListFetchResult {
    models: Vec<String>,
    message: String,
}

impl Default for LauncherSettings {
    fn default() -> Self {
        Self {
            selected_provider_id: "deepseek".into(),
            custom_base_url: String::new(),
            custom_confirmed: false,
            active_api_key_id: None,
            api_keys: Vec::new(),
        }
    }
}

fn decode_console_output(bytes: &[u8]) -> String {
    let pair_count = bytes.len() / 2;
    let odd_zero_count = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
    let even_zero_count = bytes.iter().step_by(2).filter(|b| **b == 0).count();
    if pair_count > 0 && odd_zero_count * 4 >= pair_count * 3 && even_zero_count * 10 <= pair_count
    {
        let words: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&words).replace('\0', "");
    }
    String::from_utf8_lossy(bytes).replace('\0', "")
}

fn is_wsl_localhost_proxy_warning(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("wsl:")
        && lower.contains("localhost")
        && (lower.contains("proxy")
            || lower.contains("nat")
            || lower.contains("代理")
            || lower.contains("镜像"))
}

fn clean_diagnostic_text(text: &str) -> String {
    let text = text.replace('\0', "");
    let filtered = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !is_wsl_localhost_proxy_warning(line))
        .collect::<Vec<_>>()
        .join("\n");
    if filtered.trim().is_empty() {
        text.trim().to_string()
    } else {
        filtered
    }
}

fn command_error_text(output: &Output) -> String {
    let stderr = clean_diagnostic_text(&decode_console_output(&output.stderr));
    if !stderr.trim().is_empty() {
        return stderr;
    }
    clean_diagnostic_text(&decode_console_output(&output.stdout))
}

fn output_text(output: &Output) -> String {
    decode_console_output(&output.stdout).trim().to_string()
}

fn discover_distros() -> Result<Vec<String>, String> {
    let mut command = background_command("wsl.exe");
    command.args(["--list", "--quiet"]);
    let output = command_output_with_timeout(command, Duration::from_secs(5), "WSL 发行版检查")?;
    if !output.status.success() {
        return Err("WSL 尚未安装或当前不可用".to_string());
    }
    let distros = output_text(&output)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.to_ascii_lowercase().starts_with("docker-desktop"))
        .map(ToOwned::to_owned)
        .collect();
    Ok(distros)
}

fn preferred_distro(distros: &[String]) -> Option<String> {
    distros
        .iter()
        .find(|name| name.eq_ignore_ascii_case("Ubuntu-24.04"))
        .or_else(|| {
            distros
                .iter()
                .find(|name| name.to_ascii_lowercase().starts_with("ubuntu"))
        })
        .or_else(|| distros.first())
        .cloned()
}

fn run_wsl(distro: &str, args: &[&str]) -> Result<Output, String> {
    run_wsl_with_timeout(distro, args, Duration::from_secs(8))
}

fn run_wsl_with_timeout(distro: &str, args: &[&str], timeout: Duration) -> Result<Output, String> {
    let mut command = background_command("wsl.exe");
    command
        .arg("--distribution")
        .arg(distro)
        .arg("--")
        .args(args);
    command_output_with_timeout(command, timeout, &format!("WSL {distro}"))
}

fn wsl_shell(distro: &str, script: &str) -> Result<Output, String> {
    run_wsl(distro, &["sh", "-lc", script])
}

fn parse_first_pid(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| line.trim().parse::<u32>().ok())
}

fn legacy_windows_bridge_pid() -> Option<u32> {
    let root = project_root().ok()?;
    let escaped_root = root.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$h=$null; try{{$h=Invoke-RestMethod -Uri 'http://127.0.0.1:9876/health' -TimeoutSec 1}}catch{{}}; \
         if($h -and $h.proxy_dir -eq '{}'){{ \
           $c=Get-NetTCPConnection -LocalPort 9876 -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1; \
           if($c){{$p=Get-CimInstance Win32_Process -Filter \"ProcessId=$($c.OwningProcess)\"; \
             if($p.CommandLine -match 'proxy\\.py'){{$c.OwningProcess}}}}}}",
        escaped_root
    );
    let mut command = background_command("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    command_output_with_timeout(command, Duration::from_secs(5), "旧版 Bridge 检查")
        .ok()
        .and_then(|output| parse_first_pid(&output_text(&output)))
}

fn windows_path_to_wsl(distro: &str, path: &Path) -> Option<String> {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let output = run_wsl(distro, &["wslpath", "-a", &normalized]).ok()?;
    if !output.status.success() {
        return None;
    }
    output_text(&output)
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with('/'))
        .map(ToOwned::to_owned)
}

fn windows_storage_snapshot(distro: &str) -> WindowsStorageSnapshot {
    let distro = distro.replace('\'', "''");
    let settings = settings_path()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_default()
        .to_string_lossy()
        .replace('\'', "''");
    let script = r#"
$distro = '__DISTRO__'
$settingsPath = '__SETTINGS_PATH__'
function Normalize-LocalPath([string]$Path) {
  if (-not $Path) { return '' }
  $expanded = [Environment]::ExpandEnvironmentVariables($Path)
  if ($expanded.StartsWith('\\?\')) { return $expanded.Substring(4) }
  return $expanded
}
function Get-DriveSnapshot([string]$Path) {
  $clean = Normalize-LocalPath $Path
  if (-not $clean) { return $null }
  $root = [IO.Path]::GetPathRoot($clean)
  if (-not $root) { return $null }
  try {
    $drive = [IO.DriveInfo]::new($root)
    return [ordered]@{
      name = $drive.Name.TrimEnd('\')
      free_bytes = [int64]$drive.AvailableFreeSpace
    }
  } catch { return $null }
}
$entry = Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss' -ErrorAction SilentlyContinue |
  ForEach-Object { Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue } |
  Where-Object { $_.DistributionName -eq $distro } |
  Select-Object -First 1
$base = if ($entry) { Normalize-LocalPath ([string]$entry.BasePath) } else { '' }
$wslDrive = Get-DriveSnapshot $base
$settingsDrive = Get-DriveSnapshot $settingsPath
$vhdx = if ($base) { Join-Path $base 'ext4.vhdx' } else { '' }
$vhdxItem = if ($vhdx -and (Test-Path -LiteralPath $vhdx)) { Get-Item -LiteralPath $vhdx -ErrorAction SilentlyContinue } else { $null }
[ordered]@{
  wsl_base_path = if ($base) { $base } else { $null }
  wsl_drive = if ($wslDrive) { $wslDrive.name } else { $null }
  wsl_drive_free_bytes = if ($wslDrive) { $wslDrive.free_bytes } else { $null }
  vhdx_size_bytes = if ($vhdxItem) { [int64]$vhdxItem.Length } else { $null }
  settings_drive = if ($settingsDrive) { $settingsDrive.name } else { $null }
  settings_drive_free_bytes = if ($settingsDrive) { $settingsDrive.free_bytes } else { $null }
} | ConvertTo-Json -Compress
"#
    .replace("__DISTRO__", &distro)
    .replace("__SETTINGS_PATH__", &settings);
    let mut command = background_command("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    let Ok(output) =
        command_output_with_timeout(command, Duration::from_secs(5), "WSL 存储位置检查")
    else {
        return WindowsStorageSnapshot::default();
    };
    let Ok(data) = serde_json::from_str::<serde_json::Value>(&output_text(&output)) else {
        return WindowsStorageSnapshot::default();
    };
    WindowsStorageSnapshot {
        wsl_base_path: data
            .get("wsl_base_path")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        wsl_drive: data
            .get("wsl_drive")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        wsl_drive_free_bytes: data
            .get("wsl_drive_free_bytes")
            .and_then(serde_json::Value::as_u64),
        vhdx_size_bytes: data
            .get("vhdx_size_bytes")
            .and_then(serde_json::Value::as_u64),
        settings_drive: data
            .get("settings_drive")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        settings_drive_free_bytes: data
            .get("settings_drive_free_bytes")
            .and_then(serde_json::Value::as_u64),
    }
}

fn rounded_gb_from_bytes(bytes: u64) -> f64 {
    ((bytes as f64 / 1024_f64.powi(3)) * 10.0).round() / 10.0
}

fn rounded_gb_from_kb(kb: u64) -> f64 {
    ((kb as f64 / 1024_f64.powi(2)) * 10.0).round() / 10.0
}

fn is_windows_system_drive(drive: Option<&str>) -> bool {
    drive
        .map(|value| {
            value
                .trim()
                .trim_end_matches('\\')
                .eq_ignore_ascii_case("C:")
        })
        .unwrap_or(false)
}

fn inspect_wsl_runtime(distro: &str, project_wsl: &str) -> Result<WslProbeReport, String> {
    let inspect_script = format!(
        "{}/skills/bootstrap-claude-science-wsl/scripts/inspect-wsl.sh",
        project_wsl.trim_end_matches('/')
    );
    let project_env = format!("PROJECT_DIR={project_wsl}");
    let output = run_wsl_with_timeout(
        distro,
        &[
            "env",
            &project_env,
            "PROXY_PORT=9876",
            "bash",
            &inspect_script,
            project_wsl,
        ],
        Duration::from_secs(8),
    )?;
    if !output.status.success() {
        return Err(format!("WSL 只读体检失败：{}", command_error_text(&output)));
    }
    let text = output_text(&output);
    let json = text
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .ok_or_else(|| "WSL 只读体检没有返回 JSON 结果".to_string())?;
    serde_json::from_str(json).map_err(|error| format!("WSL 体检结果解析失败：{error}"))
}

fn current_status() -> SystemStatus {
    let mut warnings = Vec::new();
    let distros = match discover_distros() {
        Ok(items) => items,
        Err(error) => {
            warnings.push(error);
            return SystemStatus {
                state: "notInstalled".into(),
                wsl_installed: false,
                distro: None,
                linux_user: None,
                bridge_running: false,
                bridge_pid: None,
                claude_running: false,
                claude_pid: None,
                bridge_healthy: false,
                windows_bridge_pid: legacy_windows_bridge_pid(),
                runtime_ready: false,
                source_binary_present: false,
                bridge_venv_present: false,
                wsl_storage_path: None,
                wsl_storage_drive: None,
                wsl_storage_free_gb: None,
                wsl_vhdx_size_gb: None,
                wsl_root_free_gb: None,
                settings_storage_drive: None,
                settings_storage_free_gb: None,
                storage_warning: false,
                storage_blocked: false,
                restart_blocked: false,
                warnings,
            };
        }
    };

    let Some(distro) = preferred_distro(&distros) else {
        warnings.push("No usable Linux distro was found.".into());
        return SystemStatus {
            state: "notInstalled".into(),
            wsl_installed: true,
            distro: None,
            linux_user: None,
            bridge_running: false,
            bridge_pid: None,
            claude_running: false,
            claude_pid: None,
            bridge_healthy: false,
            windows_bridge_pid: legacy_windows_bridge_pid(),
            runtime_ready: false,
            source_binary_present: false,
            bridge_venv_present: false,
            wsl_storage_path: None,
            wsl_storage_drive: None,
            wsl_storage_free_gb: None,
            wsl_vhdx_size_gb: None,
            wsl_root_free_gb: None,
            settings_storage_drive: None,
            settings_storage_free_gb: None,
            storage_warning: false,
            storage_blocked: false,
            restart_blocked: false,
            warnings,
        };
    };

    if !distro.eq_ignore_ascii_case("Ubuntu-24.04") {
        warnings.push(format!("推荐使用 Ubuntu-24.04；当前兼容使用 {}。", distro));
    }
    let windows_storage = windows_storage_snapshot(&distro);
    let wsl_storage_free_gb = windows_storage
        .wsl_drive_free_bytes
        .map(rounded_gb_from_bytes);
    let wsl_vhdx_size_gb = windows_storage.vhdx_size_bytes.map(rounded_gb_from_bytes);
    let settings_storage_free_gb = windows_storage
        .settings_drive_free_bytes
        .map(rounded_gb_from_bytes);
    let wsl_on_system_drive = is_windows_system_drive(windows_storage.wsl_drive.as_deref());
    let project_files_present = project_runtime_files_present();
    let project_wsl = project_root()
        .ok()
        .and_then(|root| windows_path_to_wsl(&distro, &root));
    let probe = project_wsl
        .as_deref()
        .ok_or_else(|| "无法把当前 CSA 目录转换为 WSL 路径".to_string())
        .and_then(|path| inspect_wsl_runtime(&distro, path));
    let probe = match probe {
        Ok(probe) => probe,
        Err(error) => {
            let storage_blocked = wsl_storage_free_gb.map(|free| free < 1.0).unwrap_or(false)
                || settings_storage_free_gb
                    .map(|free| free < 1.0)
                    .unwrap_or(false);
            warnings.push(format!(
                "{error}。启动器已停止后续探测：若 WSL 本身无响应，请检查宿主盘空间与 VHDX；若体检脚本缺失，请重新解压完整 Release ZIP。"
            ));
            return SystemStatus {
                state: "degraded".into(),
                wsl_installed: true,
                distro: Some(distro),
                linux_user: None,
                bridge_running: false,
                bridge_pid: None,
                claude_running: false,
                claude_pid: None,
                bridge_healthy: false,
                windows_bridge_pid: None,
                runtime_ready: false,
                source_binary_present: false,
                bridge_venv_present: false,
                wsl_storage_path: windows_storage.wsl_base_path,
                wsl_storage_drive: windows_storage.wsl_drive,
                wsl_storage_free_gb,
                wsl_vhdx_size_gb,
                wsl_root_free_gb: None,
                settings_storage_drive: windows_storage.settings_drive,
                settings_storage_free_gb,
                storage_warning: storage_blocked,
                storage_blocked,
                restart_blocked: true,
                warnings,
            };
        }
    };

    let linux_user = (!probe.wsl.user.trim().is_empty()).then_some(probe.wsl.user);
    let source_binary_present = probe.components.source_binary;
    let bridge_venv_present = probe.components.bridge_venv;
    let wsl_runtime_writable = probe.components.tmp_writable && probe.components.home_writable;
    let runtime_ready = source_binary_present
        && bridge_venv_present
        && project_files_present
        && wsl_runtime_writable
        && !probe.storage.root_read_only;
    let bridge_pid = probe.runtime.bridge_pid;
    let claude_pid = probe.runtime.claude_pid;
    let bridge_healthy = probe.runtime.bridge_healthy;
    let bridge_running = bridge_pid.is_some()
        || probe.runtime.bridge_health_responding
        || probe.runtime.bridge_service_active
        || probe.runtime.port_9876;
    let claude_running = claude_pid.is_some() || probe.runtime.port_8765 || probe.runtime.port_8766;
    let unit_matches_project = probe.runtime.unit_matches_project;
    let wsl_root_free_gb = probe.storage.root_free_kb.map(rounded_gb_from_kb);
    let root_free_ratio = probe
        .storage
        .root_total_kb
        .zip(probe.storage.root_free_kb)
        .filter(|(total, _)| *total > 0)
        .map(|(total, free)| free as f64 / total as f64);
    let inode_free_ratio = probe
        .storage
        .root_inode_total
        .zip(probe.storage.root_inode_free)
        .filter(|(total, _)| *total > 0)
        .map(|(total, free)| free as f64 / total as f64);
    let storage_blocked = probe.storage.root_read_only
        || !wsl_runtime_writable
        || wsl_storage_free_gb.map(|free| free < 1.0).unwrap_or(false)
        || wsl_root_free_gb.map(|free| free < 1.0).unwrap_or(false)
        || settings_storage_free_gb
            .map(|free| free < 1.0)
            .unwrap_or(false)
        || root_free_ratio.map(|ratio| ratio < 0.01).unwrap_or(false)
        || inode_free_ratio.map(|ratio| ratio < 0.01).unwrap_or(false);
    let storage_warning = storage_blocked
        || wsl_on_system_drive
        || wsl_storage_free_gb.map(|free| free < 15.0).unwrap_or(false)
        || wsl_root_free_gb.map(|free| free < 15.0).unwrap_or(false)
        || settings_storage_free_gb
            .map(|free| free < 10.0)
            .unwrap_or(false)
        || root_free_ratio.map(|ratio| ratio < 0.10).unwrap_or(false)
        || inode_free_ratio.map(|ratio| ratio < 0.05).unwrap_or(false);
    let restart_blocked = storage_blocked;
    let windows_bridge_pid = legacy_windows_bridge_pid();

    if bridge_running && !probe.runtime.bridge_health_responding {
        warnings.push("Bridge process/service/port exists, but health check failed.".into());
    }
    if probe.runtime.bridge_health_responding && !bridge_healthy {
        let expected = project_wsl
            .as_deref()
            .map(|root| format!("{}/proxy.py", root.trim_end_matches('/')))
            .unwrap_or_else(|| "unknown".into());
        let actual = probe
            .runtime
            .bridge_source_path
            .as_deref()
            .unwrap_or("unknown");
        let source_match = probe
            .runtime
            .bridge_source_matches
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".into());
        warnings.push(format!(
            "Port 9876 is answered by a Bridge from another or older CSA package directory; expected {expected}, actual {actual}, source_match={source_match}. Restart from this package to migrate it."
        ));
    }
    if bridge_running && !claude_running {
        warnings.push("Bridge is running, but Claude Science is not detected on 8765/8766.".into());
    }
    if unit_matches_project == Some(false) {
        warnings.push("WSL Bridge service points to a different package directory; run the repair flow to re-register the current CSA folder.".into());
    }

    if bridge_pid.is_some() && claude_pid.is_none() {
        warnings.push("Bridge 正在运行，但 Claude Science 尚未启动".into());
    }
    if bridge_pid.is_some() && windows_bridge_pid.is_some() {
        warnings.push("检测到 Windows 与 WSL 同时运行 Bridge；请迁移旧 Windows 实例".into());
    }
    if !source_binary_present {
        warnings.push(
            "尚未检测到 Claude Science Linux 二进制：请先运行 1-run-acceptance-preview.bat，确认后运行 4-install-runtime-after-preview.bat；完整便携包会内置锁定版本".into(),
        );
    }
    if !bridge_venv_present {
        warnings.push("尚未检测到 WSL Bridge 运行时 venv：请先运行 repair-approved.ps1 -PlanOnly，确认后再修复".into());
    }
    if !project_files_present {
        warnings.push(
            "启动器同目录缺少 proxy.py、requirements.txt 或 WSL 启动脚本；请从完整便携包根目录运行"
                .into(),
        );
    }

    if probe.storage.root_read_only {
        warnings.push(
            "WSL root filesystem is mounted read-only. Do not restart or repair CSA until the host disk and WSL VHDX are healthy.".into(),
        );
    }
    if wsl_on_system_drive {
        let location = windows_storage
            .wsl_base_path
            .as_deref()
            .unwrap_or("C: (exact WSL storage path unavailable)");
        warnings.push(format!(
            "WSL virtual disk is located on the Windows system drive ({location}). Large experiments can exhaust C:. CSA will not move the distro automatically; do not move ext4.vhdx manually. Generate and review a machine-specific migration plan before any WSL Move/export/import operation."
        ));
    }
    if let Some(free) = wsl_storage_free_gb.filter(|free| *free < 15.0) {
        let location = windows_storage
            .wsl_base_path
            .as_deref()
            .unwrap_or("unknown WSL storage path");
        warnings.push(format!(
            "WSL virtual disk host volume has only {free:.1} GB free ({location}). Free space before running large experiments."
        ));
    }
    if let Some(free) = wsl_root_free_gb.filter(|free| *free < 15.0) {
        warnings.push(format!(
            "WSL Linux root filesystem has only {free:.1} GB free. Move datasets/results to a suitable data volume or clean them before continuing."
        ));
    }
    if let Some(free) = settings_storage_free_gb.filter(|free| *free < 10.0) {
        let drive = windows_storage
            .settings_drive
            .as_deref()
            .unwrap_or("Windows settings drive");
        warnings.push(format!(
            "Windows settings drive {drive} has only {free:.1} GB free. API Key switching can fail if this drive becomes full."
        ));
    }
    if inode_free_ratio.map(|ratio| ratio < 0.05).unwrap_or(false) {
        warnings.push("WSL root filesystem is running low on free inodes; a large number of small experiment files can prevent new files even when GB remain.".into());
    }
    if probe
        .storage
        .bridge_log_bytes
        .map(|bytes| bytes > 50 * 1024 * 1024)
        .unwrap_or(false)
    {
        warnings.push("Bridge log exceeds 50 MB and will be rotated on the next CSA start.".into());
    }

    if !probe.components.tmp_writable {
        warnings.push(
            "WSL temporary directory is read-only or not writable. Claude Science needs /tmp to create runtime files. Run `wsl --shutdown`, reopen Ubuntu, then retry; if it remains read-only, repair or recreate the WSL distro.".into(),
        );
    }
    if !probe.components.home_writable {
        warnings.push(
            "WSL user home is read-only or not writable. CSA cannot update logs, runtime files, or Bridge configuration until the WSL distro is repaired.".into(),
        );
    }

    let state = if bridge_healthy && claude_running && unit_matches_project != Some(false) {
        "running"
    } else if storage_blocked || !wsl_runtime_writable {
        "degraded"
    } else if !runtime_ready && !bridge_running && !claude_running {
        "notInstalled"
    } else if bridge_running || claude_running {
        "degraded"
    } else {
        "stopped"
    };

    SystemStatus {
        state: state.into(),
        wsl_installed: true,
        distro: Some(distro),
        linux_user,
        bridge_running,
        bridge_pid,
        claude_running,
        claude_pid,
        bridge_healthy,
        windows_bridge_pid,
        runtime_ready,
        source_binary_present,
        bridge_venv_present,
        wsl_storage_path: windows_storage.wsl_base_path,
        wsl_storage_drive: windows_storage.wsl_drive,
        wsl_storage_free_gb,
        wsl_vhdx_size_gb,
        wsl_root_free_gb,
        settings_storage_drive: windows_storage.settings_drive,
        settings_storage_free_gb,
        storage_warning,
        storage_blocked,
        restart_blocked,
        warnings,
    }
}

fn project_runtime_files_present() -> bool {
    project_root()
        .map(|root| {
            root.join("proxy.py").is_file()
                && root.join("requirements.txt").is_file()
                && root
                    .join("scripts")
                    .join("start-claude-science-wsl.sh")
                    .is_file()
                && root
                    .join("skills")
                    .join("bootstrap-claude-science-wsl")
                    .join("scripts")
                    .join("inspect-wsl.sh")
                    .is_file()
        })
        .unwrap_or(false)
}

fn project_root() -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.to_path_buf());
        }
    }

    if let Some(argv0) = std::env::args_os().next() {
        let argv0 = PathBuf::from(argv0);
        let argv0 = if argv0.is_absolute() {
            argv0
        } else if let Ok(cwd) = std::env::current_dir() {
            cwd.join(argv0)
        } else {
            argv0
        };
        if let Some(parent) = argv0.parent() {
            candidates.push(parent.to_path_buf());
        }
    }

    if cfg!(debug_assertions) {
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd);
        }
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    }

    candidates
        .iter()
        .find_map(|candidate| find_project_root_from(candidate))
        .ok_or_else(|| {
            "无法定位项目目录：请把启动器放在包含 proxy.py 和 scripts/ 的项目目录中".to_string()
        })
}

fn find_project_root_from(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        if ancestor.join("proxy.py").is_file()
            && ancestor.join("requirements.txt").is_file()
            && ancestor
                .join("scripts")
                .join("start-claude-science-wsl.sh")
                .is_file()
        {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn settings_path() -> Result<PathBuf, String> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .ok_or_else(|| "无法定位用户配置目录".to_string())?;
    Ok(base.join("ClaudeScienceAssistant").join("settings.json"))
}

fn research_os_root() -> Result<PathBuf, String> {
    let parent = settings_path()?
        .parent()
        .ok_or_else(|| "Research OS 配置路径无父目录".to_string())?
        .to_path_buf();
    Ok(parent.join("research-os"))
}

fn research_os_settings_path() -> Result<PathBuf, String> {
    Ok(research_os_root()?.join("repositories.json"))
}

fn connect_settings_path() -> Result<PathBuf, String> {
    let parent = settings_path()?
        .parent()
        .ok_or_else(|| "Connect 配置路径无父目录".to_string())?
        .to_path_buf();
    Ok(parent.join("connect.json"))
}

struct PreparedAtomicWrite {
    destination: PathBuf,
    temporary: PathBuf,
    committed: bool,
}

impl PreparedAtomicWrite {
    fn commit(mut self) -> Result<(), String> {
        replace_file_atomically(&self.temporary, &self.destination)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PreparedAtomicWrite {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, destination: &Path) -> Result<(), String> {
    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(format!(
            "无法原子替换配置文件：{}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file_atomically(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination).map_err(|error| format!("无法原子替换配置文件：{error}"))
}

fn prepare_atomic_write(path: &Path, content: &str) -> Result<PreparedAtomicWrite, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "配置路径无父目录".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建配置目录：{error}"))?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let tmp = path.with_extension(format!("json.{}.{suffix}.tmp", std::process::id()));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|error| format!("无法写入临时配置：{error}"))?;
        file.write_all(content.as_bytes())
            .map_err(|error| format!("无法写入配置内容：{error}"))?;
        file.sync_all()
            .map_err(|error| format!("无法同步配置：{error}"))?;
    }
    Ok(PreparedAtomicWrite {
        destination: path.to_path_buf(),
        temporary: tmp,
        committed: false,
    })
}

fn provider_catalog() -> Vec<ProviderCatalogGroup> {
    vec![
        ProviderCatalogGroup {
            title: "官方直连".into(),
            tier: "official".into(),
            providers: vec![
                ProviderPreset {
                    id: "glm".into(),
                    name: "GLM-5.2".into(),
                    meta: "智谱官方 API".into(),
                    badge: "官方".into(),
                    trust: "official".into(),
                    protocol: "openai-compatible".into(),
                    base_url: Some("https://open.bigmodel.cn/api/paas/v4".into()),
                    default_model: None,
                },
                ProviderPreset {
                    id: "longcat".into(),
                    name: "LongCat".into(),
                    meta: "Anthropic 兼容".into(),
                    badge: "官方".into(),
                    trust: "official".into(),
                    protocol: "openai-compatible".into(),
                    base_url: Some("https://api.longcat.chat/openai".into()),
                    default_model: None,
                },
                ProviderPreset {
                    id: "deepseek".into(),
                    name: "DeepSeek".into(),
                    meta: "官方 API".into(),
                    badge: "官方".into(),
                    trust: "official".into(),
                    protocol: "anthropic-compatible".into(),
                    base_url: Some("https://api.deepseek.com/anthropic".into()),
                    default_model: None,
                },
                ProviderPreset {
                    id: "minimax".into(),
                    name: "MiniMax".into(),
                    meta: "中国区官方 API / Anthropic 兼容".into(),
                    badge: "官方".into(),
                    trust: "official".into(),
                    protocol: "anthropic-compatible".into(),
                    base_url: Some("https://api.minimaxi.com/anthropic".into()),
                    default_model: None,
                },
                ProviderPreset {
                    id: "claude".into(),
                    name: "Claude".into(),
                    meta: "官方登录 / API".into(),
                    badge: "官方".into(),
                    trust: "official".into(),
                    protocol: "official-login-or-api".into(),
                    base_url: None,
                    default_model: None,
                },
                ProviderPreset {
                    id: "openai".into(),
                    name: "OpenAI / GPT".into(),
                    meta: "官方登录 / API".into(),
                    badge: "官方".into(),
                    trust: "official".into(),
                    protocol: "official-login-or-api".into(),
                    base_url: Some("https://api.openai.com/v1".into()),
                    default_model: None,
                },
            ],
        },
        ProviderCatalogGroup {
            title: "聚合与编程订阅".into(),
            tier: "aggregator".into(),
            providers: vec![
                ProviderPreset {
                    id: "opencode-go".into(),
                    name: "OpenCode Go".into(),
                    meta: "订阅 API Key".into(),
                    badge: "聚合".into(),
                    trust: "aggregator".into(),
                    protocol: "openai-compatible".into(),
                    base_url: Some("https://opencode.ai/zen/go/v1".into()),
                    default_model: None,
                },
                ProviderPreset {
                    id: "openrouter".into(),
                    name: "OpenRouter".into(),
                    meta: "多模型路由".into(),
                    badge: "聚合".into(),
                    trust: "aggregator".into(),
                    protocol: "openai-compatible".into(),
                    base_url: Some("https://openrouter.ai/api/v1".into()),
                    default_model: None,
                },
            ],
        },
        ProviderCatalogGroup {
            title: "中转服务".into(),
            tier: "custom".into(),
            providers: vec![
                ProviderPreset {
                    id: "builtin-relay".into(),
                    name: "项目方自建中转".into(),
                    meta: "10521052.xyz/v1 · 非模型厂商官方 API".into(),
                    badge: "自建".into(),
                    trust: "untrusted-builtin".into(),
                    protocol: "openai-compatible".into(),
                    base_url: Some("https://10521052.xyz/v1".into()),
                    default_model: None,
                },
                ProviderPreset {
                    id: "custom".into(),
                    name: "自定义中转".into(),
                    meta: "用户填写 Base URL".into(),
                    badge: "自定义".into(),
                    trust: "untrusted-custom".into(),
                    protocol: "openai-compatible".into(),
                    base_url: None,
                    default_model: None,
                },
            ],
        },
    ]
}

fn provider_exists(provider_id: &str) -> bool {
    provider_catalog()
        .iter()
        .flat_map(|group| group.providers.iter())
        .any(|provider| provider.id == provider_id)
}

fn provider_by_id(provider_id: &str) -> Option<ProviderPreset> {
    provider_catalog()
        .into_iter()
        .flat_map(|group| group.providers.into_iter())
        .find(|provider| provider.id == provider_id)
}

fn load_settings() -> LauncherSettings {
    let Ok(path) = settings_path() else {
        return LauncherSettings::default();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return LauncherSettings::default();
    };
    serde_json::from_str(&text).unwrap_or_else(|_| LauncherSettings::default())
}

fn launcher_state(settings: &LauncherSettings) -> LauncherState {
    LauncherState {
        selected_provider_id: settings.selected_provider_id.clone(),
        custom_base_url: settings.custom_base_url.clone(),
        custom_confirmed: settings.custom_confirmed,
        active_api_key_id: settings.active_api_key_id.clone(),
        api_keys: settings
            .api_keys
            .iter()
            .map(|entry| ApiKeySummary {
                id: entry.id.clone(),
                provider_id: entry.provider_id.clone(),
                label: entry.label.clone(),
                base_url: entry.base_url.clone(),
                model: entry.model.clone(),
                custom_confirmed: entry.custom_confirmed,
                model_aliases: entry.model_aliases.clone(),
                has_secret: !entry.encrypted_api_key.is_empty(),
                active: settings.active_api_key_id.as_deref() == Some(entry.id.as_str()),
            })
            .collect(),
    }
}

fn run_powershell_with_stdin(script: &str, input: &str) -> Result<String, String> {
    let script = format!(
        "$utf8=New-Object System.Text.UTF8Encoding($false); [Console]::InputEncoding=$utf8; [Console]::OutputEncoding=$utf8; $OutputEncoding=$utf8; {script}"
    );
    let mut child = background_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动 Windows 密钥保护：{error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "无法打开 Windows 密钥保护输入".to_string())?;
    stdin
        .write_all(input.as_bytes())
        .map_err(|error| format!("无法写入 Windows 密钥保护输入：{error}"))?;
    drop(stdin);
    let output = child
        .wait_with_output()
        .map_err(|error| format!("Windows 密钥保护执行失败：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Windows 密钥保护失败：{}",
            command_error_text(&output)
        ));
    }
    Ok(decode_console_output(&output.stdout).trim().to_string())
}

fn protect_api_key(api_key: &str) -> Result<String, String> {
    if api_key.is_empty() {
        return Ok(String::new());
    }
    run_powershell_with_stdin(
        "Add-Type -AssemblyName System.Security; $plain=[Console]::In.ReadToEnd(); $bytes=[Text.Encoding]::UTF8.GetBytes($plain); $cipher=[Security.Cryptography.ProtectedData]::Protect($bytes,$null,[Security.Cryptography.DataProtectionScope]::CurrentUser); [Console]::Out.Write([Convert]::ToBase64String($cipher))",
        api_key,
    )
}

fn unprotect_api_key(encrypted: &str) -> Result<String, String> {
    if encrypted.is_empty() {
        return Ok(String::new());
    }
    run_powershell_with_stdin(
        "Add-Type -AssemblyName System.Security; $encoded=[Console]::In.ReadToEnd(); $cipher=[Convert]::FromBase64String($encoded); $plain=[Security.Cryptography.ProtectedData]::Unprotect($cipher,$null,[Security.Cryptography.DataProtectionScope]::CurrentUser); [Console]::Out.Write([Text.Encoding]::UTF8.GetString($plain))",
        encrypted,
    )
}

fn next_api_key_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("key-{nanos}-{}", std::process::id())
}

#[cfg(windows)]
fn current_local_date() -> String {
    let mut value = WindowsSystemTime::default();
    unsafe { GetLocalTime(&mut value) };
    format!("{:04}-{:02}-{:02}", value.year, value.month, value.day)
}

#[cfg(not(windows))]
fn current_local_date() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64 / 86_400)
        .unwrap_or_default();
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn validate_display_name(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.chars().any(char::is_control) {
        return Err("中转名称不能包含控制字符".into());
    }
    if trimmed.chars().count() > 80 {
        return Err("中转名称不能超过 80 个字符".into());
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
fn custom_relay_label_for_date(
    settings: &LauncherSettings,
    requested_name: &str,
    date: &str,
) -> Result<String, String> {
    provider_entry_label_for_date(settings, "custom", "自定义中转", requested_name, date)
}

fn provider_entry_label_for_date(
    settings: &LauncherSettings,
    provider_id: &str,
    provider_name: &str,
    requested_name: &str,
    date: &str,
) -> Result<String, String> {
    let requested_name = validate_display_name(requested_name)?;
    if !requested_name.is_empty() {
        return Ok(requested_name);
    }
    let prefix = format!("{provider_name} {date} #");
    let highest = settings
        .api_keys
        .iter()
        .filter(|entry| entry.provider_id == provider_id)
        .filter_map(|entry| entry.label.strip_prefix(&prefix))
        .filter_map(|sequence| sequence.parse::<u32>().ok())
        .max()
        .unwrap_or_default();
    Ok(format!("{prefix}{:02}", highest.saturating_add(1)))
}

fn provider_entry_label(
    settings: &LauncherSettings,
    provider: &ProviderPreset,
    requested_name: &str,
) -> Result<String, String> {
    provider_entry_label_for_date(
        settings,
        &provider.id,
        &provider.name,
        requested_name,
        &current_local_date(),
    )
}

fn validate_base_url(value: &str) -> Result<String, String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if !trimmed.starts_with("https://") {
        return Err("自定义中转地址必须使用 https://".into());
    }
    if trimmed.len() < "https://a.b".len() || trimmed.contains(char::is_whitespace) {
        return Err("自定义中转地址格式无效".into());
    }
    Ok(trimmed.to_string())
}

fn runtime_profile_for_provider(
    provider_id: &str,
    custom_base_url: &str,
    custom_confirmed: bool,
) -> Result<Option<BridgeRuntimeProfile>, String> {
    let provider = provider_by_id(provider_id).ok_or_else(|| "未知 Provider".to_string())?;
    let profile = match provider_id {
        "glm" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "custom",
            api_key_field: "custom_api_key",
            base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
            upstream_mode: "openai",
            default_model: String::new(),
            default_fast_model: String::new(),
            requires_explicit_model: true,
        },
        "longcat" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "custom",
            api_key_field: "custom_api_key",
            base_url: "https://api.longcat.chat/openai".into(),
            upstream_mode: "openai",
            default_model: String::new(),
            default_fast_model: String::new(),
            requires_explicit_model: true,
        },
        "deepseek" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "deepseek",
            api_key_field: "deepseek_api_key",
            base_url: "https://api.deepseek.com/anthropic".into(),
            upstream_mode: "anthropic",
            default_model: String::new(),
            default_fast_model: String::new(),
            requires_explicit_model: true,
        },
        "minimax" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "custom",
            api_key_field: "custom_api_key",
            base_url: "https://api.minimaxi.com/anthropic".into(),
            upstream_mode: "anthropic",
            default_model: String::new(),
            default_fast_model: String::new(),
            requires_explicit_model: true,
        },
        "claude" => return Ok(None),
        "openai" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "openai",
            api_key_field: "openai_api_key",
            base_url: "https://api.openai.com/v1".into(),
            upstream_mode: "openai",
            default_model: String::new(),
            default_fast_model: String::new(),
            requires_explicit_model: true,
        },
        "opencode-go" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "custom",
            api_key_field: "custom_api_key",
            base_url: "https://opencode.ai/zen/go/v1".into(),
            upstream_mode: "openai",
            default_model: String::new(),
            default_fast_model: String::new(),
            requires_explicit_model: true,
        },
        "openrouter" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "custom",
            api_key_field: "custom_api_key",
            base_url: "https://openrouter.ai/api/v1".into(),
            upstream_mode: "openai",
            default_model: String::new(),
            default_fast_model: String::new(),
            requires_explicit_model: true,
        },
        "builtin-relay" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "custom",
            api_key_field: "custom_api_key",
            base_url: "https://10521052.xyz/v1".into(),
            upstream_mode: "openai",
            default_model: String::new(),
            default_fast_model: String::new(),
            requires_explicit_model: true,
        },
        "custom" => {
            if !custom_confirmed {
                return Ok(None);
            }
            let base_url = validate_base_url(custom_base_url)?;
            if base_url.is_empty() {
                return Err("确认自定义中转前，请先填写 Base URL".into());
            }
            BridgeRuntimeProfile {
                provider_id: provider_id.into(),
                label: provider.name,
                backend: "custom",
                api_key_field: "custom_api_key",
                base_url,
                upstream_mode: "openai",
                default_model: String::new(),
                default_fast_model: String::new(),
                requires_explicit_model: true,
            }
        }
        _ => return Err("未知 Provider".into()),
    };
    Ok(Some(profile))
}

fn documented_models_for_profile(profile: &BridgeRuntimeProfile) -> Vec<String> {
    match profile.provider_id.as_str() {
        "deepseek" => vec!["deepseek-v4-pro".into(), "deepseek-v4-flash".into()],
        "minimax" => vec![
            "MiniMax-M3".into(),
            "MiniMax-M2.7".into(),
            "MiniMax-M2.7-highspeed".into(),
            "MiniMax-M2.5".into(),
            "MiniMax-M2.5-highspeed".into(),
            "MiniMax-M2.1".into(),
            "MiniMax-M2.1-highspeed".into(),
            "MiniMax-M2".into(),
        ],
        _ => Vec::new(),
    }
}

fn runtime_profile_for_settings(
    settings: &LauncherSettings,
) -> Result<Option<BridgeRuntimeProfile>, String> {
    runtime_profile_for_provider(
        &settings.selected_provider_id,
        &settings.custom_base_url,
        settings.custom_confirmed,
    )
}

fn selected_runtime_model(profile: &BridgeRuntimeProfile, model: &str) -> Result<String, String> {
    let selected = model.trim();
    if !selected.is_empty() {
        return Ok(selected.to_string());
    }
    if profile.requires_explicit_model {
        return Err(format!(
            "{} 需要先填写模型 ID，或者点击“测试连通”自动选择一个可用模型。",
            profile.label
        ));
    }
    Ok(String::new())
}

fn primary_model_from_aliases(model_aliases: &[StoredModelAlias]) -> Option<String> {
    model_aliases
        .iter()
        .find(|alias| alias.id == "byok-model-0001" && !alias.model.trim().is_empty())
        .or_else(|| {
            model_aliases
                .iter()
                .find(|alias| !alias.model.trim().is_empty())
        })
        .map(|alias| alias.model.trim().to_string())
}

fn clean_model_aliases(model_aliases: &[StoredModelAlias]) -> Vec<StoredModelAlias> {
    let mut aliases = Vec::new();
    for alias in model_aliases {
        let id = alias.id.trim();
        let model = alias.model.trim();
        if id.is_empty() || model.is_empty() {
            continue;
        }
        if aliases.iter().any(|item: &StoredModelAlias| item.id == id) {
            continue;
        }
        let display_name = alias.display_name.trim();
        aliases.push(StoredModelAlias {
            id: id.to_string(),
            display_name: if display_name.is_empty() {
                format!("{id} -> {model}")
            } else {
                display_name.to_string()
            },
            model: model.to_string(),
        });
    }
    aliases
}

fn default_model_aliases(primary_model: &str, fast_model: &str) -> Vec<StoredModelAlias> {
    let primary = primary_model.trim();
    if primary.is_empty() {
        return Vec::new();
    }
    let fast = if fast_model.trim().is_empty() {
        primary
    } else {
        fast_model.trim()
    };
    vec![
        StoredModelAlias {
            id: "byok-model-0001".into(),
            display_name: format!("BYOK 主力模型 -> {primary}"),
            model: primary.into(),
        },
        StoredModelAlias {
            id: "claude-sonnet-5".into(),
            display_name: format!("Claude Sonnet 5 -> {primary}"),
            model: primary.into(),
        },
        StoredModelAlias {
            id: "claude-sonnet-4-5".into(),
            display_name: format!("Claude Sonnet 4.5 -> {primary}"),
            model: primary.into(),
        },
        StoredModelAlias {
            id: "claude-opus-4-8".into(),
            display_name: format!("Claude Opus 4.8 -> {primary}"),
            model: primary.into(),
        },
        StoredModelAlias {
            id: "claude-haiku-4-5-20251001".into(),
            display_name: format!("Claude Haiku 4.5 / Fast -> {fast}"),
            model: fast.into(),
        },
    ]
}

fn canonical_model_for_profile(profile: &BridgeRuntimeProfile, model: &str) -> String {
    let clean = model.trim();
    if clean.is_empty() {
        return String::new();
    }
    let lower = clean.to_ascii_lowercase();
    match profile.provider_id.as_str() {
        "deepseek" => match lower.as_str() {
            // Keep official IDs intact. Only repair the user's known speech-input
            // typo; do not silently turn an unrelated or stale model into a paid
            // DeepSeek model.
            "deep-chat" => "deepseek-chat".into(),
            "deepseek-chat" => "deepseek-chat".into(),
            "deepseek-reasoner" => "deepseek-reasoner".into(),
            "deepseek-v4-pro" => "deepseek-v4-pro".into(),
            "deepseek-v4-flash" => "deepseek-v4-flash".into(),
            _ => clean.to_string(),
        },
        "longcat" => {
            if lower == "longcat-2.0" || lower == "longcat2" || lower == "longcat" {
                "LongCat-2.0".into()
            } else {
                clean.to_string()
            }
        }
        "minimax" => match lower.as_str() {
            "minimax-m3" => "MiniMax-M3".into(),
            "minimax-m2.7" => "MiniMax-M2.7".into(),
            "minimax-m2.7-highspeed" => "MiniMax-M2.7-highspeed".into(),
            "minimax-m2.5" => "MiniMax-M2.5".into(),
            "minimax-m2.5-highspeed" => "MiniMax-M2.5-highspeed".into(),
            "minimax-m2.1" => "MiniMax-M2.1".into(),
            "minimax-m2.1-highspeed" => "MiniMax-M2.1-highspeed".into(),
            "minimax-m2" => "MiniMax-M2".into(),
            _ => clean.to_string(),
        },
        "opencode-go" => {
            if lower.starts_with("opencode-go/") {
                clean["opencode-go/".len()..].to_string()
            } else {
                clean.to_string()
            }
        }
        _ => clean.to_string(),
    }
}

fn default_aliases_for_profile(
    profile: &BridgeRuntimeProfile,
    selected_model: &str,
) -> Vec<StoredModelAlias> {
    let primary = canonical_model_for_profile(profile, selected_model);
    if primary.is_empty() {
        return Vec::new();
    }
    let default_model = canonical_model_for_profile(profile, &profile.default_model);
    let fast = if !profile.default_fast_model.trim().is_empty() && primary == default_model {
        canonical_model_for_profile(profile, &profile.default_fast_model)
    } else {
        primary.clone()
    };
    default_model_aliases(&primary, &fast)
}

fn effective_model_aliases(
    model: &str,
    model_aliases: &[StoredModelAlias],
) -> Vec<StoredModelAlias> {
    let aliases = clean_model_aliases(model_aliases);
    if !aliases.is_empty() {
        return aliases;
    }
    default_model_aliases(model, model)
}

fn looks_like_chat_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    let blocked = [
        "embedding",
        "embed",
        "rerank",
        "ranker",
        "tts",
        "speech",
        "audio",
        "whisper",
        "image",
        "moderation",
        "ocr",
    ];
    !blocked.iter().any(|keyword| lower.contains(keyword))
}

fn opencode_go_openai_model_id(model: &str) -> bool {
    let lower = model.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "glm-5.2"
            | "glm-5.1"
            | "qwen3.7-max"
            | "qwen3.7"
            | "kimi-k2.7-code"
            | "kimi-k2.6"
            | "deepseek-v4-pro"
            | "deepseek-v4-flash"
            | "minimax-m3"
            | "mimo-v2.5"
            | "mimo-v2.5-pro"
    )
}

fn auto_mapping_inputs_for_profile(
    profile: &BridgeRuntimeProfile,
    models: &[String],
    fallback_model: &str,
) -> (Vec<String>, String) {
    if profile.provider_id != "opencode-go" || profile.upstream_mode != "openai" {
        return (models.to_vec(), fallback_model.trim().to_string());
    }

    let allowed_models = models
        .iter()
        .filter(|model| opencode_go_openai_model_id(model))
        .cloned()
        .collect::<Vec<_>>();
    let fallback = if opencode_go_openai_model_id(fallback_model) {
        fallback_model.trim().to_string()
    } else {
        String::new()
    };
    (allowed_models, fallback)
}

fn chat_model_candidates(models: &[String], fallback_model: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let fallback = fallback_model.trim();
    if !fallback.is_empty() {
        candidates.push(fallback.to_string());
    }
    for model in models {
        let clean = model.trim();
        if clean.is_empty() || candidates.iter().any(|item| item == clean) {
            continue;
        }
        candidates.push(clean.to_string());
    }
    let filtered: Vec<String> = candidates
        .iter()
        .filter(|model| looks_like_chat_model(model))
        .cloned()
        .collect();
    if filtered.is_empty() {
        candidates
    } else {
        filtered
    }
}

fn model_keyword_score(model: &str, keywords: &[(&str, i32)]) -> i32 {
    let lower = model.to_ascii_lowercase();
    keywords
        .iter()
        .filter(|(keyword, _)| lower.contains(keyword))
        .map(|(_, score)| *score)
        .sum()
}

fn primary_model_score(model: &str) -> i32 {
    model_keyword_score(
        model,
        &[
            ("glm-5.2", 140),
            ("qwen3.7-max", 132),
            ("qwen3.7", 124),
            ("glm-5", 120),
            ("deepseek-v4-pro", 118),
            ("gpt-5", 115),
            ("grok-4", 110),
            ("minimax-m3", 108),
            ("opus", 105),
            ("pro", 100),
            ("ultra", 95),
            ("max", 90),
            ("deepseek-v3.2", 90),
            ("kimi-k2.6", 85),
            ("longcat-2.0", 82),
            ("longcat", 80),
            ("sonnet", 80),
            ("reasoner", 75),
            ("thinking", 65),
            ("plus", 60),
            ("large", 55),
            ("qwen3", 55),
            ("gpt-4.1", 55),
            ("gpt-4o", 50),
            ("deepseek-chat", 45),
            ("deepseek-v4-flash", 42),
            ("deepseek", 40),
            ("glm-4.5", 40),
            ("flash", -35),
            ("mini", -40),
            ("lite", -35),
            ("nano", -35),
            ("air", -25),
            ("haiku", -25),
            ("fast", -15),
        ],
    )
}

fn fast_model_score(model: &str) -> i32 {
    model_keyword_score(
        model,
        &[
            ("grok-4.20-fast", 150),
            ("deepseek-v4-flash", 145),
            ("minimax-m2.7-highspeed", 140),
            ("minimax-m2.5-highspeed", 135),
            ("fast", 125),
            ("flash", 120),
            ("highspeed", 118),
            ("mini", 110),
            ("lite", 105),
            ("air", 100),
            ("haiku", 95),
            ("turbo", 80),
            ("nano", 75),
            ("deepseek-chat", 50),
            ("minimax-m3", 48),
            ("deepseek-v3", 45),
            ("glm-4.5-air", 45),
            ("pro", -45),
            ("max", -45),
            ("ultra", -45),
            ("reasoner", -50),
            ("thinking", -45),
            ("opus", -40),
        ],
    )
}

fn best_model_index_by_score<F>(models: &[String], score_fn: F) -> usize
where
    F: Fn(&str) -> i32,
{
    let mut best_index = 0usize;
    let mut best_score = i32::MIN;
    for (index, model) in models.iter().enumerate() {
        let score = score_fn(model);
        if score > best_score {
            best_score = score;
            best_index = index;
        }
    }
    best_index
}

fn auto_model_mapping(
    models: &[String],
    fallback_model: &str,
) -> Result<(String, String, Vec<StoredModelAlias>, Vec<String>), String> {
    let candidates = chat_model_candidates(models, fallback_model);
    if candidates.is_empty() {
        return Err("无法获取可映射的模型列表；请先测试连通，或手动填写一个模型 ID。".into());
    }
    let primary_index = best_model_index_by_score(&candidates, primary_model_score);
    let primary_model = candidates[primary_index].clone();
    let mut fast_index = best_model_index_by_score(&candidates, fast_model_score);
    if fast_index == primary_index && candidates.len() > 1 {
        if let Some((index, _)) = candidates
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != primary_index)
            .map(|(index, model)| (index, fast_model_score(model)))
            .filter(|(_, score)| *score > 0)
            .max_by_key(|(_, score)| *score)
        {
            fast_index = index;
        }
    }
    let fast_model = candidates[fast_index].clone();
    let aliases = default_model_aliases(&primary_model, &fast_model);
    Ok((primary_model, fast_model, aliases, candidates))
}

fn bridge_config_patch_for_provider(
    settings: &LauncherSettings,
) -> Result<Option<serde_json::Value>, String> {
    let Some(profile) = runtime_profile_for_settings(settings)? else {
        return Ok(None);
    };
    let model = canonical_model_for_profile(&profile, &profile.default_model);
    let aliases = default_aliases_for_profile(&profile, &model);
    Ok(Some(bridge_config_patch_for_runtime_profile(
        &profile, "", &model, &aliases,
    )))
}

fn bridge_config_patch_for_runtime_profile(
    profile: &BridgeRuntimeProfile,
    api_key: &str,
    model: &str,
    model_aliases: &[StoredModelAlias],
) -> serde_json::Value {
    let mut patch = serde_json::Map::new();
    let clean_model = canonical_model_for_profile(profile, model);
    let aliases = effective_model_aliases(&clean_model, model_aliases);

    patch.insert("default_backend".into(), profile.backend.into());
    patch.insert("force_model".into(), clean_model.clone().into());
    patch.insert("deepseek_api_key".into(), "".into());
    patch.insert("openai_api_key".into(), "".into());
    patch.insert("custom_api_key".into(), "".into());
    patch.insert(
        "deepseek_base_url".into(),
        "https://api.deepseek.com/anthropic".into(),
    );
    patch.insert("openai_base_url".into(), "https://api.openai.com/v1".into());
    patch.insert("custom_base_url".into(), "".into());
    patch.insert("deepseek_upstream_mode".into(), "anthropic".into());
    patch.insert("openai_upstream_mode".into(), "openai".into());
    patch.insert("custom_upstream_mode".into(), "openai".into());

    match profile.backend {
        "deepseek" => {
            patch.insert("deepseek_base_url".into(), profile.base_url.clone().into());
            patch.insert(
                "deepseek_upstream_mode".into(),
                profile.upstream_mode.into(),
            );
        }
        "openai" => {
            patch.insert("openai_base_url".into(), profile.base_url.clone().into());
            patch.insert("openai_upstream_mode".into(), profile.upstream_mode.into());
        }
        "custom" => {
            patch.insert("custom_base_url".into(), profile.base_url.clone().into());
            patch.insert("custom_upstream_mode".into(), profile.upstream_mode.into());
        }
        _ => {}
    }

    if !api_key.trim().is_empty() {
        patch.insert(profile.api_key_field.into(), api_key.trim().into());
    }

    if aliases.is_empty() {
        patch.insert("model_aliases".into(), serde_json::Value::Array(Vec::new()));
        patch.insert("model_list_mode".into(), "aliases".into());
    } else {
        let alias_values = aliases
            .iter()
            .map(|alias| {
                serde_json::json!({
                    "id": alias.id,
                    "display_name": alias.display_name,
                    "backend": profile.backend,
                    "model": alias.model
                })
            })
            .collect::<Vec<_>>();
        patch.insert(
            "model_aliases".into(),
            serde_json::Value::Array(alias_values),
        );
        patch.insert("model_list_mode".into(), "aliases".into());
    }

    serde_json::Value::Object(patch)
}

fn json_arg_hex<T: Serialize>(value: &T) -> Result<String, String> {
    let json =
        serde_json::to_string(value).map_err(|error| format!("无法序列化 Bridge 配置：{error}"))?;
    Ok(json
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>())
}

fn write_bridge_config_patch(
    distro: &str,
    patch: &serde_json::Value,
) -> Result<BridgeConfigRollback, String> {
    let patch_hex = json_arg_hex(patch)?;
    let script = r#"
import json
import os
import pathlib
import sys
import tempfile

patch = json.loads(bytes.fromhex(sys.argv[1]).decode("utf-8"))
path = pathlib.Path.home() / ".claude-science" / "proxy" / "config.json"
path.parent.mkdir(parents=True, exist_ok=True)
try:
    data = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}
    if not isinstance(data, dict):
        data = {}
except Exception:
    data = {}
restore = {}
delete = []
for key in patch:
    if key in data:
        restore[key] = data[key]
    else:
        delete.append(key)
data.update(patch)
fd, tmp = tempfile.mkstemp(prefix=".config.json.", suffix=".tmp", dir=str(path.parent))
with os.fdopen(fd, "w", encoding="utf-8") as f:
    json.dump(data, f, ensure_ascii=False, indent=2)
    f.write("\n")
    f.flush()
    os.fsync(f.fileno())
os.chmod(tmp, 0o600)
os.replace(tmp, path)
os.chmod(path, 0o600)
print(json.dumps({"restore": restore, "delete": delete}, ensure_ascii=False))
"#;
    let output = run_wsl(distro, &["python3", "-c", script, &patch_hex])?;
    if output.status.success() {
        serde_json::from_str(&output_text(&output))
            .map_err(|error| format!("Bridge 配置已写入，但回滚信息解析失败：{error}"))
    } else {
        Err(format!(
            "写入 Bridge 配置失败：{}",
            command_error_text(&output)
        ))
    }
}

fn restore_bridge_config(distro: &str, rollback: &BridgeConfigRollback) -> Result<(), String> {
    let rollback_hex = json_arg_hex(rollback)?;
    let script = r#"
import json
import os
import pathlib
import sys
import tempfile

rollback = json.loads(bytes.fromhex(sys.argv[1]).decode("utf-8"))
path = pathlib.Path.home() / ".claude-science" / "proxy" / "config.json"
path.parent.mkdir(parents=True, exist_ok=True)
try:
    data = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}
    if not isinstance(data, dict):
        data = {}
except Exception:
    data = {}
for key in rollback.get("delete", []):
    data.pop(key, None)
data.update(rollback.get("restore", {}))
fd, tmp = tempfile.mkstemp(prefix=".config.json.", suffix=".tmp", dir=str(path.parent))
with os.fdopen(fd, "w", encoding="utf-8") as f:
    json.dump(data, f, ensure_ascii=False, indent=2)
    f.write("\n")
    f.flush()
    os.fsync(f.fileno())
os.chmod(tmp, 0o600)
os.replace(tmp, path)
os.chmod(path, 0o600)
"#;
    let output = run_wsl(distro, &["python3", "-c", script, &rollback_hex])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "回滚 Bridge 配置失败：{}",
            command_error_text(&output)
        ))
    }
}

fn restart_bridge_after_config(
    status: &SystemStatus,
    expected_revision: Option<&str>,
) -> Result<(), String> {
    if !status.bridge_running {
        return Ok(());
    }
    let Some(distro) = status.distro.as_deref() else {
        return Ok(());
    };
    let project_wsl = project_root()
        .ok()
        .and_then(|root| windows_path_to_wsl(distro, &root))
        .ok_or_else(|| "Bridge 配置已写入，但无法定位当前 CSA 的 WSL 启动脚本".to_string())?;
    let start_script = format!(
        "{}/scripts/start-claude-science-wsl.sh",
        project_wsl.trim_end_matches('/')
    );
    let restart_output = run_wsl_with_timeout(
        distro,
        &[
            "env",
            "CSA_FORCE_RESTART=1",
            "PROXY_PORT=9876",
            "bash",
            &start_script,
        ],
        Duration::from_secs(45),
    )?;
    if !restart_output.status.success() {
        return Err(format!(
            "Bridge 配置已写入，但重启失败：{}",
            command_error_text(&restart_output)
        ));
    }
    if let Some(expected_revision) = expected_revision {
        let health_output = run_wsl(
            distro,
            &[
                "curl",
                "-fsS",
                "--connect-timeout",
                "0.4",
                "--max-time",
                "2",
                "http://127.0.0.1:9876/health",
            ],
        )?;
        let health = serde_json::from_str::<serde_json::Value>(&output_text(&health_output))
            .map_err(|error| format!("Bridge 已重启，但健康结果无法解析：{error}"))?;
        let actual_revision = health
            .get("config_revision")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let source_path = health
            .get("source_path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if actual_revision != expected_revision || source_path.is_empty() {
            return Err("Bridge 已响应，但仍未加载刚保存的配置或不是当前 CSA 实例".into());
        }
    }
    Ok(())
}

fn dashboard_url_from_config(data: &serde_json::Value) -> String {
    let host = data
        .get("proxy_host")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("127.0.0.1");
    let port = data
        .get("proxy_port")
        .and_then(serde_json::Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(9876);
    let token = data
        .get("proxy_auth_token")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let mode = data
        .get("proxy_auth_mode")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("optional")
        .to_ascii_lowercase();
    if mode == "required" && !token.is_empty() {
        format!(
            "http://{host}:{port}/{}/dashboard",
            percent_encode_path_segment(token)
        )
    } else {
        format!("http://{host}:{port}/dashboard")
    }
}

fn percent_encode_path_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn apply_bridge_config_patch_value(
    mut patch: serde_json::Value,
) -> Result<AppliedBridgeConfig, String> {
    let status = current_status();
    if status.restart_blocked {
        return Err("当前诊断不允许写入 API Key/模型配置；请先处理磁盘、WSL 或安装包问题".into());
    }
    let Some(distro) = status.distro.as_deref() else {
        return Err("未检测到可用 WSL 发行版，暂不能应用 Provider 配置".into());
    };
    let revision = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let object = patch
        .as_object_mut()
        .ok_or_else(|| "Bridge 配置补丁格式无效".to_string())?;
    object.insert("_csa_revision".into(), revision.clone().into());
    let rollback = write_bridge_config_patch(distro, &patch)?;
    match restart_bridge_after_config(&status, Some(&revision)) {
        Ok(()) => Ok(AppliedBridgeConfig {
            distro: distro.to_string(),
            rollback,
            previous_status: status,
        }),
        Err(error) => {
            let rollback_message = match restore_bridge_config(distro, &rollback) {
                Ok(()) => {
                    let _ = restart_bridge_after_config(&status, None);
                    "已回滚 Bridge 配置".to_string()
                }
                Err(rollback_error) => format!("回滚失败：{rollback_error}"),
            };
            Err(format!("{error}；{rollback_message}"))
        }
    }
}

fn rollback_applied_bridge(applied: &AppliedBridgeConfig) -> Result<(), String> {
    restore_bridge_config(&applied.distro, &applied.rollback)?;
    restart_bridge_after_config(&applied.previous_status, None)
}

fn bridge_config_patch_for_api_key(
    settings: &LauncherSettings,
    api_key: &str,
    model: &str,
    model_aliases: &[StoredModelAlias],
) -> Result<Option<serde_json::Value>, String> {
    let clean_key = api_key.trim();
    if clean_key.contains(char::is_whitespace) {
        return Err("API Key 不能包含空白字符".into());
    }

    let Some(profile) = runtime_profile_for_settings(settings)? else {
        if clean_key.is_empty() {
            return Ok(None);
        }
        return Err(
            "Claude 官方模式暂不通过 Bridge 保存 API Key；请先使用 Claude Science 自身登录。"
                .into(),
        );
    };

    let model_for_runtime = if model.trim().is_empty() {
        primary_model_from_aliases(model_aliases).unwrap_or_default()
    } else {
        model.trim().to_string()
    };
    let selected_model = canonical_model_for_profile(
        &profile,
        &selected_runtime_model(&profile, &model_for_runtime)?,
    );
    let effective_aliases = if clean_model_aliases(model_aliases).is_empty() {
        default_aliases_for_profile(&profile, &selected_model)
    } else {
        clean_model_aliases(model_aliases)
    };
    Ok(Some(bridge_config_patch_for_runtime_profile(
        &profile,
        clean_key,
        &selected_model,
        &effective_aliases,
    )))
}

fn redact_secret_text(text: &str, secret: &str) -> String {
    let secret = secret.trim();
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, "[redacted-api-key]")
}

fn truncate_agent_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{head}\n\n[output truncated by CSA launcher]")
}

fn redact_agent_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let sensitive_markers = [
        "api_key",
        "apikey",
        "authorization",
        "access_token",
        "refresh_token",
        "id_token",
        "token=",
        "token:",
        "token ",
        "cookie",
        "password",
        "private_key",
        "secret",
        "bearer ",
        "sk-",
    ];
    if !sensitive_markers
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return line.to_string();
    }
    if let Some((prefix, _)) = line.split_once('=') {
        return format!("{}=[redacted]", prefix.trim_end());
    }
    if let Some((prefix, _)) = line.split_once(':') {
        return format!("{}: [redacted]", prefix.trim_end());
    }
    "[redacted-sensitive-line]".to_string()
}

fn redact_agent_output(text: &str) -> String {
    let cleaned = clean_diagnostic_text(text);
    let redacted = cleaned
        .lines()
        .map(redact_agent_line)
        .collect::<Vec<_>>()
        .join("\n");
    truncate_agent_text(&redacted, 16_000)
}

fn redact_session_content(text: &str) -> String {
    let redacted = text
        .replace('\0', "")
        .lines()
        .map(redact_agent_line)
        .collect::<Vec<_>>()
        .join("\n");
    truncate_agent_text(redacted.trim(), 12_000)
}

fn safe_claude_session_id(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() < 8 || value.len() > 128 {
        return Err("Claude session id is invalid.".to_string());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err("Claude session id contains unsupported characters.".to_string());
    }
    Ok(value.to_string())
}

fn claude_resume_command(session_id: &str) -> String {
    format!("claude --resume {session_id} -p \"<message>\"")
}

fn new_claude_session_id() -> String {
    static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = (now as u64) ^ ((now >> 64) as u64) ^ u64::from(std::process::id()) ^ counter;
    let mix = |mut value: u64| {
        value = value.wrapping_add(0x9e3779b97f4a7c15);
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    };
    let value = (u128::from(mix(seed)) << 64) | u128::from(mix(seed ^ counter.rotate_left(17)));
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

fn powershell_single_quote(value: &str) -> String {
    value.replace('\'', "''")
}

fn headed_claude_launch_script(
    claude_cli: &Path,
    root: &Path,
    prompt_path: &Path,
    session_id: &str,
    session_name: &str,
) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n$prompt = Get-Content -Raw -LiteralPath '{}'\nSet-Location -LiteralPath '{}'\n& '{}' --session-id '{}' --name '{}' --permission-mode plan $prompt\nWrite-Host ''\nWrite-Host 'CSA saved this session. You can continue here or resume it later from the launcher.'\nRead-Host 'Press Enter to close this window'\n",
        powershell_single_quote(&prompt_path.display().to_string()),
        powershell_single_quote(&root.display().to_string()),
        powershell_single_quote(&claude_cli.display().to_string()),
        powershell_single_quote(session_id),
        powershell_single_quote(session_name),
    )
}

fn launch_headed_claude_session(
    session_id: &str,
    session_name: &str,
    prompt_path: &Path,
    launch_script_path: &Path,
) -> Result<String, String> {
    let claude_cli = PathBuf::from(resolve_claude_cli()?);
    let root = project_root()?;
    let script =
        headed_claude_launch_script(&claude_cli, &root, prompt_path, session_id, session_name);
    write_text_file_atomic(launch_script_path, &script)?;

    #[cfg(windows)]
    {
        let mut powershell = Command::new("powershell.exe");
        powershell
            .creation_flags(CREATE_NEW_CONSOLE)
            .current_dir(&root)
            .args([
                "-NoLogo",
                "-NoExit",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(launch_script_path);
        powershell
            .spawn()
            .map_err(|error| format!("Unable to open the headed Claude Code session: {error}"))?;
        Ok("PowerShell".to_string())
    }

    #[cfg(not(windows))]
    {
        let _ = (session_id, session_name, prompt_path, launch_script_path);
        Err("Headed Claude Code sessions are currently supported on Windows only.".to_string())
    }
}

fn parse_claude_json_stdout(stdout_raw: &str) -> (Option<String>, Option<String>) {
    let trimmed = stdout_raw.trim_start_matches('\u{feff}').trim();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return (None, None);
    };
    let session_id = value
        .get("session_id")
        .or_else(|| value.get("sessionId"))
        .and_then(|item| item.as_str())
        .and_then(|item| safe_claude_session_id(item).ok());
    let result_text = value
        .get("result")
        .or_else(|| value.get("message"))
        .and_then(|item| item.as_str())
        .map(redact_agent_output)
        .filter(|item| !item.trim().is_empty());
    (session_id, result_text)
}

fn claude_native_exe_for_candidate(candidate: &str) -> Option<String> {
    let path = PathBuf::from(candidate);
    let parent = path.parent()?;
    let native = parent
        .join("node_modules")
        .join("@anthropic-ai")
        .join("claude-code")
        .join("bin")
        .join(if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        });
    if native.is_file() {
        Some(native.display().to_string())
    } else {
        None
    }
}

fn resolve_claude_cli() -> Result<String, String> {
    let lookup_program = if cfg!(windows) { "where.exe" } else { "which" };
    let mut command = background_command(lookup_program);
    command.arg("claude");
    let output =
        command_output_with_timeout(command, Duration::from_secs(5), "Claude Code CLI 检查")?;
    if !output.status.success() {
        return Err(
            "没有在 PATH 中找到 claude CLI。请先安装并登录 Claude Code，再回到面板重试。"
                .to_string(),
        );
    }
    let candidates = output_text(&output)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err("没有在 PATH 中找到可执行的 claude CLI。".to_string());
    }
    if let Some(native) = candidates
        .iter()
        .find_map(|candidate| claude_native_exe_for_candidate(candidate))
    {
        return Ok(native);
    }
    if cfg!(windows) {
        if let Some(exe) = candidates
            .iter()
            .find(|item| item.to_ascii_lowercase().ends_with(".exe"))
        {
            return Ok(exe.clone());
        }
        if let Some(cmd) = candidates
            .iter()
            .find(|item| item.to_ascii_lowercase().ends_with(".cmd"))
        {
            return Ok(cmd.clone());
        }
    }
    Ok(candidates[0].clone())
}

fn run_claude_agent_once(
    claude_cli: &str,
    prompt: &str,
    json_output: bool,
    resume_session_id: Option<&str>,
) -> Result<(Output, u128), String> {
    let root = project_root()?;
    let started = Instant::now();
    let mut command = background_command(claude_cli);
    command.current_dir(root);
    if let Some(session_id) = resume_session_id {
        command.args(["--resume", session_id]);
    }
    command.args(["-p", prompt]);
    if json_output {
        command.args(["--output-format", "json"]);
    }
    let output =
        command_output_with_timeout(command, Duration::from_secs(120), "外部 Claude Code Agent")?;
    Ok((output, started.elapsed().as_millis()))
}

fn should_retry_agent_without_json(output: &Output) -> bool {
    if output.status.success() || !output.stdout.is_empty() {
        return false;
    }
    let text = format!(
        "{}\n{}",
        decode_console_output(&output.stderr),
        decode_console_output(&output.stdout)
    )
    .to_ascii_lowercase();
    text.contains("output-format")
        || text.contains("unknown option")
        || text.contains("unknown argument")
        || text.contains("invalid option")
}

fn run_external_agent_task_impl(prompt: String) -> Result<ExternalAgentRunResult, String> {
    run_external_agent_task_with_resume_impl(prompt, None)
}

fn run_external_agent_task_with_resume_impl(
    prompt: String,
    resume_session_id: Option<String>,
) -> Result<ExternalAgentRunResult, String> {
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("请先选择任务或填写 Prompt，再启动外部 Agent。".to_string());
    }
    if prompt.chars().count() > 24_000 {
        return Err("当前 Prompt 过长，可能超过 Windows 命令行限制。请先复制 Prompt 手动运行 Claude Code，或缩短任务说明后重试。".to_string());
    }

    let resume_session_id = resume_session_id
        .map(|value| safe_claude_session_id(&value))
        .transpose()?;
    let claude_cli = resolve_claude_cli()?;
    let (mut output, mut duration_ms) =
        run_claude_agent_once(&claude_cli, &prompt, true, resume_session_id.as_deref())?;
    if should_retry_agent_without_json(&output) {
        let (fallback_output, fallback_duration_ms) =
            run_claude_agent_once(&claude_cli, &prompt, false, resume_session_id.as_deref())?;
        output = fallback_output;
        duration_ms += fallback_duration_ms;
    }

    let ok = output.status.success();
    let exit_code = output.status.code();
    let stdout_raw = decode_console_output(&output.stdout);
    let stderr_raw = decode_console_output(&output.stderr);
    let (parsed_session_id, result_text) = parse_claude_json_stdout(&stdout_raw);
    let session_id = parsed_session_id.or(resume_session_id);
    let resume_command = session_id.as_deref().map(claude_resume_command);
    let stdout = redact_agent_output(&stdout_raw);
    let stderr = redact_agent_output(&stderr_raw);
    let message = if ok {
        "外部 Claude Code 已完成，结果已写入下方输出区。".to_string()
    } else {
        format!(
            "外部 Claude Code 退出码为 {}。请查看 stdout/stderr 后决定是否复制 Prompt 手动继续。",
            exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        )
    };

    Ok(ExternalAgentRunResult {
        ok,
        tool: "claude".to_string(),
        exit_code,
        duration_ms,
        stdout,
        stderr,
        result_text,
        session_id,
        resume_command,
        message,
    })
}

#[tauri::command]
async fn run_external_agent_task(prompt: String) -> Result<ExternalAgentRunResult, String> {
    run_blocking(move || run_external_agent_task_impl(prompt)).await
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn subagent_inbox_dir() -> Result<PathBuf, String> {
    Ok(project_root()?.join("reports").join("csa-agent-inbox"))
}

fn subagent_runs_dir() -> Result<PathBuf, String> {
    Ok(project_root()?.join("reports").join("csa-agent-runs"))
}

fn subagent_outbox_dir() -> Result<PathBuf, String> {
    Ok(project_root()?.join("reports").join("csa-agent-outbox"))
}

fn safe_subagent_id(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value == "." || value == ".." || value.chars().count() > 96 {
        return Err("Subagent request id is invalid.".to_string());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return Err(
            "Subagent request id may only contain ASCII letters, numbers, '.', '-' and '_'."
                .to_string(),
        );
    }
    Ok(value.to_string())
}

fn write_text_file_atomic(path: &Path, content: &str) -> Result<(), String> {
    prepare_atomic_write(path, content)?.commit()
}

fn build_subagent_outbox_result(
    request_id: &str,
    run_id: &str,
    agent: &ExternalAgentRunResult,
) -> Result<SubagentOutboxResult, String> {
    let request_id = safe_subagent_id(request_id)?;
    let run_id = safe_optional_subagent_run_id(Some(run_id.to_string()))?
        .ok_or_else(|| "Subagent run id is missing.".to_string())?;
    let completed = agent.ok
        && agent
            .result_text
            .as_deref()
            .or_else(|| (!agent.stdout.trim().is_empty()).then_some(agent.stdout.as_str()))
            .is_some();
    let status = if completed {
        "completed"
    } else if agent.ok {
        "running"
    } else {
        "failed"
    };
    let summary_source = agent
        .result_text
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| (!agent.stdout.trim().is_empty()).then_some(agent.stdout.as_str()))
        .or_else(|| (!agent.stderr.trim().is_empty()).then_some(agent.stderr.as_str()))
        .unwrap_or(agent.message.as_str());
    let summary = truncate_agent_text(&redact_agent_output(summary_source), 4_000);
    let next_action = match status {
        "completed" => "read_result",
        "running" => "wait_for_result",
        _ => "review_failure",
    };
    Ok(SubagentOutboxResult {
        schema_version: 1,
        request_id,
        status: status.to_string(),
        latest_run_id: run_id.clone(),
        session_id: agent.session_id.clone(),
        result_path: format!("reports/csa-agent-runs/{run_id}/result.json"),
        summary,
        next_action: next_action.to_string(),
        updated_at: unix_millis(),
    })
}

fn write_subagent_outbox(
    request_id: &str,
    run_id: &str,
    agent: &ExternalAgentRunResult,
) -> Result<PathBuf, String> {
    let value = build_subagent_outbox_result(request_id, run_id, agent)?;
    let outbox = subagent_outbox_dir()?;
    fs::create_dir_all(&outbox)
        .map_err(|error| format!("Unable to create Subagent outbox directory: {error}"))?;
    let path = outbox.join(format!("{}.json", value.request_id));
    let encoded = serde_json::to_string_pretty(&value)
        .map_err(|error| format!("Unable to serialize Subagent outbox result: {error}"))?;
    write_text_file_atomic(&path, &(encoded + "\n"))?;
    Ok(path)
}

fn subagent_modified_ms(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

fn subagent_item_from_path(path: &Path) -> Option<SubagentInboxItem> {
    if path.extension().and_then(|value| value.to_str()) != Some("json") {
        return None;
    }
    let request_id = path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_string)?;
    let file_name = path.file_name()?.to_string_lossy().to_string();
    let modified_ms = subagent_modified_ms(path);
    match fs::read_to_string(path) {
        Ok(content) if content.len() > 128 * 1024 => Some(SubagentInboxItem {
            request_id,
            file_name,
            file_path: path.display().to_string(),
            modified_ms,
            request: None,
            parse_error: Some("request file is larger than 128 KiB".to_string()),
        }),
        Ok(content) => {
            match serde_json::from_str::<SubagentRequest>(content.trim_start_matches('\u{feff}')) {
                Ok(request) => Some(SubagentInboxItem {
                    request_id,
                    file_name,
                    file_path: path.display().to_string(),
                    modified_ms,
                    request: Some(request),
                    parse_error: None,
                }),
                Err(error) => Some(SubagentInboxItem {
                    request_id,
                    file_name,
                    file_path: path.display().to_string(),
                    modified_ms,
                    request: None,
                    parse_error: Some(error.to_string()),
                }),
            }
        }
        Err(error) => Some(SubagentInboxItem {
            request_id,
            file_name,
            file_path: path.display().to_string(),
            modified_ms,
            request: None,
            parse_error: Some(error.to_string()),
        }),
    }
}

fn list_subagent_requests_impl() -> Result<Vec<SubagentInboxItem>, String> {
    let inbox = subagent_inbox_dir()?;
    fs::create_dir_all(&inbox).map_err(|error| format!("无法创建 Subagent 收件箱目录：{error}"))?;
    let mut items = fs::read_dir(&inbox)
        .map_err(|error| format!("无法读取 Subagent 收件箱：{error}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| subagent_item_from_path(&entry.path()))
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .modified_ms
            .cmp(&left.modified_ms)
            .then_with(|| left.request_id.cmp(&right.request_id))
    });
    items.truncate(100);
    Ok(items)
}

#[tauri::command]
async fn list_subagent_requests() -> Result<Vec<SubagentInboxItem>, String> {
    run_blocking(list_subagent_requests_impl).await
}

fn request_value(value: &Option<String>, fallback: &str) -> String {
    let cleaned = value
        .as_deref()
        .unwrap_or("")
        .replace(['\r', '\n', '\0'], " ")
        .trim()
        .to_string();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

fn build_subagent_prompt(request_id: &str, request: &SubagentRequest) -> String {
    let note = redact_agent_output(request.note.as_deref().unwrap_or(""));
    [
        "你是 CSA Subagent Host Runner，正在处理一个从沙盒投递到宿主机面板的任务。",
        "默认只读：不要删除、移动、安装、上传、修改代理/DNS/hosts/证书、防火墙、系统服务、WSL、VHDX 或注册表。",
        "不要输出 API Key、token、cookie、私钥、完整 .env、完整聊天记录或浏览器数据。",
        "本轮目标是诊断、规划、生成可执行建议；需要真实写入/安装/迁移时必须明确列为“等待用户批准”。",
        "",
        "任务元数据：",
        &format!("- requestId: {request_id}"),
        &format!("- source: {}", request_value(&request.source, "sandbox")),
        &format!("- taskKind: {}", request_value(&request.task_kind, "custom")),
        &format!("- title: {}", request_value(&request.title, "untitled")),
        &format!("- cwd: {}", request_value(&request.cwd, "unknown")),
        &format!(
            "- requestedAction: {}",
            request_value(&request.requested_action, "diagnose")
        ),
        &format!(
            "- approvalMode: {}",
            request_value(&request.approval_mode, "manual")
        ),
        &format!("- policyId: {}", request_value(&request.policy_id, "none")),
        &format!("- createdAt: {}", request_value(&request.created_at, "unknown")),
        "",
        "沙盒提交的脱敏说明：",
        if note.trim().is_empty() {
            "(empty)"
        } else {
            note.trim()
        },
        "",
        "请按以下结构输出：",
        "A. 结论：可解决 / 需要更多信息 / 当前不应执行。",
        "B. 证据：你只读检查到或根据任务元数据判断出的事实。",
        "C. 下一步计划：按最小风险顺序列出。",
        "D. 需要用户批准的命令：如果没有就写“无”。",
        "E. 可以回写给沙盒的结果摘要。",
    ]
    .join("\n")
}

fn read_subagent_request(request_id: &str) -> Result<(PathBuf, SubagentRequest), String> {
    let request_id = safe_subagent_id(request_id)?;
    let path = subagent_inbox_dir()?.join(format!("{request_id}.json"));
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("无法读取 Subagent request {request_id}：{error}"))?;
    let request = serde_json::from_str::<SubagentRequest>(content.trim_start_matches('\u{feff}'))
        .map_err(|error| format!("Subagent request {request_id} JSON 无效：{error}"))?;
    Ok((path, request))
}

fn run_subagent_request_impl(
    request_id: String,
    prompt_override: String,
) -> Result<SubagentRunResult, String> {
    let request_id = safe_subagent_id(&request_id)?;
    let (request_path, request) = read_subagent_request(&request_id)?;
    let prompt = if prompt_override.trim().is_empty() {
        build_subagent_prompt(&request_id, &request)
    } else {
        prompt_override.trim().to_string()
    };
    let run_id = format!("{request_id}-{}", unix_millis());
    let result_dir = subagent_runs_dir()?.join(&run_id);
    fs::create_dir_all(&result_dir)
        .map_err(|error| format!("无法创建 Subagent run 目录：{error}"))?;

    let request_json = fs::read_to_string(&request_path).unwrap_or_else(|_| "{}".to_string());
    write_text_file_atomic(&result_dir.join("request.json"), &request_json)?;
    let prompt_path = result_dir.join("prompt.md");
    write_text_file_atomic(&prompt_path, &prompt)?;

    let session_id = new_claude_session_id();
    let launch_script_path = result_dir.join("launch-session.ps1");
    let session_name = format!("CSA {}", short_run_prefix(&request_id));
    let launched_at = unix_millis();
    let agent = match launch_headed_claude_session(
        &session_id,
        &session_name,
        &prompt_path,
        &launch_script_path,
    ) {
        Ok(terminal) => ExternalAgentRunResult {
            ok: true,
            tool: "claude".to_string(),
            exit_code: None,
            duration_ms: 0,
            stdout: String::new(),
            stderr: String::new(),
            result_text: None,
            session_id: Some(session_id.clone()),
            resume_command: Some(claude_resume_command(&session_id)),
            message: format!(
                "Headed Claude Code session opened in {terminal}; local transcript collection is active."
            ),
        },
        Err(error) => ExternalAgentRunResult {
            ok: false,
            tool: "claude".to_string(),
            exit_code: None,
            duration_ms: 0,
            stdout: String::new(),
            stderr: redact_agent_output(&error),
            result_text: None,
            session_id: Some(session_id.clone()),
            resume_command: Some(claude_resume_command(&session_id)),
            message: "Unable to open the headed Claude Code session; details were written to result.json."
                .to_string(),
        },
    };
    write_text_file_atomic(&result_dir.join("stdout.txt"), &agent.stdout)?;
    write_text_file_atomic(&result_dir.join("stderr.txt"), &agent.stderr)?;

    let result = SubagentRunResult {
        run_id,
        request_id,
        result_dir: result_dir.display().to_string(),
        result_json_path: result_dir.join("result.json").display().to_string(),
        agent,
    };
    let result_json = serde_json::to_string_pretty(&result)
        .map_err(|error| format!("无法序列化 Subagent run 结果：{error}"))?;
    write_text_file_atomic(&result_dir.join("result.json"), &(result_json + "\n"))?;
    write_subagent_outbox(&result.request_id, &result.run_id, &result.agent)?;
    if result.agent.ok {
        watch_headed_subagent_result(result.clone(), launched_at);
    }
    Ok(result)
}

#[tauri::command]
async fn run_subagent_request(
    request_id: String,
    prompt_override: String,
) -> Result<SubagentRunResult, String> {
    run_blocking(move || run_subagent_request_impl(request_id, prompt_override)).await
}

fn safe_optional_subagent_id(value: Option<String>) -> Result<Option<String>, String> {
    match value {
        Some(value) if !value.trim().is_empty() => safe_subagent_id(&value).map(Some),
        _ => Ok(None),
    }
}

fn safe_optional_subagent_run_id(value: Option<String>) -> Result<Option<String>, String> {
    match value {
        Some(value) if !value.trim().is_empty() => {
            let value = value.trim();
            if value.len() > 160
                || !value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
            {
                return Err("Subagent run id is invalid.".to_string());
            }
            Ok(Some(value.to_string()))
        }
        _ => Ok(None),
    }
}

fn short_run_prefix(value: &str) -> String {
    let mut prefix = value.chars().take(60).collect::<String>();
    while prefix.ends_with(['.', '-', '_']) {
        prefix.pop();
    }
    if prefix.is_empty() {
        "session".to_string()
    } else {
        prefix
    }
}

fn continue_subagent_session_impl(
    session_id: String,
    message: String,
    request_id: Option<String>,
    parent_run_id: Option<String>,
) -> Result<SubagentSessionReplyResult, String> {
    let session_id = safe_claude_session_id(&session_id)?;
    let message = message.trim().to_string();
    if message.is_empty() {
        return Err("Please enter a message before continuing the Subagent session.".to_string());
    }
    if message.chars().count() > 24_000 {
        return Err("Subagent continuation message is too long.".to_string());
    }

    let request_id = safe_optional_subagent_id(request_id)?;
    let parent_run_id = safe_optional_subagent_run_id(parent_run_id)?;
    let prefix = request_id
        .as_deref()
        .or(parent_run_id.as_deref())
        .map(short_run_prefix)
        .unwrap_or_else(|| "session".to_string());
    let run_id = format!("continue-{prefix}-{}", unix_millis());
    let result_dir = subagent_runs_dir()?.join(&run_id);
    fs::create_dir_all(&result_dir)
        .map_err(|error| format!("Unable to create Subagent continuation directory: {error}"))?;
    write_text_file_atomic(&result_dir.join("message.md"), &message)?;
    write_text_file_atomic(&result_dir.join("session.txt"), &session_id)?;

    let mut agent =
        match run_external_agent_task_with_resume_impl(message, Some(session_id.clone())) {
            Ok(result) => result,
            Err(error) => ExternalAgentRunResult {
                ok: false,
                tool: "claude".to_string(),
                exit_code: None,
                duration_ms: 0,
                stdout: String::new(),
                stderr: redact_agent_output(&error),
                result_text: None,
                session_id: Some(session_id.clone()),
                resume_command: Some(claude_resume_command(&session_id)),
                message: "External Agent continuation failed. Details were written to result.json."
                    .to_string(),
            },
        };
    if agent.session_id.is_none() {
        agent.session_id = Some(session_id.clone());
    }
    if agent.resume_command.is_none() {
        agent.resume_command = Some(claude_resume_command(&session_id));
    }
    write_text_file_atomic(&result_dir.join("stdout.txt"), &agent.stdout)?;
    write_text_file_atomic(&result_dir.join("stderr.txt"), &agent.stderr)?;

    let result = SubagentSessionReplyResult {
        run_id,
        request_id,
        parent_run_id,
        result_dir: result_dir.display().to_string(),
        result_json_path: result_dir.join("result.json").display().to_string(),
        agent,
    };
    let result_json = serde_json::to_string_pretty(&result)
        .map_err(|error| format!("Unable to serialize Subagent continuation result: {error}"))?;
    write_text_file_atomic(&result_dir.join("result.json"), &(result_json + "\n"))?;
    if let Some(request_id) = result.request_id.as_deref() {
        write_subagent_outbox(request_id, &result.run_id, &result.agent)?;
    }
    Ok(result)
}

#[tauri::command]
async fn continue_subagent_session(
    session_id: String,
    message: String,
    request_id: Option<String>,
    parent_run_id: Option<String>,
) -> Result<SubagentSessionReplyResult, String> {
    run_blocking(move || {
        continue_subagent_session_impl(session_id, message, request_id, parent_run_id)
    })
    .await
}

fn subagent_history_item_from_result(
    result_path: &Path,
    request_id_filter: &str,
) -> Option<SubagentRunHistoryItem> {
    let content = fs::read_to_string(result_path).ok()?;
    let modified_ms = subagent_modified_ms(result_path);
    if let Ok(mut result) =
        serde_json::from_str::<SubagentRunResult>(content.trim_start_matches('\u{feff}'))
    {
        if result.request_id == request_id_filter {
            let _ = sync_headed_subagent_result(&mut result, result_path, None);
            return Some(SubagentRunHistoryItem {
                run_id: result.run_id,
                kind: "run".to_string(),
                request_id: Some(result.request_id),
                parent_run_id: None,
                result_dir: result.result_dir,
                result_json_path: result.result_json_path,
                modified_ms,
                agent: result.agent,
            });
        }
    }
    if let Ok(result) =
        serde_json::from_str::<SubagentSessionReplyResult>(content.trim_start_matches('\u{feff}'))
    {
        if result.request_id.as_deref() == Some(request_id_filter) {
            return Some(SubagentRunHistoryItem {
                run_id: result.run_id,
                kind: "continue".to_string(),
                request_id: result.request_id,
                parent_run_id: result.parent_run_id,
                result_dir: result.result_dir,
                result_json_path: result.result_json_path,
                modified_ms,
                agent: result.agent,
            });
        }
    }
    None
}

fn list_subagent_run_history_impl(
    request_id: String,
) -> Result<Vec<SubagentRunHistoryItem>, String> {
    let request_id = safe_subagent_id(&request_id)?;
    let runs = subagent_runs_dir()?;
    fs::create_dir_all(&runs).map_err(|error| format!("无法创建 Subagent runs 目录：{error}"))?;
    let mut items = fs::read_dir(&runs)
        .map_err(|error| format!("无法读取 Subagent runs 目录：{error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("result.json"))
        .filter(|path| path.is_file())
        .filter_map(|path| subagent_history_item_from_result(&path, &request_id))
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.modified_ms
            .cmp(&right.modified_ms)
            .then_with(|| left.run_id.cmp(&right.run_id))
    });
    items.truncate(200);
    Ok(items)
}

#[tauri::command]
async fn list_subagent_run_history(
    request_id: String,
) -> Result<Vec<SubagentRunHistoryItem>, String> {
    run_blocking(move || list_subagent_run_history_impl(request_id)).await
}

fn launch_external_claude_session_impl(
    session_id: String,
) -> Result<ExternalSessionLaunchResult, String> {
    let session_id = safe_claude_session_id(&session_id)?;
    let claude_cli = resolve_claude_cli()?;
    let root = project_root()?;

    #[cfg(windows)]
    let terminal = {
        let mut windows_terminal = Command::new("wt.exe");
        windows_terminal
            .args(["-w", "0", "new-tab", "-d"])
            .arg(&root)
            .arg(&claude_cli)
            .args(["--resume", &session_id]);
        match windows_terminal.spawn() {
            Ok(_) => "Windows Terminal".to_string(),
            Err(wt_error) => {
                let quoted_cli = claude_cli.replace('\'', "''");
                let quoted_session = session_id.replace('\'', "''");
                let script = format!("& '{quoted_cli}' --resume '{quoted_session}'");
                let mut powershell = Command::new("powershell.exe");
                powershell
                    .creation_flags(CREATE_NEW_CONSOLE)
                    .current_dir(&root)
                    .args(["-NoLogo", "-NoExit", "-NoProfile", "-Command", &script]);
                powershell.spawn().map_err(|error| {
                    format!(
                        "无法打开外部 Claude Code 会话。Windows Terminal: {wt_error}; PowerShell: {error}"
                    )
                })?;
                "PowerShell".to_string()
            }
        }
    };

    #[cfg(not(windows))]
    let terminal = {
        return Err("当前版本仅支持在 Windows 中打开外部 Claude Code 终端。".to_string());
    };

    Ok(ExternalSessionLaunchResult {
        session_id: session_id.clone(),
        command: format!("claude --resume {session_id}"),
        cwd: root.display().to_string(),
        terminal: terminal.clone(),
        message: format!("已在 {terminal} 中打开 Claude Code 会话。"),
    })
}

#[tauri::command]
async fn launch_external_claude_session(
    session_id: String,
) -> Result<ExternalSessionLaunchResult, String> {
    run_blocking(move || launch_external_claude_session_impl(session_id)).await
}

fn claude_projects_root() -> Result<PathBuf, String> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| "无法定位当前用户目录。".to_string())?;
    Ok(PathBuf::from(home).join(".claude").join("projects"))
}

fn find_claude_session_file(session_id: &str) -> Result<PathBuf, String> {
    let session_id = safe_claude_session_id(session_id)?;
    let root = claude_projects_root()?;
    let file_name = format!("{session_id}.jsonl");
    let direct = root.join(&file_name);
    if direct.is_file() {
        return Ok(direct);
    }
    let entries = fs::read_dir(&root)
        .map_err(|error| format!("无法读取 Claude Code 会话目录 {}：{error}", root.display()))?;
    for entry in entries.filter_map(Result::ok).take(2_000) {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let candidate = project_dir.join(&file_name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "没有找到 Claude Code session {session_id} 的本地聊天记录。"
    ))
}

fn claude_message_text(message: &serde_json::Value) -> String {
    let Some(content) = message.get("content") else {
        return String::new();
    };
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let Some(parts) = content.as_array() else {
        return String::new();
    };
    parts
        .iter()
        .filter_map(|part| {
            if let Some(text) = part.as_str() {
                return Some(text.to_string());
            }
            if part.get("type").and_then(|value| value.as_str()) == Some("text") {
                return part
                    .get("text")
                    .and_then(|value| value.as_str())
                    .map(str::to_string);
            }
            None
        })
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_claude_session_messages(session_id: &str, content: &str) -> Vec<ClaudeSessionMessage> {
    let mut messages = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("isSidechain").and_then(|item| item.as_bool()) == Some(true) {
            continue;
        }
        let event_type = value
            .get("type")
            .and_then(|item| item.as_str())
            .unwrap_or("");
        if event_type != "user" && event_type != "assistant" {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let role = message
            .get("role")
            .and_then(|item| item.as_str())
            .unwrap_or(event_type);
        if role != "user" && role != "assistant" {
            continue;
        }
        let content = redact_session_content(&claude_message_text(message));
        if content.trim().is_empty() {
            continue;
        }
        let id = value
            .get("uuid")
            .or_else(|| message.get("id"))
            .and_then(|item| item.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{session_id}-{line_index}"));
        messages.push(ClaudeSessionMessage {
            id,
            session_id: session_id.to_string(),
            role: role.to_string(),
            kind: "text".to_string(),
            content,
            created_at: value
                .get("timestamp")
                .and_then(|item| item.as_str())
                .map(str::to_string),
        });
    }
    messages.reverse();
    messages
}

fn read_claude_session_tail(path: &Path) -> Result<(String, bool), String> {
    const MAX_SESSION_BYTES: u64 = 8 * 1024 * 1024;
    let mut file = fs::File::open(path)
        .map_err(|error| format!("无法打开 Claude Code session {}：{error}", path.display()))?;
    let length = file
        .metadata()
        .map_err(|error| format!("无法读取 Claude Code session 大小：{error}"))?
        .len();
    let truncated = length > MAX_SESSION_BYTES;
    if truncated {
        file.seek(SeekFrom::Start(length - MAX_SESSION_BYTES))
            .map_err(|error| format!("无法定位 Claude Code session 尾部：{error}"))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("无法读取 Claude Code session：{error}"))?;
    let mut content = String::from_utf8_lossy(&bytes).to_string();
    if truncated {
        if let Some(first_newline) = content.find('\n') {
            content = content[first_newline + 1..].to_string();
        }
    }
    Ok((content, truncated))
}

fn read_claude_session_history_impl(
    session_id: String,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<ClaudeSessionHistory, String> {
    let session_id = safe_claude_session_id(&session_id)?;
    let path = find_claude_session_file(&session_id)?;
    let modified_ms = subagent_modified_ms(&path);
    let (content, source_truncated) = read_claude_session_tail(&path)?;
    let messages = parse_claude_session_messages(&session_id, &content);
    let total_messages = messages.len();
    let offset = offset.unwrap_or_default().min(total_messages);
    let limit = limit.unwrap_or(50).clamp(1, 200);
    let end = offset.saturating_add(limit).min(total_messages);
    let page = messages[offset..end].to_vec();
    Ok(ClaudeSessionHistory {
        session_id,
        file_path: path.display().to_string(),
        modified_ms,
        messages: page,
        total_messages,
        has_more: end < total_messages || source_truncated,
    })
}

fn sync_headed_subagent_result(
    result: &mut SubagentRunResult,
    result_path: &Path,
    launched_at: Option<u128>,
) -> Result<bool, String> {
    let Some(session_id) = result.agent.session_id.clone() else {
        return Ok(false);
    };
    let session_path = match find_claude_session_file(&session_id) {
        Ok(path) => path,
        Err(_) => return Ok(false),
    };
    let (content, _) = read_claude_session_tail(&session_path)?;
    let messages = parse_claude_session_messages(&session_id, &content);
    let Some(latest_assistant) = messages.iter().find(|message| message.role == "assistant") else {
        return Ok(false);
    };
    if result.agent.result_text.as_deref() == Some(latest_assistant.content.as_str()) {
        return Ok(false);
    }

    result.agent.ok = true;
    result.agent.result_text = Some(latest_assistant.content.clone());
    result.agent.message =
        "Latest Claude Code answer collected from the local session transcript.".to_string();
    if let Some(started) = launched_at {
        result.agent.duration_ms = unix_millis().saturating_sub(started);
    }
    let encoded = serde_json::to_string_pretty(result)
        .map_err(|error| format!("Unable to serialize headed session result: {error}"))?;
    write_text_file_atomic(result_path, &(encoded + "\n"))?;
    write_subagent_outbox(&result.request_id, &result.run_id, &result.agent)?;
    Ok(true)
}

fn watch_headed_subagent_result(mut result: SubagentRunResult, launched_at: u128) {
    thread::spawn(move || {
        let result_path = PathBuf::from(&result.result_json_path);
        let started = Instant::now();
        let mut last_change = Instant::now();
        let mut collected = false;
        while started.elapsed() < Duration::from_secs(8 * 60 * 60) {
            if sync_headed_subagent_result(&mut result, &result_path, Some(launched_at))
                .unwrap_or(false)
            {
                collected = true;
                last_change = Instant::now();
            }
            if collected && last_change.elapsed() > Duration::from_secs(30 * 60) {
                break;
            }
            if !collected && started.elapsed() > Duration::from_secs(10 * 60) {
                break;
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
}

#[tauri::command]
async fn read_claude_session_history(
    session_id: String,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<ClaudeSessionHistory, String> {
    run_blocking(move || read_claude_session_history_impl(session_id, offset, limit)).await
}

fn load_research_os_state() -> ResearchOsState {
    let Ok(path) = research_os_settings_path() else {
        return ResearchOsState::default();
    };
    let Ok(content) = fs::read_to_string(path) else {
        return ResearchOsState::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn persist_research_os_state(state: &ResearchOsState) -> Result<(), String> {
    let body = serde_json::to_string_pretty(state)
        .map_err(|error| format!("无法序列化 Research OS 配置：{error}"))?;
    write_text_file_atomic(&research_os_settings_path()?, &(body + "\n"))
}

fn safe_skill_repository_id(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 96
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err("Skill 仓库 ID 无效".to_string());
    }
    Ok(value.to_string())
}

fn validate_skill_repository_source(value: &str) -> Result<String, String> {
    let source = value.trim();
    if source.is_empty() || source.len() > 2_048 || source.contains(['\r', '\n', '\0']) {
        return Err("请输入有效的 Git 仓库地址".to_string());
    }
    let path = Path::new(source);
    let supported = source.starts_with("https://")
        || source.starts_with("ssh://")
        || source.starts_with("git@")
        || path.is_absolute();
    if !supported {
        return Err("首版支持 HTTPS、SSH 或本机绝对路径 Git 仓库".to_string());
    }
    Ok(source.to_string())
}

fn git_output(command: Command, timeout: Duration, label: &str) -> Result<String, String> {
    let output = command_output_with_timeout(command, timeout, label)?;
    if !output.status.success() {
        return Err(format!("{label}失败：{}", command_error_text(&output)));
    }
    Ok(decode_console_output(&output.stdout).trim().to_string())
}

fn verify_system_git() -> Result<(), String> {
    let mut command = background_command("git");
    command.arg("--version");
    git_output(command, Duration::from_secs(10), "系统 Git 检查").map(|_| ())
}

fn git_clone_skill_repository(source: &str, destination: &Path) -> Result<(), String> {
    let mut command = background_command("git");
    command
        .args(["clone", "--depth", "1", "--"])
        .arg(source)
        .arg(destination);
    git_output(command, Duration::from_secs(120), "Skill 仓库浅克隆").map(|_| ())
}

fn git_pull_skill_repository(destination: &Path) -> Result<(), String> {
    let mut command = background_command("git");
    command
        .arg("-C")
        .arg(destination)
        .args(["pull", "--ff-only"]);
    git_output(command, Duration::from_secs(120), "Skill 仓库更新").map(|_| ())
}

fn git_current_commit(destination: &Path) -> Result<String, String> {
    let mut command = background_command("git");
    command
        .arg("-C")
        .arg(destination)
        .args(["rev-parse", "--short=12", "HEAD"]);
    git_output(command, Duration::from_secs(10), "读取 Skill 仓库版本")
}

fn tracked_skill_paths(destination: &Path) -> Result<Vec<String>, String> {
    let mut command = background_command("git");
    command.arg("-C").arg(destination).args(["ls-files", "-z"]);
    let output = git_output(command, Duration::from_secs(15), "扫描 Skill 仓库文件")?;
    let mut paths = output
        .split('\0')
        .filter(|value| !value.is_empty())
        .filter(|value| {
            Path::new(value).file_name().and_then(|name| name.to_str()) == Some("SKILL.md")
        })
        .filter(|value| {
            Path::new(value)
                .components()
                .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    paths.sort();
    paths.truncate(500);
    Ok(paths)
}

fn short_text(value: &str, max_chars: usize) -> String {
    value
        .replace(['\r', '\n', '\0'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

fn parse_skill_metadata(content: &str, relative_path: &str) -> (String, String) {
    let normalized = content.replace("\r\n", "\n");
    let front_matter = normalized.strip_prefix("---\n").and_then(|rest| {
        rest.find("\n---\n")
            .and_then(|end| serde_yaml::from_str::<SkillFrontMatter>(&rest[..end]).ok())
    });
    let fallback_name = Path::new(relative_path)
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("Skill");
    let heading = normalized
        .lines()
        .find_map(|line| line.trim().strip_prefix("# "))
        .unwrap_or(fallback_name);
    let name = front_matter
        .as_ref()
        .and_then(|value| value.name.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(heading)
        .to_string();
    let description = front_matter
        .as_ref()
        .and_then(|value| value.description.clone())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            normalized.lines().find_map(|line| {
                let line = line.trim();
                (!line.is_empty() && !line.starts_with('#') && line != "---" && !line.contains(":"))
                    .then(|| line.to_string())
            })
        })
        .unwrap_or_else(|| "未提供说明".to_string());
    (short_text(&name, 80), short_text(&description, 240))
}

fn stable_text_fingerprint(content: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn scan_skill_repository(
    repository_id: &str,
    destination: &Path,
    previous: &[SkillFeedItem],
) -> Result<Vec<SkillFeedItem>, String> {
    let previous_fingerprints = previous
        .iter()
        .map(|item| (item.relative_path.as_str(), item.fingerprint.as_str()))
        .collect::<HashMap<_, _>>();
    let mut skills = Vec::new();
    for relative_path in tracked_skill_paths(destination)? {
        let path = destination.join(Path::new(&relative_path));
        let metadata = match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() && metadata.len() <= 256 * 1024 => metadata,
            _ => continue,
        };
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let fingerprint = stable_text_fingerprint(&content);
        let is_new = previous_fingerprints.get(relative_path.as_str()).copied()
            != Some(fingerprint.as_str());
        let (name, description) = parse_skill_metadata(&content, &relative_path);
        let path_fingerprint = stable_text_fingerprint(&relative_path);
        skills.push(SkillFeedItem {
            id: format!("{repository_id}-{path_fingerprint}"),
            repository_id: repository_id.to_string(),
            name,
            description,
            relative_path,
            modified_ms: metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_millis().min(u128::from(u64::MAX)) as u64)
                .unwrap_or_default(),
            fingerprint,
            is_new,
        });
    }
    skills.sort_by(|left, right| {
        right
            .modified_ms
            .cmp(&left.modified_ms)
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(skills)
}

fn list_skill_repositories_impl() -> Result<ResearchOsState, String> {
    Ok(load_research_os_state())
}

#[tauri::command]
async fn list_skill_repositories() -> Result<ResearchOsState, String> {
    run_blocking(list_skill_repositories_impl).await
}

fn add_skill_repository_impl(source: String) -> Result<ResearchOsState, String> {
    let source = validate_skill_repository_source(&source)?;
    verify_system_git()?;
    let mut state = load_research_os_state();
    if state.repositories.iter().any(|item| item.source == source) {
        return Err("这个 Skill 仓库已经添加".to_string());
    }
    let repository_id = format!("repo-{}", unix_millis());
    let root = research_os_root()?;
    let destination = root.join("repositories").join(&repository_id);
    fs::create_dir_all(destination.parent().unwrap_or(&root))
        .map_err(|error| format!("无法创建 Skill 仓库目录：{error}"))?;
    git_clone_skill_repository(&source, &destination)?;
    let skills = scan_skill_repository(&repository_id, &destination, &[])?;
    let now = unix_millis().min(u128::from(u64::MAX)) as u64;
    state.repositories.push(SkillRepository {
        id: repository_id,
        source,
        local_path: destination.display().to_string(),
        created_at: now,
        last_synced_at: now,
        last_commit: git_current_commit(&destination)?,
        skills,
    });
    state
        .repositories
        .sort_by_key(|item| std::cmp::Reverse(item.last_synced_at));
    persist_research_os_state(&state)?;
    Ok(state)
}

#[tauri::command]
async fn add_skill_repository(source: String) -> Result<ResearchOsState, String> {
    run_blocking(move || add_skill_repository_impl(source)).await
}

fn sync_skill_repository_impl(repository_id: String) -> Result<ResearchOsState, String> {
    let repository_id = safe_skill_repository_id(&repository_id)?;
    verify_system_git()?;
    let mut state = load_research_os_state();
    let repository = state
        .repositories
        .iter_mut()
        .find(|item| item.id == repository_id)
        .ok_or_else(|| "没有找到这个 Skill 仓库".to_string())?;
    let destination = PathBuf::from(&repository.local_path);
    let expected_root = research_os_root()?.join("repositories");
    if !destination.starts_with(&expected_root) || !destination.join(".git").is_dir() {
        return Err("Skill 仓库本地目录无效，请重新添加仓库".to_string());
    }
    git_pull_skill_repository(&destination)?;
    repository.skills = scan_skill_repository(&repository.id, &destination, &repository.skills)?;
    repository.last_commit = git_current_commit(&destination)?;
    repository.last_synced_at = unix_millis().min(u128::from(u64::MAX)) as u64;
    state
        .repositories
        .sort_by_key(|item| std::cmp::Reverse(item.last_synced_at));
    persist_research_os_state(&state)?;
    Ok(state)
}

#[tauri::command]
async fn sync_skill_repository(repository_id: String) -> Result<ResearchOsState, String> {
    run_blocking(move || sync_skill_repository_impl(repository_id)).await
}

fn load_connect_settings() -> StoredConnectSettings {
    let Ok(path) = connect_settings_path() else {
        return StoredConnectSettings::default();
    };
    let Ok(content) = fs::read_to_string(path) else {
        return StoredConnectSettings::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn persist_connect_settings(settings: &StoredConnectSettings) -> Result<(), String> {
    let body = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("无法序列化 Connect 配置：{error}"))?;
    write_text_file_atomic(&connect_settings_path()?, &(body + "\n"))
}

fn connect_state(settings: &StoredConnectSettings) -> ConnectState {
    let telegram_detail = if settings.encrypted_telegram_bot_token.is_empty() {
        "Bot Token + Chat ID".to_string()
    } else {
        let suffix = settings
            .telegram_chat_id
            .chars()
            .rev()
            .take(4)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        format!("Bot Token 已加密 · Chat ID …{suffix}")
    };
    ConnectState {
        feishu: ConnectChannelSummary {
            id: "feishu".to_string(),
            configured: !settings.encrypted_feishu_webhook.is_empty(),
            detail: if settings.encrypted_feishu_webhook.is_empty() {
                "Incoming Webhook".to_string()
            } else {
                "Webhook 已使用 Windows 当前用户加密".to_string()
            },
            updated_at: settings.feishu_updated_at,
        },
        telegram: ConnectChannelSummary {
            id: "telegram".to_string(),
            configured: !settings.encrypted_telegram_bot_token.is_empty()
                && !settings.telegram_chat_id.is_empty(),
            detail: telegram_detail,
            updated_at: settings.telegram_updated_at,
        },
    }
}

fn validate_feishu_webhook(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() > 2_048 || value.contains(['\r', '\n', '\0']) {
        return Err("飞书 Webhook 地址无效".to_string());
    }
    let lower = value.to_ascii_lowercase();
    let valid = lower.starts_with("https://open.feishu.cn/open-apis/bot/v2/hook/")
        || lower.starts_with("https://open.larksuite.com/open-apis/bot/v2/hook/");
    if !valid || value.rsplit('/').next().unwrap_or("").len() < 8 {
        return Err("只接受飞书或 Lark 官方 Incoming Webhook HTTPS 地址".to_string());
    }
    Ok(value.to_string())
}

fn validate_telegram_bot_token(value: &str) -> Result<String, String> {
    let value = value.trim();
    let Some((bot_id, secret)) = value.split_once(':') else {
        return Err("Telegram Bot Token 格式无效".to_string());
    };
    if value.len() > 256
        || bot_id.is_empty()
        || !bot_id.chars().all(|ch| ch.is_ascii_digit())
        || secret.len() < 10
        || !secret
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err("Telegram Bot Token 格式无效".to_string());
    }
    Ok(value.to_string())
}

fn validate_telegram_chat_id(value: &str) -> Result<String, String> {
    let value = value.trim();
    let numeric = value
        .strip_prefix('-')
        .unwrap_or(value)
        .chars()
        .all(|ch| ch.is_ascii_digit());
    let username = value
        .strip_prefix('@')
        .map(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
        .unwrap_or(false);
    if value.is_empty() || value.len() > 128 || (!numeric && !username) {
        return Err("Telegram Chat ID 应为数字 ID 或 @channel_name".to_string());
    }
    Ok(value.to_string())
}

fn run_connect_powershell(script: &str, payload: &serde_json::Value) -> Result<String, String> {
    let input = serde_json::to_vec(payload)
        .map_err(|error| format!("无法序列化 Connect 测试请求：{error}"))?;
    let script = format!(
        "$utf8=New-Object System.Text.UTF8Encoding($false); [Console]::InputEncoding=$utf8; [Console]::OutputEncoding=$utf8; $OutputEncoding=$utf8; {script}"
    );
    let mut command = background_command("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    let output = command_output_with_input_timeout(
        command,
        &input,
        Duration::from_secs(20),
        "Connect 测试消息",
    )?;
    if !output.status.success() {
        return Err(command_error_text(&output));
    }
    Ok(output_text(&output))
}

fn get_connect_state_impl() -> Result<ConnectState, String> {
    Ok(connect_state(&load_connect_settings()))
}

#[tauri::command]
async fn get_connect_state() -> Result<ConnectState, String> {
    run_blocking(get_connect_state_impl).await
}

fn save_feishu_connection_impl(webhook_url: String) -> Result<ConnectState, String> {
    let webhook_url = validate_feishu_webhook(&webhook_url)?;
    let mut settings = load_connect_settings();
    settings.encrypted_feishu_webhook = protect_api_key(&webhook_url)?;
    settings.feishu_updated_at = unix_millis().min(u128::from(u64::MAX)) as u64;
    persist_connect_settings(&settings)?;
    Ok(connect_state(&settings))
}

#[tauri::command]
async fn save_feishu_connection(webhook_url: String) -> Result<ConnectState, String> {
    run_blocking(move || save_feishu_connection_impl(webhook_url)).await
}

fn save_telegram_connection_impl(
    bot_token: String,
    chat_id: String,
) -> Result<ConnectState, String> {
    let bot_token = validate_telegram_bot_token(&bot_token)?;
    let chat_id = validate_telegram_chat_id(&chat_id)?;
    let mut settings = load_connect_settings();
    settings.encrypted_telegram_bot_token = protect_api_key(&bot_token)?;
    settings.telegram_chat_id = chat_id;
    settings.telegram_updated_at = unix_millis().min(u128::from(u64::MAX)) as u64;
    persist_connect_settings(&settings)?;
    Ok(connect_state(&settings))
}

#[tauri::command]
async fn save_telegram_connection(
    bot_token: String,
    chat_id: String,
) -> Result<ConnectState, String> {
    run_blocking(move || save_telegram_connection_impl(bot_token, chat_id)).await
}

fn test_connect_channel_impl(channel: String) -> Result<ConnectTestResult, String> {
    let settings = load_connect_settings();
    let test_text = "CSA 连接测试成功。此通道当前只发送任务状态通知，不接收远程命令。";
    match channel.as_str() {
        "feishu" => {
            let webhook = unprotect_api_key(&settings.encrypted_feishu_webhook)?;
            if webhook.is_empty() {
                return Err("请先配置飞书 Webhook".to_string());
            }
            let payload = serde_json::json!({ "url": webhook, "text": test_text });
            let script = r#"$p=[Console]::In.ReadToEnd() | ConvertFrom-Json; $body=@{msg_type='text';content=@{text=$p.text}} | ConvertTo-Json -Depth 5 -Compress; $r=Invoke-RestMethod -Uri $p.url -Method Post -ContentType 'application/json; charset=utf-8' -Body $body -TimeoutSec 15; if($null -ne $r.code -and [int]$r.code -ne 0){throw ('Feishu API code '+$r.code)}; [Console]::Out.Write('ok')"#;
            run_connect_powershell(script, &payload)
                .map_err(|error| redact_secret_text(&error, &webhook))?;
            Ok(ConnectTestResult {
                ok: true,
                channel,
                message: "飞书测试消息已发送".to_string(),
            })
        }
        "telegram" => {
            let token = unprotect_api_key(&settings.encrypted_telegram_bot_token)?;
            if token.is_empty() || settings.telegram_chat_id.is_empty() {
                return Err("请先配置 Telegram Bot Token 和 Chat ID".to_string());
            }
            let payload = serde_json::json!({
                "token": token,
                "chatId": settings.telegram_chat_id,
                "text": test_text
            });
            let script = r#"$p=[Console]::In.ReadToEnd() | ConvertFrom-Json; $url='https://api.telegram.org/bot'+$p.token+'/sendMessage'; $body=@{chat_id=$p.chatId;text=$p.text} | ConvertTo-Json -Compress; $r=Invoke-RestMethod -Uri $url -Method Post -ContentType 'application/json; charset=utf-8' -Body $body -TimeoutSec 15; if(-not $r.ok){throw 'Telegram API rejected the message'}; [Console]::Out.Write('ok')"#;
            run_connect_powershell(script, &payload)
                .map_err(|error| redact_secret_text(&error, &token))?;
            Ok(ConnectTestResult {
                ok: true,
                channel,
                message: "Telegram 测试消息已发送".to_string(),
            })
        }
        _ => Err("未知 Connect 通道".to_string()),
    }
}

#[tauri::command]
async fn test_connect_channel(channel: String) -> Result<ConnectTestResult, String> {
    run_blocking(move || test_connect_channel_impl(channel)).await
}

fn clear_connect_channel_impl(channel: String) -> Result<ConnectState, String> {
    let mut settings = load_connect_settings();
    match channel.as_str() {
        "feishu" => {
            settings.encrypted_feishu_webhook.clear();
            settings.feishu_updated_at = 0;
        }
        "telegram" => {
            settings.encrypted_telegram_bot_token.clear();
            settings.telegram_chat_id.clear();
            settings.telegram_updated_at = 0;
        }
        _ => return Err("未知 Connect 通道".to_string()),
    }
    persist_connect_settings(&settings)?;
    Ok(connect_state(&settings))
}

#[tauri::command]
async fn clear_connect_channel(channel: String) -> Result<ConnectState, String> {
    run_blocking(move || clear_connect_channel_impl(channel)).await
}

fn create_demo_subagent_request_impl() -> Result<SubagentInboxItem, String> {
    let inbox = subagent_inbox_dir()?;
    fs::create_dir_all(&inbox).map_err(|error| format!("无法创建 Subagent 收件箱目录：{error}"))?;
    let request_id = format!("demo-{}", unix_millis());
    let request = serde_json::json!({
        "schemaVersion": 1,
        "source": "launcher-demo",
        "taskKind": "dataset",
        "title": "Demo: dataset download diagnosis",
        "cwd": project_root()?.display().to_string(),
        "note": "This is a read-only demo request. Diagnose what the host runner should check before downloading a dataset outside the sandbox.",
        "requestedAction": "diagnose",
        "approvalMode": "manual",
        "policyId": "manual-only",
        "createdAt": current_local_date()
    });
    let path = inbox.join(format!("{request_id}.json"));
    let body = serde_json::to_string_pretty(&request)
        .map_err(|error| format!("无法序列化 demo request：{error}"))?;
    write_text_file_atomic(&path, &(body + "\n"))?;
    subagent_item_from_path(&path).ok_or_else(|| "无法读取刚创建的 demo request".to_string())
}

#[tauri::command]
async fn create_demo_subagent_request() -> Result<SubagentInboxItem, String> {
    run_blocking(create_demo_subagent_request_impl).await
}

fn test_api_key_impl(
    selected_provider_id: String,
    api_key: String,
    custom_base_url: String,
    custom_confirmed: bool,
    model: String,
    prompt: String,
) -> Result<ApiKeyTestResult, String> {
    let clean_key = api_key.trim();
    if clean_key.is_empty() {
        return Err("请先填写 API Key，再测试连通。".into());
    }
    let provider =
        provider_by_id(&selected_provider_id).ok_or_else(|| "未知 API Key 服务商".to_string())?;
    if provider.trust.starts_with("untrusted") && !custom_confirmed {
        return Err("中转服务需要先确认域名后再测试，避免 API Key 发到错误地址。".into());
    }
    let Some(profile) =
        runtime_profile_for_provider(&selected_provider_id, &custom_base_url, custom_confirmed)?
    else {
        return Err("Claude 官方登录模式不需要在这里测试 API Key。".into());
    };
    let clean_prompt = {
        let value = prompt.trim();
        if value.is_empty() {
            "Reply only: OK"
        } else {
            value
        }
    };
    let payload = serde_json::json!({
        "provider_id": profile.provider_id,
        "api_key": clean_key,
        "base_url": profile.base_url,
        "upstream_mode": profile.upstream_mode,
        "model": model.trim(),
        "default_model": profile.default_model,
        "documented_models": documented_models_for_profile(&profile),
        "prompt": clean_prompt,
        "initial_max_tokens": API_KEY_TEST_INITIAL_MAX_TOKENS,
        "retry_max_tokens": API_KEY_TEST_RETRY_MAX_TOKENS
    });
    let script = r#"
$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13
$req = [Console]::In.ReadToEnd() | ConvertFrom-Json
$providerId = [string]$req.provider_id
$apiKey = [string]$req.api_key
$baseUrl = ([string]$req.base_url).Trim().TrimEnd("/")
$upstreamMode = ([string]$req.upstream_mode).Trim().ToLowerInvariant()
$requestedModel = ([string]$req.model).Trim()
$defaultModel = ([string]$req.default_model).Trim()
$documentedModels = @($req.documented_models | ForEach-Object { ([string]$_).Trim() } | Where-Object { $_ })
$prompt = ([string]$req.prompt).Trim()
$initialMaxTokens = [int]$req.initial_max_tokens
$retryMaxTokens = [int]$req.retry_max_tokens
if ($initialMaxTokens -lt 64) { $initialMaxTokens = 64 }
if ($retryMaxTokens -lt $initialMaxTokens) { $retryMaxTokens = $initialMaxTokens }
$testBudgets = @($initialMaxTokens)
if ($retryMaxTokens -gt $initialMaxTokens) { $testBudgets += $retryMaxTokens }
if (-not $prompt) { $prompt = "Reply only: OK" }

function Redact([string]$text) {
  if (-not $text) { return "" }
  if ($apiKey) { $text = $text.Replace($apiKey, "[redacted-api-key]") }
  if ($text.Length -gt 900) { return $text.Substring(0, 900) }
  return $text
}

function NormalizeOpenAIBase([string]$base) {
  $b = $base.TrimEnd("/")
  if ($b.EndsWith("/v1") -or $b.EndsWith("/v4")) { return $b }
  return "$b/v1"
}

function NormalizeAnthropicBase([string]$base) {
  $b = $base.TrimEnd("/")
  if ($b.EndsWith("/v1")) { return $b }
  if ($b.EndsWith("/anthropic")) { return "$b/v1" }
  if ($b.Contains("api.deepseek.com") -and -not $b.Contains("/anthropic")) { return "$b/anthropic/v1" }
  return "$b/v1"
}

function ShortError($err) {
  try {
    if ($err.Exception.Response) {
      $stream = $err.Exception.Response.GetResponseStream()
      if ($stream) {
        $reader = New-Object IO.StreamReader($stream)
        $body = $reader.ReadToEnd()
        return (Redact $body)
      }
    }
  } catch {}
  return (Redact ([string]$err.Exception.Message))
}

function Emit($ok, $selectedModel, $reply, $models, $message) {
  [ordered]@{
    ok = [bool]$ok
    providerId = $providerId
    baseUrl = $baseUrl
    upstreamMode = $upstreamMode
    selectedModel = [string]$selectedModel
    reply = [string](Redact $reply)
    models = @($models | Select-Object -First 30)
    message = [string](Redact $message)
  } | ConvertTo-Json -Depth 8 -Compress
  exit 0
}

if (-not $baseUrl.StartsWith("https://")) {
  Emit $false "" "" @() "Base URL 必须是 https:// 地址。"
}

$models = @()
if ($upstreamMode -eq "anthropic") {
  $anthropicBase = NormalizeAnthropicBase $baseUrl
  $headers = @{
    "x-api-key" = $apiKey
    "anthropic-version" = "2023-06-01"
    "content-type" = "application/json"
  }
  try {
    $modelResp = Invoke-RestMethod -Method Get -Uri "$anthropicBase/models" -Headers $headers -TimeoutSec 12
    $models = @($modelResp.data | ForEach-Object { $_.id } | Where-Object { $_ })
  } catch {}
  $usedDocumentedModels = $false
  if ($models.Count -eq 0 -and $documentedModels.Count -gt 0) {
    $models = @($documentedModels)
    $usedDocumentedModels = $true
  }
  $candidates = New-Object System.Collections.Generic.List[string]
  foreach ($item in @($requestedModel, $defaultModel)) {
    if ($item -and -not $candidates.Contains($item)) { [void]$candidates.Add($item) }
  }
  foreach ($item in $models) {
    if ($item -and -not $candidates.Contains($item)) { [void]$candidates.Add($item) }
    if ($candidates.Count -ge 8) { break }
  }
  if ($candidates.Count -eq 0) {
    Emit $false "" "" $models "No manual model was provided and no testable model was returned by /models. Please fill a model ID or confirm the provider supports /models."
  }
  $lastError = ""
  foreach ($candidate in $candidates) {
    foreach ($budget in $testBudgets) {
      $body = @{
        model = $candidate
        max_tokens = $budget
        messages = @(@{ role = "user"; content = $prompt })
      } | ConvertTo-Json -Depth 8 -Compress
      try {
        $chat = Invoke-RestMethod -Method Post -Uri "$anthropicBase/messages" -Headers $headers -Body $body -TimeoutSec 45
        $reply = ""
        foreach ($part in @($chat.content)) {
          if ($part.type -eq "text") { $reply += [string]$part.text }
        }
        if ($reply -and $reply.Trim()) {
          $successMessage = if ($usedDocumentedModels) {
            "连接成功；模型列表接口不可用，本次使用官方文档已核验模型完成测试。"
          } else {
            "连接成功。"
          }
          Emit $true $candidate $reply $models $successMessage
        }
        $lastError = "HTTP 200，但模型在 $budget tokens 内没有返回正文：" + $candidate
        if ([string]$chat.stop_reason -ne "max_tokens") { break }
      } catch {
        $lastError = ShortError $_
        break
      }
    }
  }
  Emit $false "" "" $models ("模型列表可访问，但对话测试失败：" + $lastError)
}

$openaiBase = NormalizeOpenAIBase $baseUrl
$headers = @{
  "Authorization" = "Bearer $apiKey"
  "Content-Type" = "application/json"
}
try {
  $modelResp = Invoke-RestMethod -Method Get -Uri "$openaiBase/models" -Headers $headers -TimeoutSec 12
  $models = @($modelResp.data | ForEach-Object { $_.id } | Where-Object { $_ })
} catch {
  $last = ShortError $_
  if (-not $requestedModel -and -not $defaultModel -and $documentedModels.Count -eq 0) {
    Emit $false "" "" @() ("无法读取模型列表：" + $last)
  }
}

$usedDocumentedModels = $false
if ($models.Count -eq 0 -and $documentedModels.Count -gt 0) {
  $models = @($documentedModels)
  $usedDocumentedModels = $true
}

if ($models.Count -eq 0 -and -not $requestedModel -and -not $defaultModel) {
  Emit $false "" "" @() "No manual model was provided and no testable model was returned by /models. Please fill a model ID or confirm the provider supports /models."
}

$candidates = New-Object System.Collections.Generic.List[string]
foreach ($item in @($requestedModel, $defaultModel)) {
  if ($item -and -not $candidates.Contains($item)) {
    [void]$candidates.Add($item)
  }
}
foreach ($item in $models) {
  if ($item -and -not $candidates.Contains($item)) { [void]$candidates.Add($item) }
  if ($candidates.Count -ge 10) { break }
}

$lastError = ""
foreach ($candidate in $candidates) {
  foreach ($budget in $testBudgets) {
    $body = @{
      model = $candidate
      messages = @(@{ role = "user"; content = $prompt })
      max_tokens = $budget
      temperature = 0
      stream = $false
    } | ConvertTo-Json -Depth 8 -Compress
    try {
      $chat = Invoke-RestMethod -Method Post -Uri "$openaiBase/chat/completions" -Headers $headers -Body $body -TimeoutSec 45
      $reply = [string]$chat.choices[0].message.content
    if ($reply -and $reply.Trim()) {
      $successMessage = if ($usedDocumentedModels) {
        "连接成功；模型列表接口不可用，本次使用官方文档已核验模型完成测试。"
      } else {
        "连接成功。"
      }
      Emit $true $candidate $reply $models $successMessage
      }
      $reasoning = [string]$chat.choices[0].message.reasoning_content
      $finishReason = [string]$chat.choices[0].finish_reason
      $lastError = "HTTP 200，但模型在 $budget tokens 内没有返回正文：" + $candidate
      if ($finishReason -ne "length" -and -not $reasoning) { break }
    } catch {
      $lastError = ShortError $_
      break
    }
  }
}

Emit $false "" "" $models ("模型列表可访问，但没有找到可完成对话的模型：" + $lastError)
"#;
    let input = serde_json::to_string(&payload)
        .map_err(|error| format!("无法准备 API Key 测试请求：{error}"))?;
    let output = run_powershell_with_stdin(script, &input)?;
    let output = redact_secret_text(&output, clean_key);
    let mut result: ApiKeyTestResult = serde_json::from_str(&output)
        .map_err(|error| format!("API Key 测试结果解析失败：{error}; {output}"))?;
    result.message = redact_secret_text(&result.message, clean_key);
    result.reply = redact_secret_text(&result.reply, clean_key);
    Ok(result)
}

#[tauri::command]
async fn test_api_key(
    selected_provider_id: String,
    api_key: String,
    custom_base_url: String,
    custom_confirmed: bool,
    model: String,
    prompt: String,
) -> Result<ApiKeyTestResult, String> {
    run_blocking(move || {
        test_api_key_impl(
            selected_provider_id,
            api_key,
            custom_base_url,
            custom_confirmed,
            model,
            prompt,
        )
    })
    .await
}

fn auto_map_api_key_impl(
    selected_provider_id: String,
    api_key: String,
    custom_base_url: String,
    custom_confirmed: bool,
    model: String,
) -> Result<ApiKeyAutoMapResult, String> {
    let clean_key = api_key.trim();
    if clean_key.is_empty() {
        return Err("请先填写 API Key，再自动映射模型。".into());
    }
    let provider =
        provider_by_id(&selected_provider_id).ok_or_else(|| "未知 API Key 服务商".to_string())?;
    if provider.trust.starts_with("untrusted") && !custom_confirmed {
        return Err("中转服务需要先确认域名后再自动映射，避免 API Key 发到错误地址。".into());
    }
    let Some(profile) =
        runtime_profile_for_provider(&selected_provider_id, &custom_base_url, custom_confirmed)?
    else {
        return Err("Claude 官方登录模式不需要在这里自动映射模型。".into());
    };
    let fallback_model = if model.trim().is_empty() {
        String::new()
    } else {
        canonical_model_for_profile(&profile, &model)
    };
    let payload = serde_json::json!({
        "provider_id": profile.provider_id.clone(),
        "api_key": clean_key,
        "base_url": profile.base_url.clone(),
        "upstream_mode": profile.upstream_mode,
        "documented_models": documented_models_for_profile(&profile),
    });
    let script = r#"
$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13
$req = [Console]::In.ReadToEnd() | ConvertFrom-Json
$apiKey = [string]$req.api_key
$baseUrl = ([string]$req.base_url).Trim().TrimEnd("/")
$upstreamMode = ([string]$req.upstream_mode).Trim().ToLowerInvariant()
$documentedModels = @($req.documented_models | ForEach-Object { ([string]$_).Trim() } | Where-Object { $_ })

function Redact([string]$text) {
  if (-not $text) { return "" }
  if ($apiKey) { $text = $text.Replace($apiKey, "[redacted-api-key]") }
  if ($text.Length -gt 900) { return $text.Substring(0, 900) }
  return $text
}

function NormalizeOpenAIBase([string]$base) {
  $b = $base.TrimEnd("/")
  if ($b.EndsWith("/v1") -or $b.EndsWith("/v4")) { return $b }
  return "$b/v1"
}

function NormalizeAnthropicBase([string]$base) {
  $b = $base.TrimEnd("/")
  if ($b.EndsWith("/v1")) { return $b }
  if ($b.EndsWith("/anthropic")) { return "$b/v1" }
  if ($b.Contains("api.deepseek.com") -and -not $b.Contains("/anthropic")) { return "$b/anthropic/v1" }
  return "$b/v1"
}

function ShortError($err) {
  try {
    if ($err.Exception.Response) {
      $stream = $err.Exception.Response.GetResponseStream()
      if ($stream) {
        $reader = New-Object IO.StreamReader($stream)
        $body = $reader.ReadToEnd()
        return (Redact $body)
      }
    }
  } catch {}
  return (Redact ([string]$err.Exception.Message))
}

function Emit($models, [string]$message) {
  [ordered]@{
    models = @($models | Where-Object { $_ } | Select-Object -First 500)
    message = [string](Redact $message)
  } | ConvertTo-Json -Depth 8 -Compress
  exit 0
}

if (-not $baseUrl.StartsWith("https://")) {
  Emit @() "Base URL 必须是 https:// 地址。"
}

if ($upstreamMode -eq "anthropic") {
  $anthropicBase = NormalizeAnthropicBase $baseUrl
  $headers = @{
    "x-api-key" = $apiKey
    "anthropic-version" = "2023-06-01"
    "content-type" = "application/json"
  }
  try {
    $modelResp = Invoke-RestMethod -Method Get -Uri "$anthropicBase/models" -Headers $headers -TimeoutSec 12
    $models = @($modelResp.data | ForEach-Object { $_.id } | Where-Object { $_ })
    if ($models.Count -gt 0) { Emit $models "模型列表读取成功。" }
    if ($documentedModels.Count -gt 0) {
      Emit $documentedModels "模型列表接口未返回模型，已使用官方文档已核验模型。"
    }
    Emit @() "模型列表接口未返回模型；请手动填写模型 ID。"
  } catch {
    if ($documentedModels.Count -gt 0) {
      Emit $documentedModels "模型列表接口不可用，已使用官方文档已核验模型。"
    }
    Emit @() (ShortError $_)
  }
}

$openaiBase = NormalizeOpenAIBase $baseUrl
$headers = @{
  "Authorization" = "Bearer $apiKey"
  "Content-Type" = "application/json"
}
try {
  $modelResp = Invoke-RestMethod -Method Get -Uri "$openaiBase/models" -Headers $headers -TimeoutSec 12
  $models = @($modelResp.data | ForEach-Object { $_.id } | Where-Object { $_ })
  if ($models.Count -gt 0) { Emit $models "模型列表读取成功。" }
  if ($documentedModels.Count -gt 0) {
    Emit $documentedModels "模型列表接口未返回模型，已使用官方文档已核验模型。"
  }
  Emit @() "模型列表接口未返回模型；请手动填写模型 ID。"
} catch {
  if ($documentedModels.Count -gt 0) {
    Emit $documentedModels "模型列表接口不可用，已使用官方文档已核验模型。"
  }
  Emit @() (ShortError $_)
}
"#;
    let input = serde_json::to_string(&payload)
        .map_err(|error| format!("无法准备自动映射请求：{error}"))?;
    let output = run_powershell_with_stdin(script, &input)?;
    let output = redact_secret_text(&output, clean_key);
    let fetch: ModelListFetchResult = serde_json::from_str(&output)
        .map_err(|error| format!("自动映射结果解析失败：{error}; {output}"))?;
    let fallback_fast_model = canonical_model_for_profile(&profile, &profile.default_fast_model);
    let mut mapping_models = fetch.models.clone();
    if mapping_models.is_empty() && !fallback_model.is_empty() {
        mapping_models.push(fallback_model.clone());
        if !fallback_fast_model.is_empty() && fallback_fast_model != fallback_model {
            mapping_models.push(fallback_fast_model);
        }
    }
    let (mapping_models, mapping_fallback_model) =
        auto_mapping_inputs_for_profile(&profile, &mapping_models, &fallback_model);
    let (primary_model, fast_model, aliases, candidates) =
        auto_model_mapping(&mapping_models, &mapping_fallback_model)?;
    let message = if fetch.models.is_empty() {
        format!("未读取到模型列表，已基于默认/手动模型生成映射：{primary_model}")
    } else if primary_model == fast_model {
        format!(
            "读取到 {} 个模型；未发现明显快速模型，Claude 角色统一映射到：{}",
            fetch.models.len(),
            primary_model
        )
    } else {
        format!(
            "读取到 {} 个模型；主力映射到 {}，快速/Haiku 映射到 {}。",
            fetch.models.len(),
            primary_model,
            fast_model
        )
    };
    let mut preview_models = candidates;
    preview_models.truncate(50);
    Ok(ApiKeyAutoMapResult {
        ok: true,
        provider_id: profile.provider_id,
        base_url: profile.base_url,
        upstream_mode: profile.upstream_mode.into(),
        primary_model,
        fast_model,
        aliases,
        models: preview_models,
        message: if fetch.message.trim().is_empty() {
            message
        } else {
            format!("{message}（{}）", fetch.message.trim())
        },
    })
}

#[tauri::command]
async fn auto_map_api_key(
    selected_provider_id: String,
    api_key: String,
    custom_base_url: String,
    custom_confirmed: bool,
    model: String,
) -> Result<ApiKeyAutoMapResult, String> {
    run_blocking(move || {
        auto_map_api_key_impl(
            selected_provider_id,
            api_key,
            custom_base_url,
            custom_confirmed,
            model,
        )
    })
    .await
}

fn launcher_settings_body(settings: &LauncherSettings) -> Result<String, String> {
    let body = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("无法序列化配置：{error}"))?;
    Ok(body + "\n")
}

fn prepare_launcher_settings(settings: &LauncherSettings) -> Result<PreparedAtomicWrite, String> {
    prepare_atomic_write(&settings_path()?, &launcher_settings_body(settings)?)
}

fn persist_launcher_settings(settings: &LauncherSettings) -> Result<(), String> {
    prepare_launcher_settings(settings)?.commit()
}

fn commit_launcher_settings_with_bridge(
    settings: &LauncherSettings,
    patch: Option<serde_json::Value>,
) -> Result<(), String> {
    // Pre-write and fsync Windows settings before touching WSL. This catches a
    // full APPDATA drive without leaving Bridge on a different active Key.
    let prepared_settings = prepare_launcher_settings(settings)?;
    let applied_bridge = match patch {
        Some(patch) => Some(apply_bridge_config_patch_value(patch)?),
        None => None,
    };
    if let Err(settings_error) = prepared_settings.commit() {
        let rollback_message = match applied_bridge.as_ref() {
            Some(applied) => match rollback_applied_bridge(applied) {
                Ok(()) => "Bridge 已恢复到切换前配置".to_string(),
                Err(error) => format!("Bridge 回滚失败：{error}"),
            },
            None => "Bridge 未发生改动".to_string(),
        };
        return Err(format!(
            "Windows 启动器配置提交失败：{settings_error}；{rollback_message}"
        ));
    }
    Ok(())
}

#[tauri::command]
fn get_provider_catalog() -> Vec<ProviderCatalogGroup> {
    provider_catalog()
}

#[tauri::command]
fn get_launcher_settings() -> LauncherState {
    launcher_state(&load_settings())
}

fn save_provider_selection_impl(
    selected_provider_id: String,
    custom_base_url: String,
    custom_confirmed: bool,
) -> Result<LauncherState, String> {
    if !provider_exists(&selected_provider_id) {
        return Err("未知 Provider".into());
    }
    if selected_provider_id == "custom" && custom_confirmed && custom_base_url.trim().is_empty() {
        return Err("确认自定义中转前，请先填写 Base URL".into());
    }
    let mut settings = load_settings();
    settings.selected_provider_id = selected_provider_id;
    settings.custom_base_url = validate_base_url(&custom_base_url)?;
    settings.custom_confirmed = custom_confirmed;
    settings.active_api_key_id = None;
    let patch = bridge_config_patch_for_provider(&settings)?;
    commit_launcher_settings_with_bridge(&settings, patch)?;
    Ok(launcher_state(&settings))
}

#[tauri::command]
async fn save_provider_selection(
    selected_provider_id: String,
    custom_base_url: String,
    custom_confirmed: bool,
) -> Result<LauncherState, String> {
    run_blocking(move || {
        save_provider_selection_impl(selected_provider_id, custom_base_url, custom_confirmed)
    })
    .await
}

fn save_api_key_impl(
    selected_provider_id: String,
    api_key: String,
    display_name: String,
    custom_base_url: String,
    custom_confirmed: bool,
    model: String,
    model_aliases: Vec<StoredModelAlias>,
) -> Result<LauncherState, String> {
    let provider =
        provider_by_id(&selected_provider_id).ok_or_else(|| "未知 API Key 服务商".to_string())?;
    if provider.trust.starts_with("untrusted") && !custom_confirmed {
        return Err("中转服务需要确认域名后才能保存 API Key".into());
    }
    if selected_provider_id == "custom" && custom_base_url.trim().is_empty() {
        return Err("确认自定义中转前，请先填写 Base URL".into());
    }
    let clean_key = api_key.trim();
    if selected_provider_id != "claude" && clean_key.is_empty() {
        return Err("添加供应商时请填写 API Key；已保存的 Key 可直接从列表切换".into());
    }
    let validated_base_url = validate_base_url(&custom_base_url)?;
    let encrypted_api_key = protect_api_key(clean_key)?;
    let mut settings = load_settings();
    settings.selected_provider_id = selected_provider_id.clone();
    settings.custom_base_url = validated_base_url.clone();
    settings.custom_confirmed = custom_confirmed;
    let runtime_profile = runtime_profile_for_settings(&settings)?;
    let mut sanitized_aliases = clean_model_aliases(&model_aliases);
    let stored_model = if model.trim().is_empty() {
        primary_model_from_aliases(&sanitized_aliases).unwrap_or_default()
    } else {
        model.trim().to_string()
    };
    let stored_model = runtime_profile
        .as_ref()
        .map(|profile| canonical_model_for_profile(profile, &stored_model))
        .unwrap_or(stored_model);
    if sanitized_aliases.is_empty() {
        if let Some(profile) = runtime_profile.as_ref() {
            sanitized_aliases = default_aliases_for_profile(profile, &stored_model);
        }
    }
    let patch =
        bridge_config_patch_for_api_key(&settings, clean_key, &stored_model, &sanitized_aliases)?;
    let label = if provider.trust.starts_with("untrusted") {
        provider_entry_label(&settings, &provider, &display_name)?
    } else {
        provider.name
    };
    let entry = StoredApiKey {
        id: next_api_key_id(),
        provider_id: selected_provider_id,
        label,
        base_url: validated_base_url,
        model: stored_model,
        custom_confirmed,
        model_aliases: sanitized_aliases,
        encrypted_api_key,
    };
    settings.active_api_key_id = Some(entry.id.clone());
    settings.api_keys.push(entry);
    commit_launcher_settings_with_bridge(&settings, patch)?;
    Ok(launcher_state(&settings))
}

#[tauri::command]
async fn save_api_key(
    selected_provider_id: String,
    api_key: String,
    display_name: String,
    custom_base_url: String,
    custom_confirmed: bool,
    model: String,
    model_aliases: Vec<StoredModelAlias>,
) -> Result<LauncherState, String> {
    run_blocking(move || {
        save_api_key_impl(
            selected_provider_id,
            api_key,
            display_name,
            custom_base_url,
            custom_confirmed,
            model,
            model_aliases,
        )
    })
    .await
}

fn activate_api_key_impl(api_key_id: String) -> Result<LauncherState, String> {
    let mut settings = load_settings();
    let entry = settings
        .api_keys
        .iter()
        .find(|item| item.id == api_key_id)
        .cloned()
        .ok_or_else(|| "没有找到这条 API Key".to_string())?;
    if entry.provider_id != "claude" && entry.encrypted_api_key.is_empty() {
        return Err("这条旧配置没有可切换的加密 Key，请重新添加该 API Key".into());
    }
    let api_key = unprotect_api_key(&entry.encrypted_api_key)?;
    settings.selected_provider_id = entry.provider_id.clone();
    settings.custom_base_url = entry.base_url.clone();
    settings.custom_confirmed = entry.custom_confirmed;
    let patch =
        bridge_config_patch_for_api_key(&settings, &api_key, &entry.model, &entry.model_aliases)?;
    settings.active_api_key_id = Some(entry.id);
    commit_launcher_settings_with_bridge(&settings, patch)?;
    Ok(launcher_state(&settings))
}

#[tauri::command]
async fn activate_api_key(api_key_id: String) -> Result<LauncherState, String> {
    run_blocking(move || activate_api_key_impl(api_key_id)).await
}

fn delete_api_key_impl(api_key_id: String) -> Result<LauncherState, String> {
    let mut settings = load_settings();
    if settings.active_api_key_id.as_deref() == Some(api_key_id.as_str()) {
        return Err("当前正在使用的 API Key 不能直接删除；请先切换到另一条 Key".into());
    }
    let before = settings.api_keys.len();
    settings.api_keys.retain(|entry| entry.id != api_key_id);
    if settings.api_keys.len() == before {
        return Err("没有找到这条 API Key".into());
    }
    persist_launcher_settings(&settings)?;
    Ok(launcher_state(&settings))
}

#[tauri::command]
async fn delete_api_key(api_key_id: String) -> Result<LauncherState, String> {
    run_blocking(move || delete_api_key_impl(api_key_id)).await
}

#[tauri::command]
async fn get_system_status() -> Result<SystemStatus, String> {
    run_blocking(|| Ok(current_status())).await
}

fn start_services_raw(distro: &str, user: &str) -> Result<(), String> {
    let script = project_root()?
        .join("scripts")
        .join("start-claude-science-wsl.ps1");
    let mut command = background_command("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .arg("-Distro")
        .arg(distro)
        .arg("-User")
        .arg(user);
    let output =
        command_output_with_timeout(command, Duration::from_secs(45), "Claude Science 启动")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Claude Science 启动失败：{}",
            command_error_text(&output)
        ))
    }
}

fn start_services_impl() -> Result<SystemStatus, String> {
    let before = current_status();
    if before.state == "running" {
        return Ok(before);
    }
    if before.restart_blocked {
        return Err(format!(
            "当前诊断不允许自动启动（{}）。请先检查磁盘空间、WSL 状态和安装包完整性；CSA 不会冒险修改环境。",
            before
                .wsl_storage_path
                .as_deref()
                .unwrap_or("WSL 虚拟磁盘位置未知")
        ));
    }
    let distro = before
        .distro
        .ok_or_else(|| "请先安装 WSL2 和 Ubuntu".to_string())?;
    let user = before
        .linux_user
        .ok_or_else(|| "无法确定 WSL 默认用户".to_string())?;
    start_services_raw(&distro, &user)?;
    Ok(current_status())
}

#[tauri::command]
async fn start_services() -> Result<SystemStatus, String> {
    run_blocking(start_services_impl).await
}

fn stop_services_raw(distro: &str) -> Result<(), String> {
    let script = r#"
systemctl --user stop claude-science-bridge.service >/dev/null 2>&1 || true
for pid in $(ps -eo pid=,args= | awk '/\/patched\/claude-science serve/ && !/awk/ {print $1}'); do
  case "$pid" in ''|*[!0-9]*) continue;; esac
  kill "$pid" 2>/dev/null || true
done
for pid in $(ss -ltnp "sport = :9876" 2>/dev/null | grep -o 'pid=[0-9]*' | cut -d= -f2 | sort -u); do
  case "$pid" in ''|*[!0-9]*) continue;; esac
  if [ -r "/proc/$pid/cmdline" ]; then
    cmd="$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
    case "$cmd" in *"/proxy.py"*) kill "$pid" 2>/dev/null || true;; esac
  fi
done
deadline=$(($(date +%s) + 5))
while [ "$(date +%s)" -lt "$deadline" ]; do
  if ! curl -fsS --connect-timeout 0.3 --max-time 0.6 http://127.0.0.1:9876/health >/dev/null 2>&1 \
    && ! ss -ltn 2>/dev/null | grep -qE ':(8765|8766) '; then
    exit 0
  fi
  sleep 0.25
done
echo "CSA services did not stop within 5 seconds." >&2
exit 1
"#;
    let mut command = background_command("wsl.exe");
    command
        .arg("--distribution")
        .arg(distro)
        .arg("--")
        .arg("sh")
        .arg("-s");
    let output = command_output_with_input_timeout(
        command,
        script.as_bytes(),
        Duration::from_secs(10),
        "停止 CSA 服务",
    )?;
    if !output.status.success() {
        return Err(format!("停止服务失败：{}", command_error_text(&output)));
    }
    Ok(())
}

fn stop_services_impl() -> Result<SystemStatus, String> {
    let before = current_status();
    let Some(distro) = before.distro else {
        return Ok(before);
    };
    stop_services_raw(&distro)?;
    Ok(current_status())
}

#[tauri::command]
async fn stop_services() -> Result<SystemStatus, String> {
    run_blocking(stop_services_impl).await
}

fn restart_services_impl() -> Result<SystemStatus, String> {
    let before = current_status();
    if before.restart_blocked {
        return Err("当前诊断不允许自动重启；可能是磁盘空间不足、WSL 只读/无响应或安装包不完整。现有服务不会被停止。".into());
    }
    let distro = before
        .distro
        .ok_or_else(|| "请先安装 WSL2 和 Ubuntu".to_string())?;
    let user = before
        .linux_user
        .ok_or_else(|| "无法确定 WSL 默认用户".to_string())?;
    stop_services_raw(&distro)?;
    start_services_raw(&distro, &user)?;
    Ok(current_status())
}

#[tauri::command]
async fn restart_services() -> Result<SystemStatus, String> {
    run_blocking(restart_services_impl).await
}

fn selected_distro_quick() -> Result<String, String> {
    let distros = discover_distros()?;
    preferred_distro(&distros).ok_or_else(|| "WSL 不可用".to_string())
}

fn get_claude_url_impl() -> Result<String, String> {
    let distro = selected_distro_quick()?;
    let output = wsl_shell(
        &distro,
        "$HOME/.local/share/claude-science-api-bridge/patched/claude-science url",
    )?;
    if !output.status.success() {
        return Err("无法获取 Claude Science 地址，请先启动服务".into());
    }
    output_text(&output)
        .lines()
        .find(|line| line.starts_with("http://") || line.starts_with("https://"))
        .map(ToOwned::to_owned)
        .ok_or_else(|| "Claude Science 未返回可打开的地址".to_string())
}

#[tauri::command]
async fn get_claude_url() -> Result<String, String> {
    run_blocking(get_claude_url_impl).await
}

fn get_dashboard_url_impl() -> Result<String, String> {
    let distro = selected_distro_quick()?;
    let project_wsl = project_root()
        .ok()
        .and_then(|root| windows_path_to_wsl(&distro, &root))
        .ok_or_else(|| "无法定位当前 CSA Bridge".to_string())?;
    let health = run_wsl(
        &distro,
        &[
            "curl",
            "-fsS",
            "--connect-timeout",
            "0.4",
            "--max-time",
            "2",
            "http://127.0.0.1:9876/health",
        ],
    )?;
    let health = serde_json::from_str::<serde_json::Value>(&output_text(&health))
        .map_err(|_| "当前 Bridge 健康信息无效，请先从本目录启动/迁移 Bridge".to_string())?;
    let expected_source = format!("{}/proxy.py", project_wsl.trim_end_matches('/'));
    let actual_source = health
        .get("source_path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if actual_source.replace('\\', "/") != expected_source.replace('\\', "/") {
        return Err("9876 端口不是当前 CSA 目录的 Bridge；请先启动当前包完成迁移".into());
    }
    let script = r#"
import json
import pathlib

path = pathlib.Path.home() / ".claude-science" / "proxy" / "config.json"
try:
    data = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}
    if not isinstance(data, dict):
        data = {}
except Exception:
    data = {}
print(json.dumps(data, ensure_ascii=False))
"#;
    let output = run_wsl(&distro, &["python3", "-c", script])?;
    if !output.status.success() {
        return Err("无法读取 Bridge 配置，无法打开配置面板".into());
    }
    let text = output_text(&output);
    let data: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({}));
    Ok(dashboard_url_from_config(&data))
}

#[tauri::command]
async fn get_dashboard_url() -> Result<String, String> {
    run_blocking(get_dashboard_url_impl).await
}

fn stop_legacy_windows_bridge_impl() -> Result<SystemStatus, String> {
    let Some(pid) = legacy_windows_bridge_pid() else {
        return Ok(current_status());
    };
    let command = format!("Stop-Process -Id {pid} -Force -ErrorAction Stop");
    let mut process = background_command("powershell.exe");
    process.args(["-NoProfile", "-NonInteractive", "-Command", &command]);
    let output =
        command_output_with_timeout(process, Duration::from_secs(10), "停止旧 Windows Bridge")?;
    if !output.status.success() {
        return Err(format!(
            "无法停止旧 Windows Bridge：{}",
            command_error_text(&output)
        ));
    }
    Ok(current_status())
}

#[tauri::command]
async fn stop_legacy_windows_bridge() -> Result<SystemStatus, String> {
    run_blocking(stop_legacy_windows_bridge_impl).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

            let show = MenuItem::with_id(app, "show", "打开 CSA", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("CSA - Claude Science Assistant")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        let _ = connect::stop_connect_gateway_impl();
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(window) = tray.app_handle().get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            thread::spawn(connect::start_connect_gateway_if_configured);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_system_status,
            start_services,
            stop_services,
            restart_services,
            get_claude_url,
            get_dashboard_url,
            stop_legacy_windows_bridge,
            get_provider_catalog,
            get_launcher_settings,
            save_provider_selection,
            save_api_key,
            activate_api_key,
            test_api_key,
            auto_map_api_key,
            delete_api_key,
            run_external_agent_task,
            list_subagent_requests,
            list_subagent_run_history,
            launch_external_claude_session,
            read_claude_session_history,
            list_skill_repositories,
            add_skill_repository,
            sync_skill_repository,
            get_connect_state,
            save_feishu_connection,
            save_telegram_connection,
            test_connect_channel,
            clear_connect_channel,
            connect::get_connect_runtime_state,
            connect::save_feishu_bot,
            connect::start_feishu_registration,
            connect::poll_feishu_registration,
            connect::save_telegram_bot,
            connect::clear_connect_bot,
            connect::start_connect_gateway,
            connect::stop_connect_gateway,
            connect::generate_connect_pairing_code,
            connect::generate_browser_extension_pairing_code,
            connect::get_browser_extension_install_info,
            connect::clear_browser_extension_pairing,
            connect::list_connect_routes,
            connect::bind_connect_route,
            connect::send_connect_local_message,
            connect::dispatch_connect_pending,
            connect::list_connect_history,
            connect::clear_connect_history,
            connect::install_connect_skill,
            connect::get_connector_setup,
            run_subagent_request,
            continue_subagent_session,
            create_demo_subagent_request
        ])
        .run(tauri::generate_context!())
        .expect("error while running Claude Science Assistant");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_atomic_write_replaces_existing_file() {
        let root = std::env::temp_dir().join(format!(
            "csa-atomic-write-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let destination = root.join("settings.json");
        fs::write(&destination, "old").unwrap();
        prepare_atomic_write(&destination, "new\n")
            .unwrap()
            .commit()
            .unwrap();
        assert_eq!(fs::read_to_string(&destination).unwrap(), "new\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn decodes_wsl_utf16_output() {
        let encoded: Vec<u8> = "Ubuntu-24.04\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(decode_console_output(&encoded), "Ubuntu-24.04\r\n");
    }

    #[test]
    fn mixed_wsl_warning_does_not_hide_linux_error() {
        let mut encoded: Vec<u8> = "wsl: localhost proxy WSL NAT warning\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        encoded.extend_from_slice(b"sh: 2: Syntax error: word unexpected (expecting \"do\")\n");

        let decoded = decode_console_output(&encoded);
        assert!(decoded.contains("sh: 2: Syntax error"));
        assert_eq!(
            clean_diagnostic_text(&decoded),
            "sh: 2: Syntax error: word unexpected (expecting \"do\")"
        );
    }

    #[test]
    fn prefers_supported_ubuntu_and_ignores_order() {
        let distros = vec!["Debian".into(), "Ubuntu-24.04".into()];
        assert_eq!(preferred_distro(&distros).as_deref(), Some("Ubuntu-24.04"));
        let fallback = vec!["Debian".into(), "Ubuntu-22.04".into()];
        assert_eq!(preferred_distro(&fallback).as_deref(), Some("Ubuntu-22.04"));
    }

    #[test]
    fn parses_first_numeric_pid() {
        assert_eq!(parse_first_pid("\n94797\n94800\n"), Some(94797));
        assert_eq!(parse_first_pid(""), None);
    }

    #[test]
    fn finds_portable_project_root_from_nested_exe_dir() {
        let root = std::env::temp_dir().join(format!(
            "claude-science-assistant-root-test-{}",
            std::process::id()
        ));
        let nested = root.join("nested").join("bin");
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("proxy.py"), "").unwrap();
        fs::write(root.join("requirements.txt"), "").unwrap();
        fs::write(root.join("scripts").join("start-claude-science-wsl.sh"), "").unwrap();

        assert_eq!(
            find_project_root_from(&nested).as_deref(),
            Some(root.as_path())
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provider_catalog_contains_expected_order_and_untrusted_custom() {
        let catalog = provider_catalog();
        let official: Vec<_> = catalog[0].providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            official,
            vec!["glm", "longcat", "deepseek", "minimax", "claude", "openai"]
        );
        let glm = &catalog[0].providers[0];
        assert_eq!(
            glm.base_url.as_deref(),
            Some("https://open.bigmodel.cn/api/paas/v4")
        );
        assert_eq!(glm.default_model, None);
        let deepseek = &catalog[0].providers[2];
        assert_eq!(
            deepseek.base_url.as_deref(),
            Some("https://api.deepseek.com/anthropic")
        );
        assert_eq!(deepseek.default_model, None);
        let minimax = &catalog[0].providers[3];
        assert_eq!(
            minimax.base_url.as_deref(),
            Some("https://api.minimaxi.com/anthropic")
        );
        assert_eq!(minimax.badge, "官方");
        assert_eq!(minimax.default_model, None);
        let openai = &catalog[0].providers[5];
        assert_eq!(openai.default_model, None);
        let opencode_go = &catalog[1].providers[0];
        assert_eq!(opencode_go.default_model, None);
        let third_party: Vec<_> = catalog[2].providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(third_party, vec!["builtin-relay", "custom"]);
        let builtin = &catalog[2].providers[0];
        assert_eq!(builtin.base_url.as_deref(), Some("https://10521052.xyz/v1"));
        assert_eq!(builtin.trust, "untrusted-builtin");
        let custom = &catalog[2].providers[1];
        assert_eq!(custom.base_url, None);
        assert_eq!(custom.trust, "untrusted-custom");
    }

    #[test]
    fn custom_url_must_be_https() {
        assert!(validate_base_url("").is_ok());
        assert!(validate_base_url("https://10521052.xyz/v1").is_ok());
        assert!(validate_base_url("http://10521052.xyz/v1").is_err());
    }

    #[test]
    fn bridge_profile_maps_builtin_relay_to_untrusted_custom_backend() {
        let settings = LauncherSettings {
            selected_provider_id: "builtin-relay".into(),
            custom_base_url: String::new(),
            custom_confirmed: false,
            ..LauncherSettings::default()
        };
        let patch = bridge_config_patch_for_provider(&settings)
            .unwrap()
            .unwrap();
        assert_eq!(patch["default_backend"], "custom");
        assert_eq!(patch["custom_base_url"], "https://10521052.xyz/v1");
        assert_eq!(patch["custom_upstream_mode"], "openai");
    }

    #[test]
    fn api_key_patch_adds_key_only_when_user_supplies_one() {
        let settings = LauncherSettings {
            selected_provider_id: "opencode-go".into(),
            custom_base_url: String::new(),
            custom_confirmed: false,
            ..LauncherSettings::default()
        };
        let patch = bridge_config_patch_for_api_key(&settings, "test-key", "custom-model", &[])
            .unwrap()
            .unwrap();
        assert_eq!(patch["default_backend"], "custom");
        assert_eq!(patch["custom_api_key"], "test-key");
        assert_eq!(patch["force_model"], "custom-model");

        let patch_without_key = bridge_config_patch_for_provider(&settings)
            .unwrap()
            .unwrap();
        assert_eq!(patch_without_key["custom_api_key"], "");
        assert_eq!(patch_without_key["deepseek_api_key"], "");
        assert_eq!(patch_without_key["openai_api_key"], "");
    }

    #[test]
    fn dynamic_relay_key_requires_explicit_or_tested_model() {
        let settings = LauncherSettings {
            selected_provider_id: "builtin-relay".into(),
            custom_base_url: String::new(),
            custom_confirmed: true,
            ..LauncherSettings::default()
        };
        let error = bridge_config_patch_for_api_key(&settings, "test-key", "", &[]).unwrap_err();
        assert!(error.contains("模型 ID"));
    }

    #[test]
    fn active_runtime_profile_clears_stale_backend_keys() {
        let settings = LauncherSettings {
            selected_provider_id: "builtin-relay".into(),
            custom_base_url: String::new(),
            custom_confirmed: true,
            ..LauncherSettings::default()
        };
        let patch = bridge_config_patch_for_api_key(&settings, "relay-key", "step-router-v1", &[])
            .unwrap()
            .unwrap();
        assert_eq!(patch["default_backend"], "custom");
        assert_eq!(patch["custom_base_url"], "https://10521052.xyz/v1");
        assert_eq!(patch["custom_api_key"], "relay-key");
        assert_eq!(patch["deepseek_api_key"], "");
        assert_eq!(patch["openai_api_key"], "");
        assert_eq!(patch["force_model"], "step-router-v1");
        assert_eq!(patch["model_list_mode"], "aliases");
        assert_eq!(patch["model_aliases"][0]["model"], "step-router-v1");
    }

    #[test]
    fn deepseek_profile_preserves_official_models_and_only_repairs_known_typo() {
        let settings = LauncherSettings {
            selected_provider_id: "deepseek".into(),
            custom_base_url: String::new(),
            custom_confirmed: false,
            ..LauncherSettings::default()
        };
        let patch = bridge_config_patch_for_api_key(&settings, "deepseek-key", "Deep-chat", &[])
            .unwrap()
            .unwrap();
        assert_eq!(patch["default_backend"], "deepseek");
        assert_eq!(patch["force_model"], "deepseek-chat");
        let rows = patch["model_aliases"].as_array().unwrap();
        let haiku = rows
            .iter()
            .find(|row| row["id"] == "claude-haiku-4-5-20251001")
            .unwrap();
        assert_eq!(haiku["model"], "deepseek-chat");

        let stale_glm_patch =
            bridge_config_patch_for_api_key(&settings, "deepseek-key", "glm-5.2", &[])
                .unwrap()
                .unwrap();
        assert_eq!(stale_glm_patch["force_model"], "glm-5.2");

        let official_v4_patch =
            bridge_config_patch_for_api_key(&settings, "deepseek-key", "deepseek-v4-pro", &[])
                .unwrap()
                .unwrap();
        assert_eq!(official_v4_patch["force_model"], "deepseek-v4-pro");
    }

    #[test]
    fn minimax_china_profile_preserves_all_official_anthropic_model_ids() {
        let profile = runtime_profile_for_provider("minimax", "", false)
            .unwrap()
            .unwrap();
        assert_eq!(profile.base_url, "https://api.minimaxi.com/anthropic");
        assert_eq!(profile.upstream_mode, "anthropic");
        assert_eq!(profile.default_model, "");

        for expected in [
            "MiniMax-M3",
            "MiniMax-M2.7",
            "MiniMax-M2.7-highspeed",
            "MiniMax-M2.5",
            "MiniMax-M2.5-highspeed",
            "MiniMax-M2.1",
            "MiniMax-M2.1-highspeed",
            "MiniMax-M2",
        ] {
            assert_eq!(
                canonical_model_for_profile(&profile, &expected.to_lowercase()),
                expected
            );
        }
    }

    #[test]
    fn minimax_china_key_writes_anthropic_bridge_config_without_a_fixed_default() {
        let settings = LauncherSettings {
            selected_provider_id: "minimax".into(),
            custom_base_url: String::new(),
            custom_confirmed: false,
            ..LauncherSettings::default()
        };
        let empty_error =
            bridge_config_patch_for_api_key(&settings, "minimax-key", "", &[]).unwrap_err();
        assert!(empty_error.contains("模型 ID"));

        let patch = bridge_config_patch_for_api_key(&settings, "minimax-key", "MiniMax-M3", &[])
            .unwrap()
            .unwrap();
        assert_eq!(patch["default_backend"], "custom");
        assert_eq!(
            patch["custom_base_url"],
            "https://api.minimaxi.com/anthropic"
        );
        assert_eq!(patch["custom_upstream_mode"], "anthropic");
        assert_eq!(patch["force_model"], "MiniMax-M3");
        assert_eq!(patch["deepseek_api_key"], "");
        assert_eq!(patch["openai_api_key"], "");
    }

    #[test]
    fn minimax_live_models_map_m3_to_primary_and_highspeed_to_fast() {
        let live_models = vec![
            "MiniMax-M2.7".to_string(),
            "MiniMax-M2.7-highspeed".to_string(),
            "MiniMax-M3".to_string(),
        ];
        let (primary, fast, aliases, candidates) = auto_model_mapping(&live_models, "").unwrap();
        assert_eq!(primary, "MiniMax-M3");
        assert_eq!(fast, "MiniMax-M2.7-highspeed");
        assert_eq!(candidates, live_models);
        let haiku = aliases
            .iter()
            .find(|item| item.id == "claude-haiku-4-5-20251001")
            .unwrap();
        assert_eq!(haiku.model, "MiniMax-M2.7-highspeed");
    }

    #[test]
    fn documented_model_fallback_is_limited_to_verified_official_profiles() {
        let deepseek = runtime_profile_for_provider("deepseek", "", false)
            .unwrap()
            .unwrap();
        assert_eq!(
            documented_models_for_profile(&deepseek),
            vec!["deepseek-v4-pro", "deepseek-v4-flash"]
        );

        let minimax = runtime_profile_for_provider("minimax", "", false)
            .unwrap()
            .unwrap();
        assert_eq!(documented_models_for_profile(&minimax)[0], "MiniMax-M3");

        let custom = runtime_profile_for_provider("custom", "https://example.com/v1", true)
            .unwrap()
            .unwrap();
        assert!(documented_models_for_profile(&custom).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn powershell_json_output_is_forced_to_utf8() {
        let output = run_powershell_with_stdin(
            "[ordered]@{message='连接成功。'} | ConvertTo-Json -Compress",
            "",
        )
        .unwrap();
        assert_eq!(output, r#"{"message":"连接成功。"}"#);
    }

    #[test]
    fn opencode_go_requires_explicit_or_mapped_model() {
        let settings = LauncherSettings {
            selected_provider_id: "opencode-go".into(),
            custom_base_url: String::new(),
            custom_confirmed: false,
            ..LauncherSettings::default()
        };
        let error =
            bridge_config_patch_for_api_key(&settings, "opencode-key", "", &[]).unwrap_err();
        assert!(error.contains("模型 ID"));

        let prefixed =
            bridge_config_patch_for_api_key(&settings, "opencode-key", "opencode-go/glm-5.2", &[])
                .unwrap()
                .unwrap();
        assert_eq!(prefixed["force_model"], "glm-5.2");
    }

    #[test]
    fn auto_mapping_single_model_maps_all_roles_to_same_model() {
        let models = vec!["glm-5.2".to_string()];
        let (primary, fast, aliases, candidates) = auto_model_mapping(&models, "").unwrap();
        assert_eq!(primary, "glm-5.2");
        assert_eq!(fast, "glm-5.2");
        assert_eq!(candidates, vec!["glm-5.2"]);
        assert!(aliases.iter().all(|alias| alias.model == "glm-5.2"));
    }

    #[test]
    fn auto_mapping_prefers_pro_for_primary_and_fast_for_haiku() {
        let models = vec![
            "deepseek-fast".to_string(),
            "deepseek-pro".to_string(),
            "text-embedding-3-large".to_string(),
        ];
        let (primary, fast, aliases, candidates) = auto_model_mapping(&models, "").unwrap();
        assert_eq!(primary, "deepseek-pro");
        assert_eq!(fast, "deepseek-fast");
        assert!(!candidates.contains(&"text-embedding-3-large".to_string()));
        let haiku = aliases
            .iter()
            .find(|alias| alias.id == "claude-haiku-4-5-20251001")
            .unwrap();
        assert_eq!(haiku.model, "deepseek-fast");
    }

    #[test]
    fn auto_mapping_prefers_qwen37_max_before_deepseek_v4_pro() {
        let models = vec![
            "deepseek-v4-flash".to_string(),
            "deepseek-v4-pro".to_string(),
            "qwen3.7-max".to_string(),
        ];
        let (primary, fast, aliases, _) = auto_model_mapping(&models, "").unwrap();
        assert_eq!(primary, "qwen3.7-max");
        assert_eq!(fast, "deepseek-v4-flash");
        let haiku = aliases
            .iter()
            .find(|alias| alias.id == "claude-haiku-4-5-20251001")
            .unwrap();
        assert_eq!(haiku.model, "deepseek-v4-flash");
    }

    #[test]
    fn opencode_go_auto_mapping_uses_openai_compatible_go_models() {
        let profile = runtime_profile_for_provider("opencode-go", "", false)
            .unwrap()
            .unwrap();
        let models = vec![
            "qwen3.7-max".to_string(),
            "MiniMax-M3".to_string(),
            "deepseek-v4-pro".to_string(),
            "deepseek-v4-flash".to_string(),
        ];
        let (models, fallback) = auto_mapping_inputs_for_profile(&profile, &models, "qwen3.7-max");
        assert_eq!(
            models,
            vec![
                "qwen3.7-max".to_string(),
                "MiniMax-M3".to_string(),
                "deepseek-v4-pro".to_string(),
                "deepseek-v4-flash".to_string()
            ]
        );
        assert_eq!(fallback, "qwen3.7-max");

        let (primary, fast, aliases, candidates) = auto_model_mapping(&models, &fallback).unwrap();
        assert_eq!(primary, "qwen3.7-max");
        assert_eq!(fast, "deepseek-v4-flash");
        assert!(candidates.contains(&"qwen3.7-max".to_string()));
        assert!(candidates.contains(&"MiniMax-M3".to_string()));
        let haiku = aliases
            .iter()
            .find(|alias| alias.id == "claude-haiku-4-5-20251001")
            .unwrap();
        assert_eq!(haiku.model, "deepseek-v4-flash");
    }

    #[test]
    fn bridge_config_patch_uses_stored_model_aliases() {
        let settings = LauncherSettings {
            selected_provider_id: "builtin-relay".into(),
            custom_base_url: String::new(),
            custom_confirmed: true,
            ..LauncherSettings::default()
        };
        let aliases = default_model_aliases("deepseek-pro", "deepseek-fast");
        let patch =
            bridge_config_patch_for_api_key(&settings, "relay-key", "deepseek-pro", &aliases)
                .unwrap()
                .unwrap();
        assert_eq!(patch["force_model"], "deepseek-pro");
        assert_eq!(patch["model_list_mode"], "aliases");
        let rows = patch["model_aliases"].as_array().unwrap();
        assert_eq!(rows.len(), 5);
        let haiku = rows
            .iter()
            .find(|row| row["id"] == "claude-haiku-4-5-20251001")
            .unwrap();
        assert_eq!(haiku["model"], "deepseek-fast");
    }

    #[test]
    fn launcher_state_does_not_expose_encrypted_api_key() {
        let entry = StoredApiKey {
            id: "key-1".into(),
            provider_id: "deepseek".into(),
            label: "DeepSeek".into(),
            base_url: "https://api.deepseek.com".into(),
            model: "deepseek-v4-pro".into(),
            custom_confirmed: false,
            model_aliases: Vec::new(),
            encrypted_api_key: "ciphertext-must-stay-local".into(),
        };
        let settings = LauncherSettings {
            active_api_key_id: Some(entry.id.clone()),
            api_keys: vec![entry],
            ..LauncherSettings::default()
        };
        let json = serde_json::to_string(&launcher_state(&settings)).unwrap();
        assert!(!json.contains("ciphertext-must-stay-local"));
        assert!(!json.contains("encryptedApiKey"));
        assert!(json.contains("\"hasSecret\":true"));
        assert!(json.contains("\"active\":true"));
    }

    #[test]
    fn launcher_state_preserves_api_key_add_order() {
        let make_entry = |id: &str, provider_id: &str| StoredApiKey {
            id: id.into(),
            provider_id: provider_id.into(),
            label: provider_id.into(),
            base_url: String::new(),
            model: String::new(),
            custom_confirmed: false,
            model_aliases: Vec::new(),
            encrypted_api_key: "ciphertext".into(),
        };
        let settings = LauncherSettings {
            api_keys: vec![
                make_entry("key-first", "deepseek"),
                make_entry("key-second", "openai"),
            ],
            ..LauncherSettings::default()
        };
        let state = launcher_state(&settings);
        assert_eq!(state.api_keys[0].id, "key-first");
        assert_eq!(state.api_keys[1].id, "key-second");
    }

    #[test]
    fn custom_relay_uses_user_name_when_provided() {
        let settings = LauncherSettings::default();
        assert_eq!(
            custom_relay_label_for_date(&settings, "  实验室主线路  ", "2026-07-11").unwrap(),
            "实验室主线路"
        );
    }

    #[test]
    fn unnamed_custom_relays_use_date_and_next_sequence() {
        let make_entry = |provider_id: &str, label: &str| StoredApiKey {
            id: next_api_key_id(),
            provider_id: provider_id.into(),
            label: label.into(),
            base_url: String::new(),
            model: String::new(),
            custom_confirmed: provider_id == "custom",
            model_aliases: Vec::new(),
            encrypted_api_key: "ciphertext".into(),
        };
        let settings = LauncherSettings {
            api_keys: vec![
                make_entry("custom", "自定义中转 2026-07-11 #01"),
                make_entry("custom", "自定义中转 2026-07-11 #03"),
                make_entry("custom", "自定义中转 2026-07-10 #20"),
                make_entry("deepseek", "自定义中转 2026-07-11 #99"),
            ],
            ..LauncherSettings::default()
        };
        assert_eq!(
            custom_relay_label_for_date(&settings, "", "2026-07-11").unwrap(),
            "自定义中转 2026-07-11 #04"
        );
    }

    #[test]
    fn unnamed_builtin_relays_use_provider_date_and_next_sequence() {
        let make_entry = |provider_id: &str, label: &str| StoredApiKey {
            id: next_api_key_id(),
            provider_id: provider_id.into(),
            label: label.into(),
            base_url: String::new(),
            model: String::new(),
            custom_confirmed: provider_id == "builtin-relay",
            model_aliases: Vec::new(),
            encrypted_api_key: "ciphertext".into(),
        };
        let settings = LauncherSettings {
            api_keys: vec![
                make_entry("builtin-relay", "项目方自建中转 2026-07-11 #01"),
                make_entry("builtin-relay", "项目方自建中转 2026-07-11 #02"),
                make_entry("custom", "项目方自建中转 2026-07-11 #99"),
            ],
            ..LauncherSettings::default()
        };
        assert_eq!(
            provider_entry_label_for_date(
                &settings,
                "builtin-relay",
                "项目方自建中转",
                "",
                "2026-07-11"
            )
            .unwrap(),
            "项目方自建中转 2026-07-11 #03"
        );
    }

    #[test]
    fn custom_relay_name_rejects_control_characters() {
        let settings = LauncherSettings::default();
        assert!(custom_relay_label_for_date(&settings, "坏\n名称", "2026-07-11").is_err());
    }

    #[test]
    fn api_key_test_budget_supports_reasoning_models() {
        const { assert!(API_KEY_TEST_INITIAL_MAX_TOKENS >= 256) };
        const { assert!(API_KEY_TEST_RETRY_MAX_TOKENS >= API_KEY_TEST_INITIAL_MAX_TOKENS * 4) };
    }

    #[test]
    fn old_single_provider_settings_load_without_api_key_list() {
        let settings: LauncherSettings = serde_json::from_str(
            r#"{
              "selectedProviderId": "deepseek",
              "customBaseUrl": "",
              "customConfirmed": false
            }"#,
        )
        .unwrap();
        assert_eq!(settings.selected_provider_id, "deepseek");
        assert!(settings.active_api_key_id.is_none());
        assert!(settings.api_keys.is_empty());
        assert!(launcher_state(&settings).api_keys.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrips_api_key_for_current_windows_user() {
        let encrypted = protect_api_key("test-key-for-dpapi").unwrap();
        assert!(!encrypted.contains("test-key-for-dpapi"));
        assert_eq!(unprotect_api_key(&encrypted).unwrap(), "test-key-for-dpapi");
    }

    #[test]
    fn claude_api_key_does_not_silently_store_bridge_key() {
        let settings = LauncherSettings {
            selected_provider_id: "claude".into(),
            custom_base_url: String::new(),
            custom_confirmed: false,
            ..LauncherSettings::default()
        };
        assert!(
            bridge_config_patch_for_api_key(&settings, "test-key", "", &[])
                .unwrap_err()
                .contains("Claude 官方模式")
        );
    }

    #[test]
    fn unconfirmed_custom_provider_does_not_apply_bridge_config() {
        let settings = LauncherSettings {
            selected_provider_id: "custom".into(),
            custom_base_url: String::new(),
            custom_confirmed: false,
            ..LauncherSettings::default()
        };
        assert!(bridge_config_patch_for_provider(&settings)
            .unwrap()
            .is_none());
    }

    #[test]
    fn bridge_config_hex_argument_roundtrips_json_with_quotes() {
        let value = serde_json::json!({
            "custom_base_url": "https://10521052.xyz/v1",
            "force_model": "glm-5.2"
        });
        let hex = json_arg_hex(&value).unwrap();
        let bytes: Vec<u8> = hex
            .as_bytes()
            .chunks(2)
            .map(|pair| {
                let text = std::str::from_utf8(pair).unwrap();
                u8::from_str_radix(text, 16).unwrap()
            })
            .collect();
        let decoded: serde_json::Value =
            serde_json::from_slice(&bytes).expect("hex should decode to JSON");
        assert_eq!(decoded, value);
    }

    #[test]
    fn bridge_config_rollback_records_restore_and_delete_keys() {
        let mut restore = serde_json::Map::new();
        restore.insert(
            "force_model".into(),
            serde_json::Value::String("old".into()),
        );
        let rollback = BridgeConfigRollback {
            restore,
            delete: vec!["custom_base_url".into()],
        };
        let encoded = json_arg_hex(&rollback).unwrap();
        assert!(!encoded.contains('"'));
        assert!(encoded.len() > 20);
    }

    #[test]
    fn dashboard_url_includes_path_secret_only_when_required() {
        assert_eq!(
            dashboard_url_from_config(&serde_json::json!({})),
            "http://127.0.0.1:9876/dashboard"
        );
        assert_eq!(
            dashboard_url_from_config(&serde_json::json!({
                "proxy_auth_mode": "optional",
                "proxy_auth_token": "secret token"
            })),
            "http://127.0.0.1:9876/dashboard"
        );
        assert_eq!(
            dashboard_url_from_config(&serde_json::json!({
                "proxy_host": "127.0.0.1",
                "proxy_port": 9876,
                "proxy_auth_mode": "required",
                "proxy_auth_token": "secret token"
            })),
            "http://127.0.0.1:9876/secret%20token/dashboard"
        );
    }

    #[test]
    fn claude_session_parser_returns_latest_text_messages() {
        let jsonl = r#"{"type":"user","uuid":"user-1","timestamp":"2026-07-16T00:00:00Z","message":{"role":"user","content":"check environment\nAPI_KEY=secret"}}
{"type":"assistant","uuid":"assistant-1","timestamp":"2026-07-16T00:00:02Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hidden"},{"type":"text","text":"environment ready"}]}}"#;
        let messages = parse_claude_session_messages("session-123", jsonl);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "assistant");
        assert_eq!(messages[0].content, "environment ready");
        assert_eq!(messages[1].role, "user");
        assert!(messages[1].content.contains("API_KEY=[redacted]"));
    }

    #[test]
    fn claude_session_parser_skips_invalid_and_sidechain_events() {
        let jsonl = r#"not-json
{"type":"attachment","uuid":"attachment-1"}
{"type":"assistant","uuid":"sidechain-1","isSidechain":true,"message":{"role":"assistant","content":[{"type":"text","text":"skip me"}]}}
{"type":"assistant","uuid":"tool-only","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash"}]}}"#;
        assert!(parse_claude_session_messages("session-123", jsonl).is_empty());
    }

    #[test]
    fn headed_session_ids_are_valid_unique_v4_uuids() {
        let first = new_claude_session_id();
        let second = new_claude_session_id();
        assert_ne!(first, second);
        assert_eq!(first.len(), 36);
        assert_eq!(&first[14..15], "4");
        assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
        assert!(safe_claude_session_id(&first).is_ok());
    }

    #[test]
    fn headed_launch_script_reads_prompt_file_and_stays_in_plan_mode() {
        let script = headed_claude_launch_script(
            Path::new("C:\\Tools\\claude.exe"),
            Path::new("C:\\workspace"),
            Path::new("C:\\runs\\prompt.md"),
            "123e4567-e89b-42d3-a456-426614174000",
            "CSA environment-check",
        );
        assert!(script.contains("Get-Content -Raw -LiteralPath"));
        assert!(script.contains("--session-id '123e4567-e89b-42d3-a456-426614174000'"));
        assert!(script.contains("--permission-mode plan"));
        assert!(script.contains("Read-Host 'Press Enter to close this window'"));
        assert!(!script.contains("dangerously-skip-permissions"));
    }

    #[test]
    fn subagent_request_ids_reject_path_traversal_and_shell_characters() {
        assert!(safe_subagent_id("dataset-check-01").is_ok());
        for value in ["..", "../request", "request/child", "request;whoami", ""] {
            assert!(
                safe_subagent_id(value).is_err(),
                "accepted unsafe id: {value}"
            );
        }
    }

    #[test]
    fn subagent_prompt_enforces_manual_approval_and_redacts_credentials() {
        let request = SubagentRequest {
            schema_version: Some(1),
            source: Some("sandbox".into()),
            task_kind: Some("environment".into()),
            title: Some("Dependency diagnosis".into()),
            cwd: Some("C:\\workspace".into()),
            note: Some("API_KEY=do-not-leak\nTOKEN=also-private\nPackage import failed".into()),
            requested_action: Some("diagnose".into()),
            approval_mode: Some("manual".into()),
            policy_id: Some("manual-only".into()),
            created_at: Some("2026-07-24T00:00:00Z".into()),
        };
        let prompt = build_subagent_prompt("environment-check-01", &request);
        assert!(prompt.contains("等待用户批准"));
        assert!(prompt.contains("approvalMode: manual"));
        assert!(prompt.contains("API_KEY=[redacted]"));
        assert!(!prompt.contains("do-not-leak"));
        assert!(prompt.contains("TOKEN=[redacted]"));
        assert!(!prompt.contains("also-private"));
    }

    #[test]
    fn subagent_outbox_has_stable_relative_result_and_redacted_summary() {
        let running_agent = ExternalAgentRunResult {
            ok: true,
            tool: "claude".into(),
            exit_code: None,
            duration_ms: 0,
            stdout: String::new(),
            stderr: String::new(),
            result_text: None,
            session_id: Some("123e4567-e89b-42d3-a456-426614174000".into()),
            resume_command: None,
            message: "Session opened".into(),
        };
        let running = build_subagent_outbox_result(
            "environment-check-01",
            "environment-check-01-1784000000000",
            &running_agent,
        )
        .unwrap();
        assert_eq!(running.status, "running");
        assert_eq!(running.next_action, "wait_for_result");
        assert_eq!(
            running.result_path,
            "reports/csa-agent-runs/environment-check-01-1784000000000/result.json"
        );
        assert!(!running.result_path.contains("C:\\"));

        let mut completed_agent = running_agent;
        completed_agent.result_text =
            Some("API_KEY=do-not-leak\nTOKEN=also-private\nDiagnosis complete".into());
        let completed = build_subagent_outbox_result(
            "environment-check-01",
            "environment-check-01-1784000000001",
            &completed_agent,
        )
        .unwrap();
        assert_eq!(completed.status, "completed");
        assert_eq!(completed.next_action, "read_result");
        assert!(completed.summary.contains("API_KEY=[redacted]"));
        assert!(!completed.summary.contains("do-not-leak"));
        assert!(completed.summary.contains("TOKEN=[redacted]"));
        assert!(!completed.summary.contains("also-private"));
    }

    #[test]
    fn skill_metadata_parser_reads_yaml_front_matter() {
        let content = r#"---
name: literature-review
description: Review papers with a reproducible evidence table.
---
# Ignored heading
"#;
        let (name, description) = parse_skill_metadata(content, "skills/review/SKILL.md");
        assert_eq!(name, "literature-review");
        assert_eq!(
            description,
            "Review papers with a reproducible evidence table."
        );
    }

    #[test]
    fn skill_metadata_parser_falls_back_to_heading_and_folder() {
        let (heading, description) = parse_skill_metadata(
            "# Dataset Inspector\n\nChecks local datasets.",
            "data/SKILL.md",
        );
        assert_eq!(heading, "Dataset Inspector");
        assert_eq!(description, "Checks local datasets.");
        let (folder, _) = parse_skill_metadata("", "skills/gpu-check/SKILL.md");
        assert_eq!(folder, "gpu-check");
    }

    #[test]
    fn skill_repository_source_rejects_ambiguous_or_insecure_values() {
        assert!(validate_skill_repository_source("https://github.com/org/skills.git").is_ok());
        assert!(validate_skill_repository_source("git@github.com:org/skills.git").is_ok());
        assert!(validate_skill_repository_source("http://example.com/skills.git").is_err());
        assert!(validate_skill_repository_source("../skills").is_err());
        assert!(
            validate_skill_repository_source("https://example.com/a.git\n--upload-pack=x").is_err()
        );
    }

    #[test]
    fn skill_repository_scan_reads_only_git_tracked_skill_files() {
        if verify_system_git().is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "csa-skill-scan-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        let tracked = root.join("skills").join("tracked").join("SKILL.md");
        let untracked = root.join("skills").join("untracked").join("SKILL.md");
        fs::create_dir_all(tracked.parent().unwrap()).unwrap();
        fs::create_dir_all(untracked.parent().unwrap()).unwrap();
        fs::write(
            &tracked,
            "---\nname: tracked-skill\ndescription: Safe tracked skill.\n---\n",
        )
        .unwrap();
        fs::write(&untracked, "# Untracked\n").unwrap();
        let mut init = background_command("git");
        init.arg("-C").arg(&root).arg("init");
        git_output(init, Duration::from_secs(10), "测试 Git 初始化").unwrap();
        let mut add = background_command("git");
        add.arg("-C")
            .arg(&root)
            .args(["add", "--", "skills/tracked/SKILL.md"]);
        git_output(add, Duration::from_secs(10), "测试 Git add").unwrap();

        let first = scan_skill_repository("repo-test", &root, &[]).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].name, "tracked-skill");
        assert!(first[0].is_new);
        let second = scan_skill_repository("repo-test", &root, &first).unwrap();
        assert_eq!(second.len(), 1);
        assert!(!second[0].is_new);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn connect_validators_accept_only_expected_official_endpoints() {
        assert!(validate_feishu_webhook(
            "https://open.feishu.cn/open-apis/bot/v2/hook/12345678-abcd"
        )
        .is_ok());
        assert!(
            validate_feishu_webhook("https://example.com/open-apis/bot/v2/hook/secret").is_err()
        );
        assert!(validate_telegram_bot_token("123456789:ABCdef_123456789-xyz").is_ok());
        assert!(validate_telegram_bot_token("not-a-token").is_err());
        assert!(validate_telegram_chat_id("-1001234567890").is_ok());
        assert!(validate_telegram_chat_id("@csa_updates").is_ok());
        assert!(validate_telegram_chat_id("hello world").is_err());
    }

    #[test]
    fn connect_state_never_serializes_encrypted_credentials() {
        let settings = StoredConnectSettings {
            encrypted_feishu_webhook: "cipher-feishu".to_string(),
            encrypted_telegram_bot_token: "cipher-telegram".to_string(),
            telegram_chat_id: "-1001234567890".to_string(),
            feishu_updated_at: 1,
            telegram_updated_at: 2,
        };
        let json = serde_json::to_string(&connect_state(&settings)).unwrap();
        assert!(!json.contains("cipher-feishu"));
        assert!(!json.contains("cipher-telegram"));
        assert!(!json.contains("-1001234567890"));
        assert!(json.contains("…7890"));
    }

    #[test]
    fn system_drive_detection_warns_only_for_windows_c_drive() {
        assert!(is_windows_system_drive(Some("C:")));
        assert!(is_windows_system_drive(Some("c:\\")));
        assert!(!is_windows_system_drive(Some("D:")));
        assert!(!is_windows_system_drive(Some("/mnt/c")));
        assert!(!is_windows_system_drive(None));
    }
}
