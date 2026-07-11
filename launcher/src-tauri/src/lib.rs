use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::{ffi::OsStrExt, process::CommandExt};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

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
        warnings.push("Port 9876 is answered by a Bridge from another or older CSA package directory; restart from this package to migrate it.".into());
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
    } else if bridge_running && windows_bridge_pid.is_some() {
        "degraded"
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
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));

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

fn custom_relay_label_for_date(
    settings: &LauncherSettings,
    requested_name: &str,
    date: &str,
) -> Result<String, String> {
    let requested_name = validate_display_name(requested_name)?;
    if !requested_name.is_empty() {
        return Ok(requested_name);
    }
    let prefix = format!("自定义中转 {date} #");
    let highest = settings
        .api_keys
        .iter()
        .filter(|entry| entry.provider_id == "custom")
        .filter_map(|entry| entry.label.strip_prefix(&prefix))
        .filter_map(|sequence| sequence.parse::<u32>().ok())
        .max()
        .unwrap_or_default();
    Ok(format!("{prefix}{:02}", highest.saturating_add(1)))
}

fn custom_relay_label(settings: &LauncherSettings, requested_name: &str) -> Result<String, String> {
    custom_relay_label_for_date(settings, requested_name, &current_local_date())
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
    let label = if selected_provider_id == "custom" {
        custom_relay_label(&settings, &display_name)?
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
for pid in $(ps -eo pid=,args= | awk '/claude-science/ && /serve/ && !/awk/ {print $1}'); do
  kill "$pid" 2>/dev/null || true
done
for pid in $(ss -ltnp "sport = :9876" 2>/dev/null | grep -o 'pid=[0-9]*' | cut -d= -f2 | sort -u); do
  if [ -r "/proc/$pid/cmdline" ]; then
    cmd="$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
    case "$cmd" in *"/proxy.py"*) kill "$pid" 2>/dev/null || true;; esac
  fi
done
deadline=$((SECONDS + 5))
while [ "$SECONDS" -lt "$deadline" ]; do
  if ! curl -fsS --connect-timeout 0.3 --max-time 0.6 http://127.0.0.1:9876/health >/dev/null 2>&1 \
    && ! ss -ltn 2>/dev/null | grep -qE ':(8765|8766) '; then
    exit 0
  fi
  sleep 0.25
done
echo "CSA services did not stop within 5 seconds." >&2
exit 1
"#;
    let output = run_wsl_with_timeout(distro, &["sh", "-lc", script], Duration::from_secs(10))?;
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
            delete_api_key
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
    fn custom_relay_name_rejects_control_characters() {
        let settings = LauncherSettings::default();
        assert!(custom_relay_label_for_date(&settings, "坏\n名称", "2026-07-11").is_err());
    }

    #[test]
    fn api_key_test_budget_supports_reasoning_models() {
        assert!(API_KEY_TEST_INITIAL_MAX_TOKENS >= 256);
        assert!(API_KEY_TEST_RETRY_MAX_TOKENS >= API_KEY_TEST_INITIAL_MAX_TOKENS * 4);
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
}
