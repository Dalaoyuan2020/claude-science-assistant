use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri_plugin_opener::OpenerExt;

mod bridge_egress;
mod bridge_egress_apply;
mod runtime_lifecycle;
mod smoke;

pub use smoke::smoke_exit_code_if_requested;

use bridge_egress::{
    run_bridge_egress_connection_probe, run_bridge_egress_probe,
    run_bridge_egress_probe_for_models, BridgeEgressLayerState, BridgeEgressReport,
};
use bridge_egress_apply::apply_bridge_egress_fix;

use runtime_lifecycle::{
    parse_runtime_identity, runtime_identity_from_health, RuntimeIdentity, ServiceOperationLock,
};

#[cfg(windows)]
use std::os::windows::{ffi::OsStrExt, process::CommandExt};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

// Reasoning models such as GLM-5.2 can spend the first dozens of tokens in
// reasoning_content. A tiny cap returns HTTP 200 with an empty final answer and
// makes a valid Key look broken.
const API_KEY_TEST_INITIAL_MAX_TOKENS: u32 = 256;
const API_KEY_TEST_RETRY_MAX_TOKENS: u32 = 1024;

const CLAUDE_SCIENCE_RELEASE_BASE: &str = "https://storage.googleapis.com/operon-dist-cf94a20e-f71c-413c-bd00-9e12b1fedf59/operon-releases";
const CLAUDE_SCIENCE_CHANGELOG_URL: &str = "https://claude.com/docs/claude-science/changelog";
const BUNDLED_CLAUDE_SCIENCE_VERSION: &str = "0.1.25";
const BUNDLED_CLAUDE_SCIENCE_SHA8: &str = "b7190511";
const SUBSCRIPTION_ROLES: [&str; 3] = ["default", "vision", "fast"];
const NETWORK_DEEP_CACHE_MAX_AGE_SECONDS: u64 = 15 * 60;
const SANDBOX_NETWORK_PROBE_IDENTITY: &str = "analysis-socks5h-pypi-head-v2";
const SANDBOX_NETWORK_CANARY_IDENTITY: &str = "https://pypi.org/simple/pip/";
const CLAUDE_OPEN_HARD_BUDGET: Duration = Duration::from_secs(7);
const CLAUDE_OPEN_LOCK_BUDGET: Duration = Duration::from_millis(300);
const CLAUDE_OPEN_DISTRO_BUDGET: Duration = Duration::from_millis(1_200);
const CLAUDE_OPEN_CLEANUP_RESERVE: Duration = Duration::from_millis(300);
const CLAUDE_OPEN_GUEST_RESERVE: Duration = Duration::from_millis(300);

fn deep_network_result_is_fresh(
    deep_checked: bool,
    checked_at_unix: Option<u64>,
    now_unix: u64,
) -> bool {
    if !deep_checked {
        return false;
    }
    checked_at_unix
        .and_then(|checked_at| now_unix.checked_sub(checked_at))
        .map(|age| age <= NETWORK_DEEP_CACHE_MAX_AGE_SECONDS)
        .unwrap_or(false)
}

fn core_runtime_ready_for_ui(local_network_ready: bool, claude_io_blocked: bool) -> bool {
    local_network_ready && !claude_io_blocked
}

fn transient_deep_daemon_result_recovered(
    deep_checked: bool,
    sandbox_egress_state: &str,
    sandbox_probe_daemon_io_blocked: bool,
    sandbox_probe_daemon_mount_io_blocked: bool,
    daemon_process_state: &str,
    daemon_io_blocked: bool,
) -> bool {
    deep_checked
        && (matches!(sandbox_egress_state, "daemon_busy" | "daemon_mount_io_busy")
            || sandbox_probe_daemon_io_blocked
            || sandbox_probe_daemon_mount_io_blocked)
        && !daemon_io_blocked
        && matches!(daemon_process_state, "R" | "S" | "I")
}

// Provider changes update the WSL Bridge config and restart its listener. Keep
// the whole write/restart/verify transaction single-flight to prevent a second
// click from racing the first transaction's rollback.
static BRIDGE_CONFIG_TRANSITION: OnceLock<Mutex<()>> = OnceLock::new();
const BRIDGE_RESTART_GUEST_TIMEOUT_SECONDS: u64 = 42;
const BRIDGE_RESTART_HOST_TIMEOUT: Duration = Duration::from_secs(48);

fn bridge_config_transition_lock() -> &'static Mutex<()> {
    BRIDGE_CONFIG_TRANSITION.get_or_init(|| Mutex::new(()))
}

fn service_operation_lock_with_timeout(
    operation: &str,
    timeout: Duration,
) -> Result<ServiceOperationLock, String> {
    let path = settings_path()?
        .parent()
        .ok_or_else(|| "无法定位 CSA 状态目录".to_string())?
        .join("service-lifecycle.lock");
    ServiceOperationLock::acquire(&path, operation, timeout)
}

fn classify_service_operation_error(operation: &str, error: String) -> String {
    if ERROR_LAYER_PREFIXES
        .iter()
        .any(|candidate| error.starts_with(candidate))
    {
        return error;
    }
    let (held_code, unavailable_code) = if operation == "provider-transition" {
        (
            "bridge.transition_lock_held",
            "bridge.transition_lock_unavailable",
        )
    } else {
        ("runtime.lock_held", "runtime.lock_unavailable")
    };
    if error.contains("Another CSA service operation") {
        format!("{held_code}:{operation}")
    } else {
        format!(
            "{unavailable_code}:{operation}: {}",
            clean_diagnostic_text(&error)
        )
    }
}

fn service_operation_lock(operation: &str) -> Result<ServiceOperationLock, String> {
    service_operation_lock_with_timeout(operation, Duration::from_secs(3))
        .map_err(|error| classify_service_operation_error(operation, error))
}

fn service_operation_quick_lock(
    operation: &str,
    timeout: Duration,
) -> Result<ServiceOperationLock, String> {
    let path = settings_path()?
        .parent()
        .ok_or_else(|| "无法定位 CSA 状态目录".to_string())?
        .join("service-lifecycle.lock");
    ServiceOperationLock::acquire_quick(&path, timeout)
        .map_err(|error| classify_service_operation_error(operation, error))
}

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
    command: Command,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    command_output_with_optional_stdin_timeout(command, None, timeout, label)
}

fn command_output_with_stdin_timeout(
    command: Command,
    input: &[u8],
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    command_output_with_optional_stdin_timeout(command, Some(input), timeout, label)
}

fn command_output_with_optional_stdin_timeout(
    mut command: Command,
    input: Option<&[u8]>,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("{label}启动失败：{error}"))?;
    let started = Instant::now();
    let stdin = if input.is_some() {
        match child.stdin.take() {
            Some(stdin) => Some(stdin),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{label}无法打开标准输入"));
            }
        }
    } else {
        None
    };
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{label}无法读取标准输出"));
        }
    };
    let mut stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{label}无法读取错误输出"));
        }
    };
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
    // Never write stdin synchronously on the timeout-owning thread. A child
    // that does not read can fill the pipe and otherwise bypass the watchdog.
    let stdin_writer = input.zip(stdin).map(|(input, mut stdin)| {
        let input = input.to_vec();
        let label = label.to_string();
        thread::spawn(move || {
            stdin
                .write_all(&input)
                .map_err(|error| format!("{label}无法写入标准输入：{error}"))
        })
    });
    let status_result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "{label}在 {} 秒内没有响应，已停止本次操作。请检查宿主磁盘空间、WSL VHDX 与发行版状态。",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("{label}状态读取失败：{error}"));
            }
        }
    };
    let write_result = stdin_writer
        .map(|writer| {
            writer
                .join()
                .unwrap_or_else(|_| Err(format!("{label}标准输入写入线程异常退出")))
        })
        .unwrap_or(Ok(()));
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    let status = status_result?;
    write_result?;
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

const ERROR_LAYER_PREFIXES: [&str; 6] = [
    "login.",
    "runtime.",
    "bridge.",
    "grade.",
    "work.",
    "transport.",
];

fn ensure_error_prefix(prefix: &str, error: String) -> String {
    if ERROR_LAYER_PREFIXES
        .iter()
        .any(|candidate| error.starts_with(candidate))
    {
        error
    } else {
        format!("{prefix}: {}", clean_diagnostic_text(&error))
    }
}

const ALLOW_OPEN_INPUTS: [&str; 2] = ["claudeRunning", "windowsBridgePid"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AllowStatus {
    wsl_installed: bool,
    distro: Option<String>,
    linux_user: Option<String>,
    runtime_present: bool,
    claude_running: bool,
    claude_pid: Option<u32>,
    listener_present: bool,
    pid_8765: Option<u32>,
    pid_8766: Option<u32>,
    daemon_state: String,
    listener_probe_ok: bool,
    control_socket_present: bool,
    windows_bridge_pid: Option<u32>,
    windows_bridge_probe: String,
    can_open: bool,
    can_start: bool,
}

fn derive_can_open(claude_running: bool, windows_bridge_pid: Option<u32>) -> bool {
    claude_running && windows_bridge_pid.is_none()
}

fn allow_status_impl() -> Result<AllowStatus, String> {
    smoke::probe_allow_status()
}

#[derive(Debug, Clone, Default, Serialize)]
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
    claude_listener_present: bool,
    claude_port_8765: bool,
    claude_port_8766: bool,
    claude_unverified_pid: Option<u32>,
    bridge_healthy: bool,
    bridge_identity: Option<RuntimeIdentity>,
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
    host_access_preferences_present: bool,
    host_access_preferences_parse_ok: bool,
    drvfs_write_grant_count: u32,
    broad_drvfs_write_grant_count: u32,
    drvfs_write_grants: Vec<String>,
    network: NetworkQualityStatus,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GradeStatus {
    state: String,
    wsl_installed: bool,
    bridge_running: bool,
    bridge_pid: Option<u32>,
    bridge_healthy: bool,
    bridge_identity: Option<RuntimeIdentity>,
    claude_listener_present: bool,
    claude_port_8765: bool,
    claude_port_8766: bool,
    claude_unverified_pid: Option<u32>,
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
    host_access_preferences_present: bool,
    host_access_preferences_parse_ok: bool,
    drvfs_write_grant_count: u32,
    broad_drvfs_write_grant_count: u32,
    drvfs_write_grants: Vec<String>,
    network: GradeNetworkStatus,
    warnings: Vec<String>,
}

impl From<SystemStatus> for GradeStatus {
    fn from(status: SystemStatus) -> Self {
        Self {
            state: status.state,
            wsl_installed: status.wsl_installed,
            bridge_running: status.bridge_running,
            bridge_pid: status.bridge_pid,
            bridge_healthy: status.bridge_healthy,
            bridge_identity: status.bridge_identity,
            claude_listener_present: status.claude_listener_present,
            claude_port_8765: status.claude_port_8765,
            claude_port_8766: status.claude_port_8766,
            claude_unverified_pid: status.claude_unverified_pid,
            runtime_ready: status.runtime_ready,
            source_binary_present: status.source_binary_present,
            bridge_venv_present: status.bridge_venv_present,
            wsl_storage_path: status.wsl_storage_path,
            wsl_storage_drive: status.wsl_storage_drive,
            wsl_storage_free_gb: status.wsl_storage_free_gb,
            wsl_vhdx_size_gb: status.wsl_vhdx_size_gb,
            wsl_root_free_gb: status.wsl_root_free_gb,
            settings_storage_drive: status.settings_storage_drive,
            settings_storage_free_gb: status.settings_storage_free_gb,
            storage_warning: status.storage_warning,
            storage_blocked: status.storage_blocked,
            restart_blocked: status.restart_blocked,
            host_access_preferences_present: status.host_access_preferences_present,
            host_access_preferences_parse_ok: status.host_access_preferences_parse_ok,
            drvfs_write_grant_count: status.drvfs_write_grant_count,
            broad_drvfs_write_grant_count: status.broad_drvfs_write_grant_count,
            drvfs_write_grants: status.drvfs_write_grants,
            network: GradeNetworkStatus::from(&status.network),
            warnings: status
                .warnings
                .into_iter()
                .filter(|warning| !work_lane_warning(warning))
                .collect(),
        }
    }
}

fn grade_status_impl() -> GradeStatus {
    GradeStatus::from(current_status())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NetworkQualityStatus {
    proxy_state: String,
    local_ready: bool,
    ready: bool,
    proxy_reachable: Option<bool>,
    proxy_endpoints: Vec<String>,
    proxy_conflict: bool,
    sandbox_forwarder_count: u32,
    sandbox_forwarder_expected_count: u32,
    sandbox_forwarder_topology_state: String,
    sandbox_http_forwarder_count: u32,
    sandbox_socks_forwarder_count: u32,
    sandbox_probe_role: String,
    sandbox_probe_transport: String,
    daemon_process_state: String,
    daemon_wait_channel: String,
    daemon_io_blocked: bool,
    daemon_mount_io_blocked: bool,
    sandbox_unix_socket_state: String,
    sandbox_socks_handshake_state: String,
    sandbox_egress_failure_stage: String,
    deep_checked: bool,
    deep_checked_at_unix: Option<u64>,
    sandbox_egress_state: String,
    sandbox_egress_target: Option<String>,
    sandbox_egress_http_status: Option<u16>,
}

impl Default for NetworkQualityStatus {
    fn default() -> Self {
        Self {
            proxy_state: "unknown".into(),
            local_ready: false,
            ready: false,
            proxy_reachable: None,
            proxy_endpoints: Vec::new(),
            proxy_conflict: false,
            sandbox_forwarder_count: 0,
            sandbox_forwarder_expected_count: 3,
            sandbox_forwarder_topology_state: "incomplete".into(),
            sandbox_http_forwarder_count: 0,
            sandbox_socks_forwarder_count: 0,
            sandbox_probe_role: "analysis".into(),
            sandbox_probe_transport: "socks5h".into(),
            daemon_process_state: "unknown".into(),
            daemon_wait_channel: "unknown".into(),
            daemon_io_blocked: false,
            daemon_mount_io_blocked: false,
            sandbox_unix_socket_state: "not_checked".into(),
            sandbox_socks_handshake_state: "not_checked".into(),
            sandbox_egress_failure_stage: "not_checked".into(),
            deep_checked: false,
            deep_checked_at_unix: None,
            sandbox_egress_state: "not_checked".into(),
            sandbox_egress_target: None,
            sandbox_egress_http_status: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GradeNetworkStatus {
    proxy_state: String,
    local_ready: bool,
    proxy_reachable: Option<bool>,
    proxy_endpoints: Vec<String>,
    proxy_conflict: bool,
    sandbox_forwarder_count: u32,
    sandbox_forwarder_expected_count: u32,
    sandbox_forwarder_topology_state: String,
    sandbox_http_forwarder_count: u32,
    sandbox_socks_forwarder_count: u32,
    sandbox_probe_role: String,
    sandbox_probe_transport: String,
    daemon_process_state: String,
    daemon_wait_channel: String,
    daemon_io_blocked: bool,
    daemon_mount_io_blocked: bool,
}

impl From<&NetworkQualityStatus> for GradeNetworkStatus {
    fn from(network: &NetworkQualityStatus) -> Self {
        Self {
            proxy_state: network.proxy_state.clone(),
            local_ready: network.local_ready,
            proxy_reachable: network.proxy_reachable,
            proxy_endpoints: network.proxy_endpoints.clone(),
            proxy_conflict: network.proxy_conflict,
            sandbox_forwarder_count: network.sandbox_forwarder_count,
            sandbox_forwarder_expected_count: network.sandbox_forwarder_expected_count,
            sandbox_forwarder_topology_state: network.sandbox_forwarder_topology_state.clone(),
            sandbox_http_forwarder_count: network.sandbox_http_forwarder_count,
            sandbox_socks_forwarder_count: network.sandbox_socks_forwarder_count,
            sandbox_probe_role: network.sandbox_probe_role.clone(),
            sandbox_probe_transport: network.sandbox_probe_transport.clone(),
            daemon_process_state: network.daemon_process_state.clone(),
            daemon_wait_channel: network.daemon_wait_channel.clone(),
            daemon_io_blocked: network.daemon_io_blocked,
            daemon_mount_io_blocked: network.daemon_mount_io_blocked,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeepNetworkQualityStatus {
    deep_checked: bool,
    deep_checked_at_unix: Option<u64>,
    sandbox_unix_socket_state: String,
    sandbox_socks_handshake_state: String,
    sandbox_egress_failure_stage: String,
    sandbox_egress_state: String,
    sandbox_egress_target: Option<String>,
    sandbox_egress_http_status: Option<u16>,
}

impl From<&NetworkQualityStatus> for DeepNetworkQualityStatus {
    fn from(network: &NetworkQualityStatus) -> Self {
        Self {
            deep_checked: network.deep_checked,
            deep_checked_at_unix: network.deep_checked_at_unix,
            sandbox_unix_socket_state: network.sandbox_unix_socket_state.clone(),
            sandbox_socks_handshake_state: network.sandbox_socks_handshake_state.clone(),
            sandbox_egress_failure_stage: network.sandbox_egress_failure_stage.clone(),
            sandbox_egress_state: network.sandbox_egress_state.clone(),
            sandbox_egress_target: network.sandbox_egress_target.clone(),
            sandbox_egress_http_status: network.sandbox_egress_http_status,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkReport {
    operation: String,
    ok: bool,
    code: String,
    // Never return local readiness, daemon scheduler state, PID or topology
    // from the long-running probe. Only deep quality fields may be merged into
    // the last shallow status, so a probe cannot disable local login.
    #[serde(skip_serializing_if = "Option::is_none")]
    deep: Option<DeepNetworkQualityStatus>,
    warnings: Vec<String>,
}

fn work_lane_warning(warning: &str) -> bool {
    warning.starts_with("Sandbox deep egress quality check")
}

fn network_quality_work_report(status: SystemStatus) -> WorkReport {
    let deep = status
        .network
        .deep_checked
        .then(|| DeepNetworkQualityStatus::from(&status.network));
    let ok = deep
        .as_ref()
        .is_some_and(|report| report.sandbox_egress_state == "ok");
    let code = match deep
        .as_ref()
        .map(|report| report.sandbox_egress_state.as_str())
    {
        Some("ok") => "work.network_quality.ok",
        Some("daemon_busy") => "work.network_quality.daemon_busy",
        Some("daemon_mount_io_busy") => "work.network_quality.daemon_mount_io_busy",
        Some("not_checked") | None => "work.network_quality.not_checked",
        Some(_) => "work.network_quality.failed",
    };
    WorkReport {
        operation: "network_quality".into(),
        ok,
        code: code.into(),
        deep,
        warnings: status
            .warnings
            .into_iter()
            .filter(|warning| work_lane_warning(warning))
            .collect(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeReleaseSummary {
    version: String,
    sha8: String,
    build_date: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeUpdateStatus {
    bundled_version: String,
    bundled_sha8: String,
    recommended_version: String,
    latest: RuntimeReleaseSummary,
    stable: RuntimeReleaseSummary,
    update_available: bool,
    checked_at_unix: u64,
    release_notes_url: String,
    note: String,
}

#[derive(Debug, Deserialize)]
struct OfficialRuntimeManifest {
    version: String,
    sha8: String,
    #[serde(rename = "buildDate", default)]
    build_date: String,
    #[serde(default)]
    sha256: serde_json::Value,
}

#[derive(Debug, Default)]
struct WindowsStorageSnapshot {
    wsl_base_path: Option<String>,
    wsl_drive: Option<String>,
    wsl_drive_free_bytes: Option<u64>,
    vhdx_size_bytes: Option<u64>,
    settings_drive: Option<String>,
    settings_drive_free_bytes: Option<u64>,
    diagnostic: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WslProbeReport {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    wsl: WslProbeIdentity,
    #[serde(default)]
    components: WslProbeComponents,
    #[serde(default)]
    storage: WslProbeStorage,
    #[serde(default)]
    runtime: WslProbeRuntime,
    #[serde(default)]
    network: WslProbeNetwork,
    #[serde(default)]
    host_access: WslProbeHostAccess,
}

#[derive(Debug, Default, Deserialize)]
struct WslProbeIdentity {
    #[serde(default)]
    user: String,
    #[serde(default)]
    systemd: bool,
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
    claude_unverified_pid: Option<u32>,
    #[serde(default)]
    claude_owner_verified: bool,
    bridge_source_path: Option<String>,
    bridge_source_matches: Option<bool>,
    bridge_identity: Option<RuntimeIdentity>,
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

fn claude_runtime_is_running(runtime: &WslProbeRuntime) -> bool {
    runtime.claude_owner_verified
        && runtime.claude_pid.is_some()
        && runtime.port_8765
        && runtime.port_8766
}

fn claude_listener_is_present(runtime: &WslProbeRuntime) -> bool {
    runtime.claude_pid.is_some()
        || runtime.claude_unverified_pid.is_some()
        || runtime.port_8765
        || runtime.port_8766
}

fn append_claude_runtime_warning(
    warnings: &mut Vec<String>,
    bridge_healthy: bool,
    runtime: &WslProbeRuntime,
) {
    if claude_runtime_is_running(runtime) {
        return;
    }

    if claude_listener_is_present(runtime) {
        let owner = runtime
            .claude_unverified_pid
            .map(|pid| format!("PID {pid}"))
            .unwrap_or_else(|| "未知进程".into());
        if runtime.port_8765 && runtime.port_8766 {
            warnings.push(format!(
                "Claude Science 的 8765/8766 端口正在监听（{owner}），但尚未通过同一受管进程身份校验；CSA 不会误报为已停止，也不会自动终止该进程。"
            ));
        } else {
            let topology = match (runtime.port_8765, runtime.port_8766) {
                (true, false) => "仅 8765",
                (false, true) => "仅 8766",
                _ => "端口归属信息不完整",
            };
            warnings.push(format!(
                "Claude Science 已出现监听进程（{owner}，{topology}），但双端口拓扑尚未就绪；当前按启动中或异常状态处理，不会显示为已停止。"
            ));
        }
    } else if bridge_healthy {
        warnings.push(
            "Bridge 已就绪，但 Claude Science 核心服务尚未运行；启动器会把核心服务作为首要启动目标。"
                .into(),
        );
    }
}

#[derive(Debug, Default)]
struct SystemStateInputs {
    host_access_repair_needed: bool,
    bridge_healthy: bool,
    claude_running: bool,
    unit_contract_ok: bool,
    network_ready: bool,
    storage_blocked: bool,
    wsl_runtime_writable: bool,
    runtime_ready: bool,
    bridge_running: bool,
    claude_listener_present: bool,
    windows_bridge_present: bool,
}

fn classify_system_state(input: &SystemStateInputs) -> &'static str {
    if input.host_access_repair_needed {
        "degraded"
    } else if input.windows_bridge_present {
        // Never advertise a normal start/restart path while the legacy
        // Windows listener exists: doing so could create a second WSL Bridge.
        "degraded"
    } else if input.bridge_healthy
        && input.claude_running
        && input.unit_contract_ok
        && input.network_ready
    {
        "running"
    } else if input.storage_blocked || !input.wsl_runtime_writable {
        "degraded"
    } else if !input.runtime_ready && !input.bridge_running && !input.claude_listener_present {
        "notInstalled"
    } else if input.bridge_healthy
        && input.runtime_ready
        && !input.claude_listener_present
        && input.unit_contract_ok
        && !input.windows_bridge_present
    {
        // A healthy Bridge is Claude Science's dependency, not evidence that
        // the core daemon needs repair. The primary action can use a normal,
        // non-destructive start transaction from this state.
        "stopped"
    } else if input.bridge_running || input.claude_listener_present {
        "degraded"
    } else {
        "stopped"
    }
}

#[derive(Debug, Default, Deserialize)]
struct WslProbeNetwork {
    #[serde(default)]
    proxy_state: String,
    proxy_reachable: Option<bool>,
    #[serde(default)]
    proxy_endpoints: Vec<String>,
    #[serde(default)]
    proxy_conflict: bool,
    #[serde(default)]
    sandbox_forwarder_count: u32,
    #[serde(default)]
    sandbox_forwarder_expected_count: u32,
    #[serde(default)]
    sandbox_forwarder_topology_state: String,
    #[serde(default)]
    sandbox_http_forwarder_count: u32,
    #[serde(default)]
    sandbox_socks_forwarder_count: u32,
    #[serde(default)]
    sandbox_probe_identity: String,
    #[serde(default)]
    sandbox_probe_role: String,
    #[serde(default)]
    sandbox_probe_transport: String,
    #[serde(default)]
    claude_process_state: String,
    #[serde(default)]
    claude_wait_channel: String,
    #[serde(default)]
    claude_io_blocked: bool,
    #[serde(default)]
    claude_mount_io_blocked: bool,
    #[serde(default)]
    sandbox_unix_socket_state: String,
    #[serde(default)]
    sandbox_socks_handshake_state: String,
    #[serde(default)]
    sandbox_egress_failure_stage: String,
    #[serde(default)]
    sandbox_probe_daemon_state: String,
    #[serde(default)]
    sandbox_probe_daemon_wait_channel: String,
    #[serde(default)]
    sandbox_probe_daemon_io_blocked: bool,
    #[serde(default)]
    sandbox_probe_daemon_mount_io_blocked: bool,
    #[serde(default)]
    deep_checked: bool,
    deep_checked_at_unix: Option<u64>,
    #[serde(default)]
    sandbox_contract_stable_during_probe: Option<bool>,
    #[serde(default)]
    sandbox_egress_state: String,
    sandbox_egress_target: Option<String>,
    sandbox_egress_canary_identity: Option<String>,
    sandbox_egress_http_status: Option<u16>,
    #[serde(default)]
    sandbox_forwarder_probe_count: u32,
    #[serde(default)]
    sandbox_forwarder_passed_count: u32,
    #[serde(default)]
    sandbox_forwarder_failed_count: u32,
}

#[derive(Debug, Default, Deserialize)]
struct WslProbeHostAccess {
    #[serde(default)]
    preferences_present: bool,
    #[serde(default)]
    preferences_parse_ok: bool,
    #[serde(default)]
    drvfs_write_grant_count: u32,
    #[serde(default)]
    drvfs_write_grants: Vec<String>,
    #[serde(default)]
    broad_drvfs_write_grant_count: u32,
}

fn sandbox_forwarder_topology_is_ready(network: &WslProbeNetwork) -> bool {
    network.sandbox_forwarder_expected_count >= 3
        && network.sandbox_forwarder_count >= network.sandbox_forwarder_expected_count
        && network.sandbox_http_forwarder_count >= network.sandbox_forwarder_expected_count
        && network.sandbox_socks_forwarder_count >= network.sandbox_forwarder_expected_count
        && matches!(
            network.sandbox_forwarder_topology_state.as_str(),
            "expected" | "extended"
        )
        && network.sandbox_probe_identity == SANDBOX_NETWORK_PROBE_IDENTITY
        && network.sandbox_probe_role == "analysis"
        && network.sandbox_probe_transport == "socks5h"
        && network.sandbox_egress_target.as_deref() == Some("pypi.org")
        && network.sandbox_egress_canary_identity.as_deref()
            == Some(SANDBOX_NETWORK_CANARY_IDENTITY)
}

fn sandbox_deep_egress_is_ready(network: &WslProbeNetwork) -> bool {
    network.sandbox_contract_stable_during_probe == Some(true)
        && network.sandbox_unix_socket_state == "connected"
        && network.sandbox_socks_handshake_state == "ok"
        && network.sandbox_egress_state == "ok"
        && network.sandbox_egress_failure_stage == "none"
        && network.sandbox_forwarder_probe_count == 1
        && network.sandbox_forwarder_passed_count == 1
        && network.sandbox_forwarder_failed_count == 0
        && matches!(network.sandbox_egress_http_status, Some(status) if (200..300).contains(&status))
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

fn deserialize_optional_ui_skin<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::String(value) => Some(value),
        // A present-but-invalid value is different from a legacy missing field:
        // it falls back to console instead of reopening first-run selection.
        _ => Some("console".into()),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LauncherSettings {
    selected_provider_id: String,
    custom_base_url: String,
    custom_confirmed: bool,
    #[serde(default, deserialize_with = "deserialize_optional_ui_skin")]
    ui_skin: Option<String>,
    #[serde(default)]
    active_api_key_id: Option<String>,
    #[serde(default)]
    api_keys: Vec<StoredApiKey>,
    #[serde(default)]
    active_role: Option<String>,
    #[serde(default)]
    role_bindings: Vec<StoredRoleBinding>,
    #[serde(default)]
    active_aggregate_scheme_id: Option<String>,
    #[serde(default)]
    aggregate_schemes: Vec<StoredAggregateScheme>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct StoredRoleBinding {
    role: String,
    provider_id: String,
    api_key_id: String,
    model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct StoredAggregateScheme {
    id: String,
    name: String,
    routes: Vec<StoredRoleBinding>,
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
    active_role: Option<String>,
    role_bindings: Vec<StoredRoleBinding>,
    active_aggregate_scheme_id: Option<String>,
    aggregate_schemes: Vec<StoredAggregateScheme>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct UiPreferences {
    skin: Option<String>,
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

#[derive(Debug, Clone)]
struct AggregateRuntimeRoute {
    role: String,
    backend: String,
    api_key: String,
    base_url: String,
    upstream_mode: String,
    model: String,
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
            ui_skin: None,
            active_api_key_id: None,
            api_keys: Vec::new(),
            active_role: None,
            role_bindings: Vec::new(),
            active_aggregate_scheme_id: None,
            aggregate_schemes: Vec::new(),
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
    discover_distros_with_timeout(Duration::from_secs(5))
}

fn discover_distros_with_timeout(timeout: Duration) -> Result<Vec<String>, String> {
    let mut command = background_command("wsl.exe");
    command.args(["--list", "--quiet"]);
    let output = command_output_with_timeout(command, timeout, "WSL 发行版检查")?;
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

fn wsl_bash_stdin_command(distro: &str) -> Command {
    let mut command = background_command("wsl.exe");
    command
        .arg("--distribution")
        .arg(distro)
        .arg("--")
        .args(["bash", "-s"]);
    command
}

fn run_wsl_bash_stdin_with_timeout(
    distro: &str,
    script: &str,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    command_output_with_stdin_timeout(
        wsl_bash_stdin_command(distro),
        script.as_bytes(),
        timeout,
        label,
    )
}

fn run_wsl_default_user_script_with_guest_timeout(
    distro: &str,
    script: &str,
    host_timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    let guest_timeout = host_timeout
        .checked_sub(CLAUDE_OPEN_GUEST_RESERVE)
        .filter(|value| *value >= Duration::from_millis(200))
        .ok_or_else(|| "login.open_timeout".to_string())?;
    let guest_seconds = format!(
        "{}.{:03}s",
        guest_timeout.as_secs(),
        guest_timeout.subsec_millis()
    );
    let mut command = background_command("wsl.exe");
    command
        .arg("--distribution")
        .arg(distro)
        .arg("--")
        .args(["timeout", "--signal=TERM", "--kill-after=0.2s"])
        .arg(guest_seconds)
        .args(["bash", "-s"]);
    command_output_with_stdin_timeout(command, script.as_bytes(), host_timeout, label)
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

fn windows_path_to_wsl_result(distro: &str, path: &Path) -> Result<String, String> {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let output = run_wsl(distro, &["wslpath", "-a", &normalized])?;
    if !output.status.success() {
        return Err(format!(
            "wslpath failed: {}",
            clean_diagnostic_text(&command_error_text(&output))
        ));
    }
    output_text(&output)
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with('/'))
        .map(ToOwned::to_owned)
        .ok_or_else(|| "wslpath did not return an absolute Linux path".to_string())
}

fn windows_path_to_wsl(distro: &str, path: &Path) -> Option<String> {
    windows_path_to_wsl_result(distro, path).ok()
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
    let output =
        match command_output_with_timeout(command, Duration::from_secs(5), "WSL 存储位置检查")
        {
            Ok(output) => output,
            Err(error) => {
                return WindowsStorageSnapshot {
                    diagnostic: Some(grade_probe_diagnostic("storage_snapshot", &error)),
                    ..WindowsStorageSnapshot::default()
                };
            }
        };
    let Ok(data) = serde_json::from_str::<serde_json::Value>(&output_text(&output)) else {
        return WindowsStorageSnapshot {
            diagnostic: Some(
                "grade.storage_snapshot.failed: Windows storage probe returned invalid JSON".into(),
            ),
            ..WindowsStorageSnapshot::default()
        };
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
        diagnostic: None,
    }
}

fn grade_probe_diagnostic(probe: &str, error: &str) -> String {
    let suffix = if error.to_ascii_lowercase().contains("timeout")
        || error.contains("超时")
        || error.contains("没有响应")
    {
        "timeout"
    } else {
        "failed"
    };
    format!("grade.{probe}.{suffix}: {error}")
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

fn inspect_wsl_runtime(
    distro: &str,
    project_wsl: &str,
    deep_network_probe: bool,
) -> Result<WslProbeReport, String> {
    let inspect_script = format!(
        "{}/skills/bootstrap-claude-science-wsl/scripts/inspect-wsl.sh",
        project_wsl.trim_end_matches('/')
    );
    let project_env = format!("PROJECT_DIR={project_wsl}");
    let mut owned_args = vec![
        "env".to_string(),
        project_env,
        "PROXY_PORT=9876".to_string(),
    ];
    if deep_network_probe {
        owned_args.push("CSA_DEEP_NETWORK_PROBE=1".to_string());
        owned_args.push("CSA_WRITE_NETWORK_CACHE=1".to_string());
    }
    owned_args.extend(["bash".to_string(), inspect_script, project_wsl.to_string()]);
    let args = owned_args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = run_wsl_with_timeout(
        distro,
        &args,
        if deep_network_probe {
            Duration::from_secs(20)
        } else {
            Duration::from_secs(8)
        },
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
    let report: WslProbeReport =
        serde_json::from_str(json).map_err(|error| format!("WSL 体检结果解析失败：{error}"))?;
    if report.schema_version != 1 {
        return Err(format!(
            "WSL 体检 schema 不受支持：{}（期望 1）",
            report.schema_version
        ));
    }
    Ok(report)
}

fn current_status_with_options(deep_network_probe: bool) -> SystemStatus {
    let mut warnings = Vec::new();
    let distros = match discover_distros() {
        Ok(items) => items,
        Err(error) => {
            warnings.push(grade_probe_diagnostic("wsl_discovery", &error));
            return SystemStatus {
                state: "notInstalled".into(),
                wsl_installed: false,
                distro: None,
                linux_user: None,
                bridge_running: false,
                bridge_pid: None,
                claude_running: false,
                claude_pid: None,
                claude_listener_present: false,
                claude_port_8765: false,
                claude_port_8766: false,
                claude_unverified_pid: None,
                bridge_healthy: false,
                bridge_identity: None,
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
                host_access_preferences_present: false,
                host_access_preferences_parse_ok: false,
                drvfs_write_grant_count: 0,
                broad_drvfs_write_grant_count: 0,
                drvfs_write_grants: Vec::new(),
                network: NetworkQualityStatus::default(),
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
            claude_listener_present: false,
            claude_port_8765: false,
            claude_port_8766: false,
            claude_unverified_pid: None,
            bridge_healthy: false,
            bridge_identity: None,
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
            host_access_preferences_present: false,
            host_access_preferences_parse_ok: false,
            drvfs_write_grant_count: 0,
            broad_drvfs_write_grant_count: 0,
            drvfs_write_grants: Vec::new(),
            network: NetworkQualityStatus::default(),
            warnings,
        };
    };

    if !distro.eq_ignore_ascii_case("Ubuntu-24.04") {
        warnings.push(format!("推荐使用 Ubuntu-24.04；当前兼容使用 {}。", distro));
    }
    let windows_storage = windows_storage_snapshot(&distro);
    if let Some(diagnostic) = windows_storage.diagnostic.clone() {
        warnings.push(diagnostic);
    }
    let wsl_storage_free_gb = windows_storage
        .wsl_drive_free_bytes
        .map(rounded_gb_from_bytes);
    let wsl_vhdx_size_gb = windows_storage.vhdx_size_bytes.map(rounded_gb_from_bytes);
    let settings_storage_free_gb = windows_storage
        .settings_drive_free_bytes
        .map(rounded_gb_from_bytes);
    let wsl_on_system_drive = is_windows_system_drive(windows_storage.wsl_drive.as_deref());
    let project_files_present = project_runtime_files_present();
    let probe = project_root()
        .and_then(|root| windows_path_to_wsl_result(&distro, &root))
        .and_then(|path| inspect_wsl_runtime(&distro, &path, deep_network_probe));
    let probe = match probe {
        Ok(probe) => probe,
        Err(error) => {
            let storage_blocked = wsl_storage_free_gb.map(|free| free < 1.0).unwrap_or(false)
                || settings_storage_free_gb
                    .map(|free| free < 1.0)
                    .unwrap_or(false);
            warnings.push(format!(
                "{}。启动器已停止后续探测：若 WSL 本身无响应，请检查宿主盘空间与 VHDX；若体检脚本缺失，请重新解压完整 Release ZIP。",
                grade_probe_diagnostic("wsl_inspection", &error)
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
                claude_listener_present: false,
                claude_port_8765: false,
                claude_port_8766: false,
                claude_unverified_pid: None,
                bridge_healthy: false,
                bridge_identity: None,
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
                host_access_preferences_present: false,
                host_access_preferences_parse_ok: false,
                drvfs_write_grant_count: 0,
                broad_drvfs_write_grant_count: 0,
                drvfs_write_grants: Vec::new(),
                network: NetworkQualityStatus::default(),
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
    let claude_running = claude_runtime_is_running(&probe.runtime);
    let claude_listener_present = claude_listener_is_present(&probe.runtime);
    let unit_matches_project = probe.runtime.unit_matches_project;
    let unit_contract_ok = !probe.wsl.systemd || unit_matches_project == Some(true);
    let proxy_state = if probe.network.proxy_state.trim().is_empty() {
        "unknown".to_string()
    } else {
        probe.network.proxy_state.clone()
    };
    let proxy_contract_ok = matches!(proxy_state.as_str(), "direct" | "reachable");
    let sandbox_forwarders_ready = sandbox_forwarder_topology_is_ready(&probe.network);
    let local_network_ready = claude_running && proxy_contract_ok && sandbox_forwarders_ready;
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let cached_deep_result_fresh = deep_network_result_is_fresh(
        probe.network.deep_checked,
        probe.network.deep_checked_at_unix,
        now_unix,
    );
    // A cached D/p9 observation describes one probe interval.  Once the same
    // live daemon is back in R/S/I, discard that transient quality result
    // instead of combining it with a current do_epoll_wait snapshot for the
    // next 15 minutes.
    let deep_result_fresh = cached_deep_result_fresh
        && (deep_network_probe
            || !transient_deep_daemon_result_recovered(
                probe.network.deep_checked,
                &probe.network.sandbox_egress_state,
                probe.network.sandbox_probe_daemon_io_blocked,
                probe.network.sandbox_probe_daemon_mount_io_blocked,
                &probe.network.claude_process_state,
                probe.network.claude_io_blocked,
            ));
    let network_ready = local_network_ready
        && !probe.network.claude_io_blocked
        && deep_result_fresh
        && sandbox_deep_egress_is_ready(&probe.network);
    let daemon_process_state = if probe.network.claude_process_state.trim().is_empty() {
        if deep_result_fresh && !probe.network.sandbox_probe_daemon_state.trim().is_empty() {
            probe.network.sandbox_probe_daemon_state.clone()
        } else {
            "unknown".into()
        }
    } else {
        probe.network.claude_process_state.clone()
    };
    let daemon_wait_channel = if probe.network.claude_wait_channel.trim().is_empty() {
        if deep_result_fresh
            && !probe
                .network
                .sandbox_probe_daemon_wait_channel
                .trim()
                .is_empty()
        {
            probe.network.sandbox_probe_daemon_wait_channel.clone()
        } else {
            "unknown".into()
        }
    } else {
        probe.network.claude_wait_channel.clone()
    };
    let daemon_io_blocked = probe.network.claude_io_blocked;
    let daemon_mount_io_blocked = probe.network.claude_mount_io_blocked;
    let network = NetworkQualityStatus {
        proxy_state: proxy_state.clone(),
        local_ready: local_network_ready,
        ready: network_ready,
        proxy_reachable: probe.network.proxy_reachable,
        proxy_endpoints: probe.network.proxy_endpoints.clone(),
        proxy_conflict: probe.network.proxy_conflict,
        sandbox_forwarder_count: probe.network.sandbox_forwarder_count,
        sandbox_forwarder_expected_count: probe.network.sandbox_forwarder_expected_count,
        sandbox_forwarder_topology_state: probe.network.sandbox_forwarder_topology_state.clone(),
        sandbox_http_forwarder_count: probe.network.sandbox_http_forwarder_count,
        sandbox_socks_forwarder_count: probe.network.sandbox_socks_forwarder_count,
        sandbox_probe_role: probe.network.sandbox_probe_role.clone(),
        sandbox_probe_transport: probe.network.sandbox_probe_transport.clone(),
        daemon_process_state,
        daemon_wait_channel,
        daemon_io_blocked,
        daemon_mount_io_blocked,
        sandbox_unix_socket_state: if probe.network.sandbox_unix_socket_state.trim().is_empty() {
            "not_checked".into()
        } else {
            probe.network.sandbox_unix_socket_state.clone()
        },
        sandbox_socks_handshake_state: if probe
            .network
            .sandbox_socks_handshake_state
            .trim()
            .is_empty()
        {
            "not_checked".into()
        } else {
            probe.network.sandbox_socks_handshake_state.clone()
        },
        sandbox_egress_failure_stage: if probe
            .network
            .sandbox_egress_failure_stage
            .trim()
            .is_empty()
        {
            "not_checked".into()
        } else {
            probe.network.sandbox_egress_failure_stage.clone()
        },
        deep_checked: deep_result_fresh,
        deep_checked_at_unix: probe.network.deep_checked_at_unix,
        sandbox_egress_state: if probe.network.sandbox_egress_state.trim().is_empty() {
            "not_checked".into()
        } else {
            probe.network.sandbox_egress_state.clone()
        },
        sandbox_egress_target: probe.network.sandbox_egress_target.clone(),
        sandbox_egress_http_status: probe.network.sandbox_egress_http_status,
    };
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
    let restart_blocked = storage_blocked || (claude_running && probe.network.claude_io_blocked);
    let windows_bridge_pid = legacy_windows_bridge_pid();

    if bridge_running && !probe.runtime.bridge_health_responding {
        warnings.push("Bridge process/service/port exists, but health check failed.".into());
    }
    if probe.runtime.bridge_health_responding && !bridge_healthy {
        let expected = "$HOME/.local/share/csa/runtime/bridge/current/proxy.py";
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
    append_claude_runtime_warning(&mut warnings, bridge_healthy, &probe.runtime);
    if unit_matches_project == Some(false) {
        warnings.push("WSL Bridge service does not point to the stable CSA managed runtime; run repair and restart to migrate it.".into());
    }
    if probe.wsl.systemd && unit_matches_project.is_none() {
        warnings.push("WSL systemd is active, but CSA could not verify the Bridge unit identity; unknown service ownership is not treated as healthy.".into());
    }
    if claude_running && matches!(proxy_state.as_str(), "unreachable" | "conflict") {
        let endpoints = if probe.network.proxy_endpoints.is_empty() {
            "unknown proxy endpoint".to_string()
        } else {
            probe.network.proxy_endpoints.join(", ")
        };
        warnings.push(format!(
            "Claude Science inherited an unreachable or conflicting outbound proxy ({endpoints}). Local ports can still look healthy while sandbox/API requests fail with 502. Repair and restart to refresh the daemon environment."
        ));
    } else if claude_running && matches!(proxy_state.as_str(), "invalid" | "unknown") {
        warnings.push(format!(
            "Claude Science outbound proxy contract is {proxy_state}; CSA will not report the runtime as ready until it can be validated."
        ));
    }
    if claude_running && !sandbox_forwarders_ready {
        warnings.push(format!(
            "Claude Science is listening locally, but its owned sandbox HTTP/SOCKS pair topology is not ready ({}/{}, state={}); sandbox egress is not ready.",
            probe.network.sandbox_forwarder_count,
            probe.network.sandbox_forwarder_expected_count,
            probe.network.sandbox_forwarder_topology_state
        ));
    }
    if deep_result_fresh
        && !matches!(
            probe.network.sandbox_egress_state.as_str(),
            "ok" | "not_checked" | "daemon_busy" | "daemon_mount_io_busy"
        )
        && !probe.network.sandbox_probe_daemon_io_blocked
        && !probe.network.sandbox_probe_daemon_mount_io_blocked
    {
        let target = probe
            .network
            .sandbox_egress_target
            .as_deref()
            .unwrap_or("research API canary");
        let status = probe
            .network
            .sandbox_egress_http_status
            .map(|value| format!(", HTTP {value}"))
            .unwrap_or_default();
        warnings.push(format!(
            "Sandbox deep egress quality check to {target} failed at stage {} ({}{}). This does not prevent opening the verified local Claude Science UI; no billable model request was made.",
            probe.network.sandbox_egress_failure_stage,
            probe.network.sandbox_egress_state,
            status
        ));
    }
    if claude_running && probe.network.claude_mount_io_blocked {
        if probe.host_access.preferences_present && !probe.host_access.preferences_parse_ok {
            warnings.push("Claude Science is currently in uninterruptible WSL mount I/O. CSA could not verify the persistent host-access grant scope, so it will not recommend or apply an authorization change. Repair/restart is temporarily blocked; refresh after the current I/O returns.".into());
        } else if probe.host_access.broad_drvfs_write_grant_count > 0 {
            warnings.push("Claude Science is currently in uninterruptible WSL mount I/O, and CSA detected a broad persistent writable Windows grant. Repair/restart is temporarily blocked so CSA does not leave Bridge and daemon in a partial state. Refresh after I/O returns, then convert that grant to read-only or move the hot repository to WSL ext4.".into());
        } else {
            warnings.push("Claude Science is currently in uninterruptible WSL mount I/O. CSA found no broad persistent writable Windows grant, so this may be transient activity from an open Windows-backed workspace or concurrent MCP work. Repair/restart is temporarily blocked; refresh after the current I/O returns.".into());
        }
    } else if claude_running && probe.network.claude_io_blocked {
        warnings.push("Claude Science is currently in uninterruptible I/O. Repair/restart is temporarily blocked; CSA will not signal the daemon or mutate Bridge until the process becomes safely stoppable.".into());
    }

    if probe.host_access.preferences_present && !probe.host_access.preferences_parse_ok {
        warnings.push("Claude Science preferences.json exists but its host-access grants could not be parsed safely. CSA did not walk or modify any granted path.".into());
    }
    if probe.host_access.drvfs_write_grant_count > 0 {
        let grants = probe.host_access.drvfs_write_grants.join(", ");
        warnings.push(format!(
            "Detected {} persistent writable Windows/DrvFS grant(s) ({} standard broad-root match): {}. The first real analysis/MCP sandbox command must still run the upstream Git safety scan. Choose Repair and Restart to preserve read access while converting these grants to read-only with a private backup; keep writable projects on WSL ext4 or grant only a narrow output leaf.",
            probe.host_access.drvfs_write_grant_count,
            probe.host_access.broad_drvfs_write_grant_count,
            if grants.is_empty() { "path details unavailable" } else { grants.as_str() }
        ));
    }

    if let Some(pid) = windows_bridge_pid {
        warnings.push(format!(
            "检测到旧 Windows Bridge（PID {pid}）；请先显式停止旧实例，再启动 WSL 服务，避免形成双 Bridge。"
        ));
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

    let host_access_repair_needed = probe.host_access.preferences_present
        && probe.host_access.preferences_parse_ok
        && probe.host_access.drvfs_write_grant_count > 0;
    let state = classify_system_state(&SystemStateInputs {
        host_access_repair_needed,
        bridge_healthy,
        claude_running,
        unit_contract_ok,
        // Opening the local managed UI depends on the verified Bridge,
        // Claude dual-port topology and local proxy/forwarder contract. The
        // anonymous PyPI canary is a separate quality signal: a stale or
        // unavailable external site must not turn a healthy local runtime into
        // a restart target.
        network_ready: core_runtime_ready_for_ui(
            local_network_ready,
            probe.network.claude_io_blocked,
        ),
        storage_blocked,
        wsl_runtime_writable,
        runtime_ready,
        bridge_running,
        claude_listener_present,
        windows_bridge_present: windows_bridge_pid.is_some(),
    });

    SystemStatus {
        state: state.into(),
        wsl_installed: true,
        distro: Some(distro),
        linux_user,
        bridge_running,
        bridge_pid,
        claude_running,
        claude_pid,
        claude_listener_present,
        claude_port_8765: probe.runtime.port_8765,
        claude_port_8766: probe.runtime.port_8766,
        claude_unverified_pid: probe.runtime.claude_unverified_pid,
        bridge_healthy,
        bridge_identity: probe.runtime.bridge_identity,
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
        host_access_preferences_present: probe.host_access.preferences_present,
        host_access_preferences_parse_ok: probe.host_access.preferences_parse_ok,
        drvfs_write_grant_count: probe.host_access.drvfs_write_grant_count,
        broad_drvfs_write_grant_count: probe.host_access.broad_drvfs_write_grant_count,
        drvfs_write_grants: probe.host_access.drvfs_write_grants,
        network,
        warnings,
    }
}

fn current_status() -> SystemStatus {
    current_status_with_options(false)
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
                && root.join("scripts").join("csa-runtime-layout.sh").is_file()
                && root
                    .join("scripts")
                    .join("csa-network-quality.py")
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
            && ancestor
                .join("scripts")
                .join("csa-runtime-layout.sh")
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

fn normalized_ui_skin(value: Option<&str>) -> Option<String> {
    match value {
        Some("console") => Some("console".into()),
        Some("classic") => Some("classic".into()),
        // An invalid persisted value must never blank the launcher or reopen the
        // first-run chooser forever. Treat it as the safe new-skin fallback.
        Some(_) => Some("console".into()),
        None => None,
    }
}

fn ui_preferences(settings: &LauncherSettings) -> UiPreferences {
    UiPreferences {
        skin: normalized_ui_skin(settings.ui_skin.as_deref()),
    }
}

fn ui_preferences_from_text(text: &str) -> UiPreferences {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return UiPreferences {
            skin: Some("console".into()),
        };
    };
    match value.get("uiSkin") {
        None => UiPreferences { skin: None },
        Some(serde_json::Value::String(value)) => UiPreferences {
            skin: normalized_ui_skin(Some(value)),
        },
        Some(_) => UiPreferences {
            skin: Some("console".into()),
        },
    }
}

fn load_ui_preferences() -> UiPreferences {
    let Ok(path) = settings_path() else {
        return UiPreferences {
            skin: Some("console".into()),
        };
    };
    match fs::read_to_string(path) {
        Ok(text) => ui_preferences_from_text(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => UiPreferences { skin: None },
        Err(_) => UiPreferences {
            skin: Some("console".into()),
        },
    }
}

fn load_settings() -> LauncherSettings {
    let Ok(path) = settings_path() else {
        return LauncherSettings::default();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return LauncherSettings::default();
    };
    let mut settings = serde_json::from_str(&text).unwrap_or_else(|_| LauncherSettings::default());
    normalize_aggregate_schemes(&mut settings);
    settings
}

fn normalize_aggregate_schemes(settings: &mut LauncherSettings) {
    if settings.aggregate_schemes.is_empty() {
        settings.aggregate_schemes = vec![
            StoredAggregateScheme {
                id: "scheme-1".into(),
                name: "方案一".into(),
                routes: settings.role_bindings.clone(),
            },
            StoredAggregateScheme {
                id: "scheme-2".into(),
                name: "方案二".into(),
                routes: Vec::new(),
            },
        ];
    }
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
                active: settings.active_role.is_none()
                    && settings.active_aggregate_scheme_id.is_none()
                    && settings.active_api_key_id.as_deref() == Some(entry.id.as_str()),
            })
            .collect(),
        active_role: settings.active_role.clone(),
        role_bindings: settings.role_bindings.clone(),
        active_aggregate_scheme_id: settings.active_aggregate_scheme_id.clone(),
        aggregate_schemes: settings.aggregate_schemes.clone(),
    }
}

fn role_exists(role: &str) -> bool {
    SUBSCRIPTION_ROLES.contains(&role)
}

fn available_models_for_api_key(entry: &StoredApiKey) -> Vec<String> {
    let mut models = Vec::new();
    for model in std::iter::once(entry.model.as_str())
        .chain(entry.model_aliases.iter().map(|alias| alias.model.as_str()))
    {
        let model = model.trim();
        if !model.is_empty() && !models.iter().any(|item| item == model) {
            models.push(model.to_string());
        }
    }
    models
}

fn validate_role_bindings(
    settings: &LauncherSettings,
    bindings: &[StoredRoleBinding],
) -> Result<Vec<StoredRoleBinding>, String> {
    if bindings.len() > SUBSCRIPTION_ROLES.len() {
        return Err("角色映射最多只能包含决策、视觉和日常三项".into());
    }
    let mut normalized = Vec::new();
    for binding in bindings {
        let role = binding.role.trim().to_ascii_lowercase();
        if !role_exists(&role) {
            return Err(format!("未知订阅角色：{}", binding.role));
        }
        if normalized
            .iter()
            .any(|item: &StoredRoleBinding| item.role == role)
        {
            return Err(format!("订阅角色重复：{role}"));
        }
        let entry = settings
            .api_keys
            .iter()
            .find(|entry| entry.id == binding.api_key_id)
            .ok_or_else(|| format!("{role} 角色绑定的 API Key 已不存在"))?;
        if entry.provider_id != binding.provider_id {
            return Err(format!("{role} 角色的 Provider 与 API Key 不匹配"));
        }
        let model = binding.model.trim();
        if model.is_empty() {
            return Err(format!("{role} 角色尚未选择模型"));
        }
        if !available_models_for_api_key(entry)
            .iter()
            .any(|item| item == model)
        {
            return Err(format!("{role} 角色选择的模型不属于该订阅"));
        }
        normalized.push(StoredRoleBinding {
            role,
            provider_id: entry.provider_id.clone(),
            api_key_id: entry.id.clone(),
            model: model.to_string(),
        });
    }
    normalized.sort_by_key(|binding| {
        SUBSCRIPTION_ROLES
            .iter()
            .position(|role| *role == binding.role)
            .unwrap_or(SUBSCRIPTION_ROLES.len())
    });
    Ok(normalized)
}

#[cfg(test)]
fn aliases_for_role(entry: &StoredApiKey, model: &str) -> Vec<StoredModelAlias> {
    clean_model_aliases(&entry.model_aliases)
        .into_iter()
        .map(|mut alias| {
            alias.model = model.to_string();
            alias.display_name = format!("{} -> {model}", alias.id);
            alias
        })
        .collect()
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

fn run_powershell_with_stdin_timeout(
    script: &str,
    input: &str,
    timeout: Duration,
    label: &str,
) -> Result<String, String> {
    let script = format!(
        "$utf8=New-Object System.Text.UTF8Encoding($false); [Console]::InputEncoding=$utf8; [Console]::OutputEncoding=$utf8; $OutputEncoding=$utf8; {script}"
    );
    let mut child = background_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{label}启动失败：{error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("{label}无法打开标准输入"))?;
    stdin
        .write_all(input.as_bytes())
        .map_err(|error| format!("{label}无法写入标准输入：{error}"))?;
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
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!(
                    "{label}在 {} 秒内没有响应，已停止本次操作。",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("{label}状态读取失败：{error}"));
            }
        }
    };
    let output = Output {
        status,
        stdout: stdout_reader.join().unwrap_or_default(),
        stderr: stderr_reader.join().unwrap_or_default(),
    };
    if !output.status.success() {
        return Err(format!("{label}失败：{}", command_error_text(&output)));
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
        return Err("接入名称不能包含控制字符".into());
    }
    if trimmed.chars().count() > 80 {
        return Err("接入名称不能超过 80 个字符".into());
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

fn label_for_provider(
    settings: &LauncherSettings,
    provider: &ProviderPreset,
    requested_name: &str,
) -> Result<String, String> {
    let requested_name = validate_display_name(requested_name)?;
    if !requested_name.is_empty() {
        return Ok(requested_name);
    }
    if provider.id == "custom" {
        return custom_relay_label(settings, "");
    }
    Ok(provider.name.clone())
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
    let parsed = tauri::Url::parse(trimmed).map_err(|_| "自定义中转地址格式无效".to_string())?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("自定义中转地址不能包含账号、密码、查询参数或片段".into());
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
    patch.insert("aggregate_upstreams".into(), serde_json::json!([]));
    patch.insert("active_aggregate_scheme_id".into(), "".into());

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

fn bridge_config_patch_for_aggregate_routes(
    scheme_id: &str,
    routes: &[AggregateRuntimeRoute],
) -> Result<serde_json::Value, String> {
    if routes.len() != SUBSCRIPTION_ROLES.len() {
        return Err("聚合方案必须同时包含决策、视觉和日常三个路由".into());
    }
    let mut patch = serde_json::Map::new();
    patch.insert("default_backend".into(), "custom".into());
    patch.insert("force_model".into(), "".into());
    patch.insert("deepseek_api_key".into(), "".into());
    patch.insert("openai_api_key".into(), "".into());
    patch.insert("custom_api_key".into(), "".into());
    patch.insert("model_list_mode".into(), "aliases".into());
    patch.insert("active_aggregate_scheme_id".into(), scheme_id.into());

    let upstreams = routes
        .iter()
        .map(|route| {
            serde_json::json!({
                "id": route.role,
                "backend": route.backend,
                "api_key": route.api_key,
                "base_url": route.base_url,
                "mode": route.upstream_mode,
                "model": route.model,
            })
        })
        .collect::<Vec<_>>();
    patch.insert("aggregate_upstreams".into(), upstreams.into());

    let mut aliases = Vec::new();
    for route in routes {
        let ids: &[(&str, &str)] = match route.role.as_str() {
            "default" => &[
                ("byok-model-0001", "Opus · 决策"),
                ("claude-opus-4-8", "Opus 4.8 · 决策"),
            ],
            "vision" => &[
                ("claude-sonnet-5", "Sonnet 5 · 视觉"),
                ("claude-sonnet-4-5", "Sonnet 4.5 · 视觉"),
            ],
            "fast" => &[("claude-haiku-4-5-20251001", "Haiku 4.5 · 日常")],
            _ => return Err(format!("未知聚合角色：{}", route.role)),
        };
        aliases.extend(ids.iter().map(|(id, display_name)| {
            serde_json::json!({
                "id": id,
                "display_name": display_name,
                "backend": route.backend,
                "route_id": route.role,
                "model": route.model,
            })
        }));
    }
    patch.insert("model_aliases".into(), aliases.into());
    Ok(serde_json::Value::Object(patch))
}

const BRIDGE_CONFIG_ENVELOPE_LIMIT: usize = 8 * 1024 * 1024;

fn bridge_config_stdin_envelope<T: Serialize>(
    operation: &str,
    payload: &T,
) -> Result<Vec<u8>, String> {
    if !matches!(operation, "patch" | "rollback") {
        return Err("Bridge 配置事务 operation 无效".into());
    }
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 1,
        "operation": operation,
        "payload": payload,
    }))
    .map_err(|error| format!("无法序列化 Bridge 配置事务：{error}"))?;
    if bytes.len() > BRIDGE_CONFIG_ENVELOPE_LIMIT {
        return Err("Bridge 配置事务超过安全大小限制".into());
    }
    Ok(bytes)
}

fn wsl_python_stdin_command(distro: &str, script: &str) -> Command {
    let mut command = background_command("wsl.exe");
    command
        .arg("--distribution")
        .arg(distro)
        .arg("--")
        .args(["timeout", "--signal=TERM", "--kill-after=1s", "9s"])
        .args(["python3", "-c", script]);
    command
}

fn run_wsl_bridge_config_envelope<T: Serialize>(
    distro: &str,
    operation: &str,
    script: &str,
    payload: &T,
) -> Result<Output, String> {
    let input = bridge_config_stdin_envelope(operation, payload)?;
    command_output_with_stdin_timeout(
        wsl_python_stdin_command(distro, script),
        &input,
        Duration::from_secs(12),
        "Bridge 配置事务",
    )
}

fn write_bridge_config_patch(
    distro: &str,
    patch: &serde_json::Value,
) -> Result<BridgeConfigRollback, String> {
    let script = r#"
import json
import os
import pathlib
import sys
import tempfile

limit = 8 * 1024 * 1024
raw = sys.stdin.buffer.read(limit + 1)
if len(raw) > limit:
    raise SystemExit("Bridge config patch envelope exceeds the bounded limit")
try:
    envelope = json.loads(raw.decode("utf-8"))
except Exception:
    raise SystemExit("Bridge config patch envelope is invalid")
if (not isinstance(envelope, dict)
        or set(envelope) != {"schemaVersion", "operation", "payload"}
        or envelope.get("schemaVersion") != 1
        or envelope.get("operation") != "patch"
        or not isinstance(envelope.get("payload"), dict)):
    raise SystemExit("Bridge config patch envelope is invalid")
patch = envelope["payload"]
path = pathlib.Path.home() / ".claude-science" / "proxy" / "config.json"
path.parent.mkdir(parents=True, exist_ok=True)
if path.exists():
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        raise SystemExit("Existing Bridge config is unreadable; refusing to replace it")
    if not isinstance(data, dict):
        raise SystemExit("Existing Bridge config is not an object; refusing to replace it")
else:
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
    let output = run_wsl_bridge_config_envelope(distro, "patch", script, patch)?;
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
    let script = r#"
import json
import os
import pathlib
import sys
import tempfile

limit = 8 * 1024 * 1024
raw = sys.stdin.buffer.read(limit + 1)
if len(raw) > limit:
    raise SystemExit("Bridge config rollback envelope exceeds the bounded limit")
try:
    envelope = json.loads(raw.decode("utf-8"))
except Exception:
    raise SystemExit("Bridge config rollback envelope is invalid")
if (not isinstance(envelope, dict)
        or set(envelope) != {"schemaVersion", "operation", "payload"}
        or envelope.get("schemaVersion") != 1
        or envelope.get("operation") != "rollback"
        or not isinstance(envelope.get("payload"), dict)):
    raise SystemExit("Bridge config rollback envelope is invalid")
rollback = envelope["payload"]
path = pathlib.Path.home() / ".claude-science" / "proxy" / "config.json"
path.parent.mkdir(parents=True, exist_ok=True)
if path.exists():
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        raise SystemExit("Current Bridge config is unreadable; refusing unsafe rollback")
    if not isinstance(data, dict):
        raise SystemExit("Current Bridge config is not an object; refusing unsafe rollback")
else:
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
    let output = run_wsl_bridge_config_envelope(distro, "rollback", script, rollback)?;
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
    let distro = status
        .distro
        .as_deref()
        .ok_or_else(|| "Bridge 配置已写入，但没有可用于生效配置的 WSL 发行版".to_string())?;
    let project_wsl = project_root()
        .ok()
        .and_then(|root| windows_path_to_wsl(distro, &root))
        .ok_or_else(|| "Bridge 配置已写入，但无法定位当前 CSA 的 WSL 启动脚本".to_string())?;
    let start_script = format!(
        "{}/scripts/start-claude-science-wsl.sh",
        project_wsl.trim_end_matches('/')
    );
    let action = if status.bridge_running {
        "restart"
    } else {
        "start"
    };
    eprintln!("[CSA switch] applying Bridge config: action={action}, distro={distro}");
    let guest_timeout = format!("{BRIDGE_RESTART_GUEST_TIMEOUT_SECONDS}s");
    let restart_output = run_wsl_with_timeout(
        distro,
        &[
            "timeout",
            "--signal=TERM",
            "--kill-after=2s",
            &guest_timeout,
            "env",
            "CSA_FORCE_RESTART=1",
            "CSA_BRIDGE_ONLY=1",
            "PROXY_PORT=9876",
            concat!("CSA_PACKAGE_VERSION=", env!("CARGO_PKG_VERSION")),
            "bash",
            &start_script,
        ],
        BRIDGE_RESTART_HOST_TIMEOUT,
    )?;
    if !restart_output.status.success() {
        eprintln!(
            "[CSA switch] Bridge {action} failed: {}",
            command_error_text(&restart_output)
        );
        return Err(format!(
            "Bridge 配置已写入，但重启失败：{}",
            command_error_text(&restart_output)
        ));
    }
    let restart_trace = clean_diagnostic_text(&format!(
        "{}\n{}",
        output_text(&restart_output),
        decode_console_output(&restart_output.stderr)
    ));
    let expected_identity = parse_runtime_identity(&restart_trace)?;
    for line in restart_trace.lines().filter(|line| {
        line.contains("Stopping stale CSA Bridge listener")
            || line.contains("proxy process started (PID")
            || line.contains("Bridge-only restart complete")
    }) {
        eprintln!("[CSA switch] {line}");
    }
    let health_output = run_wsl(
        distro,
        &[
            "curl",
            "--noproxy",
            "127.0.0.1",
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
    let actual_identity = runtime_identity_from_health(&health)?;
    if actual_identity != expected_identity {
        return Err("Bridge 已响应，但运行时身份与本次启动候选不一致".into());
    }
    if let Some(expected_revision) = expected_revision {
        let actual_revision = health
            .get("config_revision")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if actual_revision != expected_revision {
            return Err("Bridge 已响应，但仍未加载刚保存的配置或不是当前 CSA 实例".into());
        }
        eprintln!(
            "[CSA switch] Bridge health verified: revision={expected_revision}, runtime={}",
            actual_identity.runtime_id
        );
    }
    Ok(())
}

fn dashboard_url_from_config(data: &serde_json::Value) -> String {
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
            "http://127.0.0.1:9876/{}/dashboard",
            percent_encode_path_segment(token)
        )
    } else {
        "http://127.0.0.1:9876/dashboard".to_string()
    }
}

fn dashboard_health_command() -> Command {
    let mut command = background_command("curl.exe");
    command.args([
        "--noproxy",
        "127.0.0.1",
        "-fsS",
        "--connect-timeout",
        "0.4",
        "--max-time",
        "2",
        "http://127.0.0.1:9876/health",
    ]);
    command
}

fn dashboard_requires_auth_config(health: &serde_json::Value) -> bool {
    health
        .get("proxy_auth_mode")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|mode| mode.eq_ignore_ascii_case("required"))
        && health
            .get("proxy_auth_configured")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
}

const DASHBOARD_AUTH_CONFIG_SCRIPT: &str = r#"
python3 - <<'PY'
import json
import pathlib
import sys

path = pathlib.Path.home() / ".claude-science" / "proxy" / "config.json"
try:
    data = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}
    if not isinstance(data, dict):
        data = {}
except Exception:
    print("dashboard config unreadable", file=sys.stderr)
    raise SystemExit(1)

allowed = ("proxy_auth_mode", "proxy_auth_token")
print(json.dumps({key: data.get(key) for key in allowed}, ensure_ascii=False))
PY
"#;

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
        return Err(
            "当前诊断不允许写入 API Key/模型配置；请先处理磁盘、WSL、守护进程 I/O 阻塞或安装包问题"
                .into(),
        );
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
    eprintln!("[CSA switch] writing Bridge config: revision={revision}");
    let rollback = write_bridge_config_patch(distro, &patch)?;
    eprintln!("[CSA switch] Bridge config written: revision={revision}");
    match restart_bridge_after_config(&status, Some(&revision)) {
        Ok(()) => Ok(AppliedBridgeConfig {
            distro: distro.to_string(),
            rollback,
            previous_status: status,
        }),
        Err(error) => {
            eprintln!("[CSA switch] activation failed; starting rollback: {error}");
            let rollback_message = match restore_bridge_config(distro, &rollback) {
                Ok(()) => match restart_bridge_after_config(&status, None) {
                    Ok(()) => {
                        eprintln!("[CSA switch] Bridge rollback completed and verified");
                        "已回滚 Bridge 配置并确认运行态".to_string()
                    }
                    Err(rollback_restart_error) => {
                        eprintln!(
                            "[CSA switch] Bridge rollback file restored but runtime verification failed: {rollback_restart_error}"
                        );
                        format!(
                            "Bridge 配置文件已恢复，但回滚重启/运行态确认失败：{rollback_restart_error}"
                        )
                    }
                },
                Err(rollback_error) => {
                    eprintln!("[CSA switch] Bridge rollback failed: {rollback_error}");
                    format!("回滚失败：{rollback_error}")
                }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreflightFailureLayer {
    Connection,
    Authentication,
    Other,
}

fn classify_preflight_failure(message: &str) -> PreflightFailureLayer {
    let normalized = message.to_ascii_lowercase();
    let authentication_markers = [
        "http 401",
        "http 403",
        "401 (unauthorized)",
        "403 (forbidden)",
        "status code 401",
        "status code 403",
        "invalid api key",
        "invalid_api_key",
        "authentication_error",
        "authentication failed",
        "unauthorized",
        "forbidden",
        "(401)",
        "(403)",
        "（401）",
        "（403）",
        "未经授权",
        "禁止访问",
        "无权限",
        "认证失败",
        "身份验证失败",
        "api key 无效",
        "密钥无效",
    ];
    if authentication_markers
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        return PreflightFailureLayer::Authentication;
    }

    let connection_markers = [
        "all connection attempts failed",
        "connection refused",
        "could not connect",
        "unable to connect",
        "failed to connect",
        "connectex",
        "connection timed out",
        "operation timed out",
        "request timed out",
        "timed out",
        "timeout",
        "name resolution",
        "dns",
        "network is unreachable",
        "no route to host",
        "tls handshake",
        "无法连接到远程服务器",
        "连接被拒绝",
        "连接超时",
        "操作超时",
        "请求超时",
        "名称解析",
        "找不到此远程名称",
    ];
    if connection_markers
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        PreflightFailureLayer::Connection
    } else {
        PreflightFailureLayer::Other
    }
}

fn preflight_failure_message(
    raw_message: &str,
    egress_code: Option<&str>,
    outbound_proxy_url: Option<&str>,
) -> String {
    let detail = {
        let cleaned = clean_diagnostic_text(raw_message);
        if cleaned.trim().is_empty() {
            "上游未返回可诊断的错误详情。".to_string()
        } else {
            cleaned
        }
    };
    match classify_preflight_failure(&detail) {
        PreflightFailureLayer::Authentication => format!(
            "认证失败（HTTP 401/403 或上游认证错误）。这是 API Key 或账号权限问题，不是出口代理问题。\n详情：{detail}"
        ),
        PreflightFailureLayer::Connection => {
            if egress_code == Some("work.bridge_egress.proxy_dead") {
                let endpoint = outbound_proxy_url.unwrap_or("当前配置值");
                format!(
                    "上游不可达。\nBridge 的出口代理 {endpoint} 无人监听或不可达（work.bridge_egress.proxy_dead）。\n这不是 Key 的问题 —— 请打开「能力体检」查看出口修复建议。\n详情：{detail}"
                )
            } else {
                let diagnosis = egress_code.unwrap_or("work.bridge_egress.not_available");
                format!(
                    "上游不可达（连接层）。这不是 Key 的问题 —— 请打开「能力体检」检查代理与网络。\n出口只读诊断：{diagnosis}。\n详情：{detail}"
                )
            }
        }
        PreflightFailureLayer::Other => {
            format!("切换前连通性检查失败，请检查 Base URL、模型和服务状态。\n详情：{detail}")
        }
    }
}

#[cfg(test)]
fn attributed_preflight_failure(raw_message: &str) -> String {
    if classify_preflight_failure(raw_message) != PreflightFailureLayer::Connection {
        return preflight_failure_message(raw_message, None, None);
    }
    match run_bridge_egress_connection_probe() {
        Ok(report) => preflight_failure_message(
            raw_message,
            Some(report.code.as_str()),
            report.outbound_proxy_url.as_deref(),
        ),
        Err(error) => preflight_failure_message(raw_message, Some(error.as_str()), None),
    }
}

#[cfg(test)]
fn require_successful_preflight(result: ApiKeyTestResult) -> Result<(), String> {
    if result.ok {
        return Ok(());
    }
    let message = result.message.trim();
    Err(attributed_preflight_failure(message))
}

fn bridge_switch_failure_message(report: &BridgeEgressReport) -> String {
    let status = report.request.http_status;
    if matches!(status, Some(401 | 403)) || report.code == "work.bridge_egress.upstream_401" {
        return preflight_failure_message(
            &format!("HTTP {}: {}", status.unwrap_or(401), report.conclusion),
            Some(report.code.as_str()),
            report.outbound_proxy_url.as_deref(),
        );
    }
    if report.code == "work.bridge_egress.proxy_dead"
        || matches!(
            report.code.as_str(),
            "work.bridge_egress.upstream_502"
                | "work.bridge_egress.upstream_timeout"
                | "work.bridge_egress.upstream_unreachable"
        )
    {
        return preflight_failure_message(
            &format!("All connection attempts failed. {}", report.conclusion),
            Some(report.code.as_str()),
            report.outbound_proxy_url.as_deref(),
        );
    }
    format!(
        "切换后的 Bridge 验证失败（{}）。\n详情：{}",
        report.code,
        clean_diagnostic_text(&report.conclusion)
    )
}

fn verify_bridge_after_api_key_switch() -> Result<(), String> {
    let report = run_bridge_egress_probe(true).map_err(|error| {
        format!(
            "切换后的 Bridge 出口验证无法完成。\n详情：{}",
            clean_diagnostic_text(&error)
        )
    })?;
    if report.ok
        && report.code == "work.bridge_egress.ok"
        && report.request.state == BridgeEgressLayerState::Passed
    {
        Ok(())
    } else {
        Err(bridge_switch_failure_message(&report))
    }
}

fn verify_bridge_after_aggregate_switch() -> Result<(), String> {
    let aliases = [
        "byok-model-0001",
        "claude-sonnet-5",
        "claude-haiku-4-5-20251001",
    ];
    let report = run_bridge_egress_probe_for_models(true, &aliases).map_err(|error| {
        format!(
            "三条聚合路由的切换后 Bridge 验证无法完成。\n详情：{}",
            clean_diagnostic_text(&error)
        )
    })?;
    if !(report.ok
        && report.code == "work.bridge_egress.ok"
        && report.request.state == BridgeEgressLayerState::Passed)
    {
        let role = match report.model.as_deref() {
            Some("byok-model-0001") => "决策路由",
            Some("claude-sonnet-5") => "视觉路由",
            Some("claude-haiku-4-5-20251001") => "日常路由",
            _ => "聚合路由",
        };
        return Err(format!("{role}{}", bridge_switch_failure_message(&report)));
    }
    Ok(())
}

fn reject_known_dead_bridge_proxy() -> Result<(), String> {
    let Ok(report) = run_bridge_egress_connection_probe() else {
        // A stopped Bridge is allowed here: the transaction may start it. The
        // post-commit verifier remains authoritative and will roll back if the
        // managed listener cannot be proven after restart.
        return Ok(());
    };
    if report.code == "work.bridge_egress.proxy_dead" {
        return Err(preflight_failure_message(
            "All connection attempts failed",
            Some(report.code.as_str()),
            report.outbound_proxy_url.as_deref(),
        ));
    }
    Ok(())
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
  if ($baseUrl) { $text = $text.Replace($baseUrl, "[redacted-upstream-url]") }
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
    let output =
        run_powershell_with_stdin_timeout(script, &input, Duration::from_secs(20), "API Key 预检")?;
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
    .map_err(|error| ensure_error_prefix("bridge.api_key_test_failed", error))
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
        return Err("请先填写 API Key，再获取模型列表。".into());
    }
    let provider =
        provider_by_id(&selected_provider_id).ok_or_else(|| "未知 API Key 服务商".to_string())?;
    if provider.trust.starts_with("untrusted") && !custom_confirmed {
        return Err("中转服务需要先确认域名后再获取模型列表，避免 API Key 发到错误地址。".into());
    }
    let Some(profile) =
        runtime_profile_for_provider(&selected_provider_id, &custom_base_url, custom_confirmed)?
    else {
        return Err("Claude 官方登录模式不需要在这里获取模型列表。".into());
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
  if ($baseUrl) { $text = $text.Replace($baseUrl, "[redacted-upstream-url]") }
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
    let output = run_powershell_with_stdin_timeout(
        script,
        &input,
        Duration::from_secs(20),
        "API Key 自动映射",
    )?;
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
    .map_err(|error| ensure_error_prefix("bridge.api_key_auto_map_failed", error))
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

fn commit_launcher_settings_with_bridge_inner(
    settings: &LauncherSettings,
    patch: Option<serde_json::Value>,
    mut verify_bridge: Option<&mut dyn FnMut() -> Result<(), String>>,
) -> Result<(), String> {
    eprintln!("[CSA switch] received configuration transition request");
    let _transition = bridge_config_transition_lock()
        .lock()
        .map_err(|_| "Bridge 配置切换锁异常，请重新启动 CSA 后再试".to_string())?;
    let _service_operation = service_operation_lock("provider-transition")?;
    // Pre-write and fsync Windows settings before touching WSL. This catches a
    // full APPDATA drive without leaving Bridge on a different active Key.
    let prepared_settings = prepare_launcher_settings(settings)?;
    let applied_bridge = match patch {
        Some(patch) => Some(apply_bridge_config_patch_value(patch)?),
        None => None,
    };
    if let Some(verify) = verify_bridge.as_mut() {
        if let Err(verification_error) = verify() {
            let rollback_message = match applied_bridge.as_ref() {
                Some(applied) => match rollback_applied_bridge(applied) {
                    Ok(()) => "Bridge 已恢复到切换前配置".to_string(),
                    Err(error) => format!("Bridge 回滚失败：{error}"),
                },
                None => "Bridge 未发生改动".to_string(),
            };
            return Err(format!("{verification_error}；{rollback_message}"));
        }
    }
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
    eprintln!("[CSA switch] Windows settings committed; transition complete");
    Ok(())
}

fn commit_launcher_settings_with_bridge(
    settings: &LauncherSettings,
    patch: Option<serde_json::Value>,
) -> Result<(), String> {
    commit_launcher_settings_with_bridge_inner(settings, patch, None)
}

fn commit_launcher_settings_with_verified_bridge(
    settings: &LauncherSettings,
    patch: Option<serde_json::Value>,
    verify_bridge: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    commit_launcher_settings_with_bridge_inner(settings, patch, Some(verify_bridge))
}

#[tauri::command]
fn get_provider_catalog() -> Vec<ProviderCatalogGroup> {
    provider_catalog()
}

#[tauri::command]
fn get_launcher_settings() -> LauncherState {
    launcher_state(&load_settings())
}

#[tauri::command]
fn get_ui_preferences() -> UiPreferences {
    load_ui_preferences()
}

fn save_ui_skin_impl(ui_skin: String) -> Result<UiPreferences, String> {
    let ui_skin = match ui_skin.as_str() {
        "console" | "classic" => ui_skin,
        _ => return Err("界面外观只能是 console 或 classic".into()),
    };
    let mut settings = load_settings();
    settings.ui_skin = Some(ui_skin);
    persist_launcher_settings(&settings)?;
    Ok(ui_preferences(&settings))
}

#[tauri::command]
async fn save_ui_skin(ui_skin: String) -> Result<UiPreferences, String> {
    run_blocking(move || save_ui_skin_impl(ui_skin))
        .await
        .map_err(|error| ensure_error_prefix("runtime.ui_skin_save_failed", error))
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
    settings.active_role = None;
    settings.active_aggregate_scheme_id = None;
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
    .map_err(|error| ensure_error_prefix("bridge.provider_save_failed", error))
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
    let mut candidate_settings = settings.clone();
    candidate_settings.selected_provider_id = selected_provider_id.clone();
    candidate_settings.custom_base_url = validated_base_url.clone();
    candidate_settings.custom_confirmed = custom_confirmed;
    let runtime_profile = runtime_profile_for_settings(&candidate_settings)?;
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
    let _validated_patch = bridge_config_patch_for_api_key(
        &candidate_settings,
        clean_key,
        &stored_model,
        &sanitized_aliases,
    )?;
    let label = label_for_provider(&settings, &provider, &display_name)?;
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
    settings.api_keys.push(entry);
    persist_launcher_settings(&settings)?;
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
    .map_err(|error| ensure_error_prefix("bridge.api_key_save_failed", error))
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
    if entry.provider_id != "claude" {
        eprintln!(
            "[CSA switch] managed Bridge verification prepared: provider={}, model={}",
            entry.provider_id, entry.model
        );
        reject_known_dead_bridge_proxy()?;
    }
    settings.selected_provider_id = entry.provider_id.clone();
    settings.custom_base_url = entry.base_url.clone();
    settings.custom_confirmed = entry.custom_confirmed;
    let patch =
        bridge_config_patch_for_api_key(&settings, &api_key, &entry.model, &entry.model_aliases)?;
    settings.active_api_key_id = Some(entry.id.clone());
    settings.active_role = None;
    settings.active_aggregate_scheme_id = None;
    if entry.provider_id == "claude" {
        commit_launcher_settings_with_bridge(&settings, patch)?;
    } else {
        let mut verify = verify_bridge_after_api_key_switch;
        commit_launcher_settings_with_verified_bridge(&settings, patch, &mut verify)?;
        eprintln!("[CSA switch] managed Bridge real request verified");
    }
    Ok(launcher_state(&settings))
}

#[tauri::command]
async fn activate_api_key(api_key_id: String) -> Result<LauncherState, String> {
    run_blocking(move || activate_api_key_impl(api_key_id))
        .await
        .map_err(|error| ensure_error_prefix("bridge.api_key_activate_failed", error))
}

#[cfg(test)]
fn save_role_bindings_impl(role_bindings: Vec<StoredRoleBinding>) -> Result<LauncherState, String> {
    let mut settings = load_settings();
    let normalized = validate_role_bindings(&settings, &role_bindings)?;
    if settings.active_role.is_some() && normalized != settings.role_bindings {
        return Err("请先从 API Key 列表退出当前角色，再修改角色映射".into());
    }
    settings.role_bindings = normalized;
    // Retained only for legacy migration tests; it is not exposed as a Tauri command.
    persist_launcher_settings(&settings)?;
    Ok(launcher_state(&settings))
}

#[cfg(test)]
fn activate_role_in_settings(
    mut settings: LauncherSettings,
    role: String,
) -> Result<LauncherState, String> {
    let role = role.trim().to_ascii_lowercase();
    if !role_exists(&role) {
        return Err("未知订阅角色".into());
    }
    let binding = settings
        .role_bindings
        .iter()
        .find(|binding| binding.role == role)
        .cloned()
        .ok_or_else(|| "请先保存该角色的订阅与模型映射".to_string())?;
    let entry = settings
        .api_keys
        .iter()
        .find(|entry| entry.id == binding.api_key_id)
        .cloned()
        .ok_or_else(|| "该角色绑定的 API Key 已不存在，请重新保存映射".to_string())?;
    if entry.provider_id != binding.provider_id {
        return Err("该角色的 Provider 与 API Key 不匹配，请重新保存映射".into());
    }
    if !available_models_for_api_key(&entry)
        .iter()
        .any(|model| model == &binding.model)
    {
        return Err("该角色绑定的模型已不可用，请重新保存映射".into());
    }
    if entry.provider_id != "claude" && entry.encrypted_api_key.is_empty() {
        return Err("该角色绑定的旧配置没有可用的加密 Key，请重新添加订阅".into());
    }
    let api_key = unprotect_api_key(&entry.encrypted_api_key)?;
    settings.selected_provider_id = entry.provider_id.clone();
    settings.custom_base_url = entry.base_url.clone();
    settings.custom_confirmed = entry.custom_confirmed;
    let role_aliases = aliases_for_role(&entry, &binding.model);
    let patch =
        bridge_config_patch_for_api_key(&settings, &api_key, &binding.model, &role_aliases)?;
    settings.active_api_key_id = Some(entry.id);
    settings.active_role = Some(role);
    settings.active_aggregate_scheme_id = None;
    commit_launcher_settings_with_bridge(&settings, patch)?;
    Ok(launcher_state(&settings))
}

#[cfg(test)]
fn activate_role_impl(role: String) -> Result<LauncherState, String> {
    activate_role_in_settings(load_settings(), role)
}

fn normalize_aggregate_scheme(
    settings: &LauncherSettings,
    scheme: StoredAggregateScheme,
) -> Result<StoredAggregateScheme, String> {
    let id = scheme.id.trim().to_ascii_lowercase();
    if !matches!(id.as_str(), "scheme-1" | "scheme-2") {
        return Err("首版聚合接入只支持方案一和方案二".into());
    }
    let routes = validate_role_bindings(settings, &scheme.routes)?;
    if routes.len() != SUBSCRIPTION_ROLES.len() {
        return Err("请为决策、视觉和日常三个模型槽都选择订阅与模型".into());
    }
    let name = if id == "scheme-1" {
        "方案一"
    } else {
        "方案二"
    };
    Ok(StoredAggregateScheme {
        id,
        name: name.into(),
        routes,
    })
}

fn aggregate_runtime_routes(
    settings: &LauncherSettings,
    routes: &[StoredRoleBinding],
) -> Result<Vec<AggregateRuntimeRoute>, String> {
    let mut runtime_routes = Vec::new();
    for role in SUBSCRIPTION_ROLES {
        let binding = routes
            .iter()
            .find(|binding| binding.role == role)
            .ok_or_else(|| format!("聚合方案缺少 {role} 路由"))?;
        let entry = settings
            .api_keys
            .iter()
            .find(|entry| entry.id == binding.api_key_id)
            .ok_or_else(|| format!("{role} 路由绑定的 API Key 已不存在"))?;
        let api_key = unprotect_api_key(&entry.encrypted_api_key)?;
        if api_key.trim().is_empty() || api_key.contains(char::is_whitespace) {
            return Err(format!("{role} 路由的 API Key 无效，请重新添加该订阅"));
        }
        let mut provider_settings = settings.clone();
        provider_settings.selected_provider_id = entry.provider_id.clone();
        provider_settings.custom_base_url = entry.base_url.clone();
        provider_settings.custom_confirmed = entry.custom_confirmed;
        let profile = runtime_profile_for_settings(&provider_settings)?
            .ok_or_else(|| "聚合接入暂不支持依赖 Claude 官方登录的订阅".to_string())?;
        let model = canonical_model_for_profile(&profile, &binding.model);
        runtime_routes.push(AggregateRuntimeRoute {
            role: role.to_string(),
            backend: profile.backend.to_string(),
            api_key: api_key.trim().to_string(),
            base_url: profile.base_url.clone(),
            upstream_mode: profile.upstream_mode.to_string(),
            model,
        });
    }
    Ok(runtime_routes)
}

fn activate_aggregate_scheme_in_settings(
    mut settings: LauncherSettings,
    scheme: StoredAggregateScheme,
) -> Result<LauncherState, String> {
    let scheme = normalize_aggregate_scheme(&settings, scheme)?;
    reject_known_dead_bridge_proxy()?;
    let runtime_routes = aggregate_runtime_routes(&settings, &scheme.routes)?;
    let patch = Some(bridge_config_patch_for_aggregate_routes(
        &scheme.id,
        &runtime_routes,
    )?);
    if let Some(existing) = settings
        .aggregate_schemes
        .iter_mut()
        .find(|stored| stored.id == scheme.id)
    {
        *existing = scheme.clone();
    } else {
        settings.aggregate_schemes.push(scheme.clone());
    }
    settings.role_bindings = scheme.routes.clone();
    settings.active_role = None;
    settings.active_aggregate_scheme_id = Some(scheme.id);
    // Verify all three aliases through the freshly restarted managed Bridge.
    // Keep Windows settings uncommitted until every max_tokens=1 request passes
    // so one unreachable provider/model rolls the entire Bridge patch back.
    let mut verify = verify_bridge_after_aggregate_switch;
    commit_launcher_settings_with_verified_bridge(&settings, patch, &mut verify)?;
    eprintln!("[CSA switch] all aggregate Bridge routes verified");
    Ok(launcher_state(&settings))
}

fn save_and_activate_aggregate_scheme_impl(
    scheme: StoredAggregateScheme,
) -> Result<LauncherState, String> {
    activate_aggregate_scheme_in_settings(load_settings(), scheme)
}

#[tauri::command]
async fn save_and_activate_aggregate_scheme(
    scheme: StoredAggregateScheme,
) -> Result<LauncherState, String> {
    run_blocking(move || save_and_activate_aggregate_scheme_impl(scheme))
        .await
        .map_err(|error| ensure_error_prefix("bridge.aggregate_save_activate_failed", error))
}

fn activate_aggregate_scheme_impl(scheme_id: String) -> Result<LauncherState, String> {
    let settings = load_settings();
    let scheme = settings
        .aggregate_schemes
        .iter()
        .find(|scheme| scheme.id == scheme_id)
        .cloned()
        .ok_or_else(|| "没有找到该聚合方案".to_string())?;
    activate_aggregate_scheme_in_settings(settings, scheme)
}

#[tauri::command]
async fn activate_aggregate_scheme(scheme_id: String) -> Result<LauncherState, String> {
    run_blocking(move || activate_aggregate_scheme_impl(scheme_id))
        .await
        .map_err(|error| ensure_error_prefix("bridge.aggregate_activate_failed", error))
}

fn delete_api_key_impl(api_key_id: String) -> Result<LauncherState, String> {
    let mut settings = load_settings();
    if settings.active_aggregate_scheme_id.is_none()
        && settings.active_api_key_id.as_deref() == Some(api_key_id.as_str())
    {
        return Err("当前正在使用的 API Key 不能直接删除；请先切换到另一条 Key".into());
    }
    if settings
        .active_aggregate_scheme_id
        .as_deref()
        .is_some_and(|active_id| {
            settings.aggregate_schemes.iter().any(|scheme| {
                scheme.id == active_id
                    && scheme
                        .routes
                        .iter()
                        .any(|route| route.api_key_id == api_key_id)
            })
        })
    {
        return Err("该 API Key 正在被当前聚合方案使用，请先切换到 API 接入或另一套方案".into());
    }
    let before = settings.api_keys.len();
    settings.api_keys.retain(|entry| entry.id != api_key_id);
    if settings.api_keys.len() == before {
        return Err("没有找到这条 API Key".into());
    }
    settings
        .role_bindings
        .retain(|binding| binding.api_key_id != api_key_id);
    for scheme in &mut settings.aggregate_schemes {
        scheme.routes.retain(|route| route.api_key_id != api_key_id);
    }
    if settings.active_role.as_deref().is_some_and(|role| {
        !settings
            .role_bindings
            .iter()
            .any(|binding| binding.role == role)
    }) {
        settings.active_role = None;
    }
    persist_launcher_settings(&settings)?;
    Ok(launcher_state(&settings))
}

fn rename_api_key_in_settings(
    settings: &mut LauncherSettings,
    api_key_id: &str,
    display_name: &str,
) -> Result<(), String> {
    let display_name = validate_display_name(display_name)?;
    if display_name.is_empty() {
        return Err("接入名称不能为空".into());
    }
    let entry = settings
        .api_keys
        .iter_mut()
        .find(|entry| entry.id == api_key_id)
        .ok_or_else(|| "没有找到这条 API Key".to_string())?;
    entry.label = display_name;
    Ok(())
}

fn rename_api_key_impl(api_key_id: String, display_name: String) -> Result<LauncherState, String> {
    let mut settings = load_settings();
    rename_api_key_in_settings(&mut settings, &api_key_id, &display_name)?;
    persist_launcher_settings(&settings)?;
    Ok(launcher_state(&settings))
}

#[tauri::command]
async fn rename_api_key(api_key_id: String, display_name: String) -> Result<LauncherState, String> {
    run_blocking(move || rename_api_key_impl(api_key_id, display_name))
        .await
        .map_err(|error| ensure_error_prefix("bridge.api_key_rename_failed", error))
}

#[tauri::command]
async fn delete_api_key(api_key_id: String) -> Result<LauncherState, String> {
    run_blocking(move || delete_api_key_impl(api_key_id))
        .await
        .map_err(|error| ensure_error_prefix("bridge.api_key_delete_failed", error))
}

#[tauri::command]
async fn get_system_status() -> Result<SystemStatus, String> {
    run_blocking(|| Ok(current_status()))
        .await
        .map_err(|error| ensure_error_prefix("grade.status_failed", error))
}

#[tauri::command]
async fn get_allow_status() -> Result<AllowStatus, String> {
    run_blocking(allow_status_impl)
        .await
        .map_err(|error| ensure_error_prefix("runtime.allow_status_failed", error))
}

#[tauri::command]
async fn get_grade_status() -> Result<GradeStatus, String> {
    run_blocking(|| Ok(grade_status_impl()))
        .await
        .map_err(|error| ensure_error_prefix("grade.status_failed", error))
}

#[tauri::command]
async fn run_network_quality_check() -> Result<WorkReport, String> {
    run_blocking(|| {
        Ok(network_quality_work_report(current_status_with_options(
            true,
        )))
    })
    .await
    .map_err(|error| ensure_error_prefix("work.network_quality_failed", error))
}

fn valid_release_sha8(value: &str) -> bool {
    value.len() == 8
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn checked_release_text(output: Output, max_bytes: usize) -> Result<String, String> {
    if !output.status.success() {
        return Err(clean_diagnostic_text(&command_error_text(&output)));
    }
    if output.stdout.len() > max_bytes {
        return Err("Claude Science 官方版本索引响应过大".into());
    }
    if output.stdout.contains(&0) {
        return Err("Claude Science 官方版本索引包含无效内容".into());
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "Claude Science 官方版本索引不是 UTF-8 文本".to_string())?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Claude Science 官方版本索引为空".into());
    }
    Ok(text)
}

const RUNTIME_UPDATE_BACKEND_BUDGET: Duration = Duration::from_secs(40);

fn runtime_update_deadline_remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            "work.runtime_update.timeout: runtime update probe exceeded its backend deadline"
                .to_string()
        })
}

fn fetch_official_release_text(
    relative_path: &str,
    max_bytes: usize,
    deadline: Instant,
) -> Result<String, String> {
    if relative_path.is_empty()
        || relative_path.starts_with('/')
        || relative_path.contains("..")
        || !relative_path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
    {
        return Err("官方版本索引路径无效".into());
    }

    let url = format!("{CLAUDE_SCIENCE_RELEASE_BASE}/{relative_path}");
    let mut command = background_command("curl.exe");
    command.args([
        "--fail",
        "--silent",
        "--show-error",
        "--proto",
        "=https",
        "--connect-timeout",
        "4",
        "--max-time",
        "12",
        "--max-filesize",
        "65536",
        &url,
    ]);
    let windows_timeout = runtime_update_deadline_remaining(deadline)?.min(Duration::from_secs(15));
    let windows_result = command_output_with_timeout(command, windows_timeout, "检查官方版本")
        .and_then(|output| checked_release_text(output, max_bytes));
    if let Ok(text) = windows_result.as_ref() {
        return Ok(text.clone());
    }

    let distro_timeout = runtime_update_deadline_remaining(deadline)?.min(Duration::from_secs(5));
    let wsl_result = discover_distros_with_timeout(distro_timeout)
        .and_then(|distros| {
            preferred_distro(&distros).ok_or_else(|| "没有可用于版本检查的 WSL 发行版".to_string())
        })
        .and_then(|distro| {
            let wsl_timeout =
                runtime_update_deadline_remaining(deadline)?.min(Duration::from_secs(15));
            run_wsl_with_timeout(
                &distro,
                &[
                    "curl",
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--proto",
                    "=https",
                    "--connect-timeout",
                    "4",
                    "--max-time",
                    "12",
                    "--max-filesize",
                    "65536",
                    &url,
                ],
                wsl_timeout,
            )
        })
        .and_then(|output| checked_release_text(output, max_bytes));
    wsl_result.map_err(|wsl_error| {
        let windows_error = windows_result
            .err()
            .unwrap_or_else(|| "Windows HTTPS 检查失败".into());
        format!(
            "无法读取 Claude Science 官方版本索引（Windows：{windows_error}；WSL：{wsl_error}）"
        )
    })
}

fn linux_x64_sha256(value: &serde_json::Value) -> Option<&str> {
    value
        .get("linux-x64")
        .or_else(|| value.get("linux_x64"))
        .and_then(serde_json::Value::as_str)
}

fn parse_official_release_manifest(
    pointer: &str,
    manifest_text: &str,
) -> Result<RuntimeReleaseSummary, String> {
    if !valid_release_sha8(pointer) {
        return Err("官方版本指针格式无效".into());
    }
    let manifest: OfficialRuntimeManifest = serde_json::from_str(manifest_text)
        .map_err(|error| format!("Claude Science 官方 manifest 解析失败：{error}"))?;
    if manifest.sha8 != pointer {
        return Err("Claude Science 官方 manifest 与版本指针不一致".into());
    }
    if manifest.version.trim().is_empty()
        || !manifest
            .version
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return Err("Claude Science 官方 manifest 版本号无效".into());
    }
    if let Some(checksum) = linux_x64_sha256(&manifest.sha256) {
        if !valid_sha256(checksum) {
            return Err("Claude Science 官方 manifest 的 Linux x64 校验值无效".into());
        }
    }
    Ok(RuntimeReleaseSummary {
        version: manifest.version,
        sha8: manifest.sha8,
        build_date: manifest.build_date,
    })
}

fn release_version_parts(value: &str) -> Option<Vec<u32>> {
    let parts = value
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (!parts.is_empty()).then_some(parts)
}

fn release_is_newer(candidate: &str, current: &str) -> bool {
    let (Some(mut candidate), Some(mut current)) = (
        release_version_parts(candidate),
        release_version_parts(current),
    ) else {
        return false;
    };
    let width = candidate.len().max(current.len());
    candidate.resize(width, 0);
    current.resize(width, 0);
    candidate > current
}

fn get_runtime_update_status_impl() -> Result<RuntimeUpdateStatus, String> {
    let deadline = Instant::now() + RUNTIME_UPDATE_BACKEND_BUDGET;
    let latest_pointer = fetch_official_release_text("latest", 128, deadline)?.to_ascii_lowercase();
    let stable_pointer = fetch_official_release_text("stable", 128, deadline)?.to_ascii_lowercase();
    if !valid_release_sha8(&latest_pointer) || !valid_release_sha8(&stable_pointer) {
        return Err("Claude Science 官方版本指针格式无效".into());
    }

    let latest_manifest = fetch_official_release_text(
        &format!("{latest_pointer}/manifest.json"),
        64 * 1024,
        deadline,
    )?;
    let stable_manifest = if latest_pointer == stable_pointer {
        latest_manifest.clone()
    } else {
        fetch_official_release_text(
            &format!("{stable_pointer}/manifest.json"),
            64 * 1024,
            deadline,
        )?
    };
    let latest = parse_official_release_manifest(&latest_pointer, &latest_manifest)?;
    let stable = parse_official_release_manifest(&stable_pointer, &stable_manifest)?;
    let update_available = release_is_newer(&stable.version, BUNDLED_CLAUDE_SCIENCE_VERSION);

    Ok(RuntimeUpdateStatus {
        bundled_version: BUNDLED_CLAUDE_SCIENCE_VERSION.into(),
        bundled_sha8: BUNDLED_CLAUDE_SCIENCE_SHA8.into(),
        recommended_version: BUNDLED_CLAUDE_SCIENCE_VERSION.into(),
        latest,
        stable,
        update_available,
        checked_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default(),
        release_notes_url: CLAUDE_SCIENCE_CHANGELOG_URL.into(),
        note: "CSA v0.1.6 已验证并锁定 0.1.25；官方更高版本需先由本地 Agent 隔离验证，不会自动覆盖当前运行时。".into(),
    })
}

#[tauri::command]
async fn get_runtime_update_status() -> Result<RuntimeUpdateStatus, String> {
    run_blocking(get_runtime_update_status_impl)
        .await
        .map_err(|error| ensure_error_prefix("work.runtime_update_status_failed", error))
}

// The inner startup can legitimately spend up to ~100 seconds in the
// lifecycle lock, Bridge fallback, Claude health checks and four bounded deep
// probes.  The outer Windows timeout must not cut the WSL transaction in half.
const START_SERVICES_TIMEOUT: Duration = Duration::from_secs(180);
const STOP_SERVICES_TIMEOUT: Duration = Duration::from_secs(60);

fn start_services_raw(
    distro: &str,
    user: &str,
    force_restart: bool,
) -> Result<RuntimeIdentity, String> {
    let script = project_root()
        .map_err(|error| ensure_error_prefix("runtime.package_root_missing", error))?
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
        .arg(user)
        .arg("-PackageVersion")
        .arg(env!("CARGO_PKG_VERSION"));
    if force_restart {
        command.arg("-ForceRestart");
    }
    let output =
        command_output_with_timeout(command, START_SERVICES_TIMEOUT, "Claude Science 启动")
            .map_err(|error| ensure_error_prefix("runtime.start_process_failed", error))?;
    if output.status.success() {
        parse_runtime_identity(&output_text(&output))
            .map_err(|error| ensure_error_prefix("bridge.identity_invalid", error))
    } else {
        Err(format!(
            "runtime.start_process_failed: Claude Science 启动失败：{}",
            command_error_text(&output)
        ))
    }
}

fn should_initialize_core_runtime(status: &SystemStatus) -> bool {
    status.wsl_installed
        && status.runtime_ready
        && !status.claude_running
        && !status.claude_listener_present
        && !status.restart_blocked
        && status.windows_bridge_pid.is_none()
}

fn initialize_runtime_impl() -> Result<SystemStatus, String> {
    let _service_operation = service_operation_lock("initialize-runtime")?;
    let before = current_status();
    if !should_initialize_core_runtime(&before) {
        return Ok(before);
    }

    let distro = before
        .distro
        .ok_or_else(|| "runtime.wsl_distro_missing: 请先安装 WSL2 和 Ubuntu".to_string())?;
    let user = before
        .linux_user
        .ok_or_else(|| "runtime.wsl_user_missing: 无法确定 WSL 默认用户".to_string())?;

    // Claude Science is the primary product service. The transaction is
    // requested first during launcher boot, while start_services_raw keeps the
    // required dependency order: a verified Bridge on 9876 must exist before
    // the daemon that points ANTHROPIC_BASE_URL at that Bridge is spawned.
    start_services_raw(&distro, &user, false)?;
    Ok(current_status())
}

fn ensure_no_legacy_windows_bridge(status: &SystemStatus) -> Result<(), String> {
    if let Some(pid) = status.windows_bridge_pid {
        return Err(format!(
            "runtime.legacy_windows_bridge_present: 检测到旧 Windows Bridge（PID {pid}）。为避免同时运行 Windows/WSL 双 Bridge，请先在诊断区显式停止旧实例，再启动 Claude Science。"
        ));
    }
    Ok(())
}

#[tauri::command]
async fn initialize_runtime() -> Result<SystemStatus, String> {
    run_blocking(initialize_runtime_impl)
        .await
        .map_err(|error| ensure_error_prefix("runtime.initialize_failed", error))
}

fn start_services_impl() -> Result<SystemStatus, String> {
    let _service_operation = service_operation_lock("start-services")?;
    let before = current_status();
    ensure_no_legacy_windows_bridge(&before)?;
    if before.state == "running" {
        return Ok(before);
    }
    if before.restart_blocked {
        return Err(format!(
            "runtime.start_blocked: 当前诊断不允许自动启动（{}）。请先检查磁盘空间、WSL 状态、守护进程 I/O 阻塞和安装包完整性；CSA 不会停止 Bridge、关闭 WSL 或影响无关端口。",
            before
                .wsl_storage_path
                .as_deref()
                .unwrap_or("WSL 虚拟磁盘位置未知")
        ));
    }
    let distro = before
        .distro
        .ok_or_else(|| "runtime.wsl_distro_missing: 请先安装 WSL2 和 Ubuntu".to_string())?;
    let user = before
        .linux_user
        .ok_or_else(|| "runtime.wsl_user_missing: 无法确定 WSL 默认用户".to_string())?;
    start_services_raw(&distro, &user, false)?;
    Ok(current_status())
}

#[tauri::command]
async fn start_services() -> Result<SystemStatus, String> {
    run_blocking(start_services_impl)
        .await
        .map_err(|error| ensure_error_prefix("runtime.start_failed", error))
}

const STOP_SERVICES_SCRIPT: &str = r#"
set -u
state_root="${CSA_STATE_ROOT:-$HOME/.local/share/csa}"
legacy_root="$HOME/.local/share/claude-science-api-bridge"
lifecycle_lock="$state_root/runtime/lifecycle.lock"

if ! command -v flock >/dev/null 2>&1; then
  echo "CSA lifecycle lock requires flock (util-linux); refusing an unlocked stop." >&2
  exit 1
fi
if ! mkdir -p "$(dirname "$lifecycle_lock")"; then
  echo "Cannot create CSA lifecycle lock directory; refusing an unlocked stop." >&2
  exit 1
fi
if ! exec 9>"$lifecycle_lock"; then
  echo "Cannot open CSA lifecycle lock; refusing an unlocked stop." >&2
  exit 1
fi
if ! flock -w "${CSA_LIFECYCLE_LOCK_TIMEOUT:-8}" 9; then
  echo "Another CSA lifecycle operation owns $lifecycle_lock; wait for it to finish and retry." >&2
  exit 1
fi

listener_pids() {
  local port="$1"
  ss -ltnp "sport = :$port" 2>/dev/null \
    | grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u
}

managed_claude_pid() {
  local pid="$1" executable raw_executable
  local -a argv=()
  [ -r "/proc/$pid/cmdline" ] || return 1
  raw_executable="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
  executable="${raw_executable% (deleted)}"
  mapfile -d '' -t argv <"/proc/$pid/cmdline" 2>/dev/null || true
  case "$executable" in
    "$state_root"/runtime/claude-science/patched/*/claude-science)
      [ "${argv[0]:-}" = "$executable" ] \
        || [ "${argv[0]:-}" = "$state_root/runtime/claude-science/patched-current/claude-science" ] \
        || return 1
      ;;
    "$legacy_root"/patched/claude-science)
      [ "${argv[0]:-}" = "$executable" ] || return 1
      ;;
    *) return 1;;
  esac
  [ "${argv[1]:-}" = "serve" ]
}

process_start_ticks() {
  local pid="$1" payload suffix
  payload="$(<"/proc/$pid/stat")" 2>/dev/null || return 1
  suffix="${payload##*) }"
  set -- $suffix
  case "${20:-}" in ''|*[!0-9]*) return 1;; esac
  printf '%s\n' "${20}"
}

process_threads_signalable() {
  local pid="$1" task_dir payload suffix state count=0
  for task_dir in "/proc/$pid"/task/[0-9]*; do
    [ -d "$task_dir" ] || continue
    payload="$(<"$task_dir/stat")" 2>/dev/null || return 1
    suffix="${payload##*) }"
    state="${suffix%% *}"
    case "$state" in R|S|I) ;; *) return 1;; esac
    count=$((count + 1))
  done
  [ "$count" -gt 0 ]
}

managed_claude_signal_token() {
  local pid="$1" before after
  before="$(process_start_ticks "$pid")" || return 1
  managed_claude_pid "$pid" || return 1
  process_threads_signalable "$pid" || return 1
  after="$(process_start_ticks "$pid")" || return 1
  [ "$before" = "$after" ] || return 1
  printf '%s:%s\n' "$pid" "$before"
}

verified_bridge_pid() {
  local pid="$1" payload
  command -v python3 >/dev/null 2>&1 || return 1
  payload="$(curl --noproxy '*' -fsS --connect-timeout 0.4 --max-time 1 \
    http://127.0.0.1:9876/health 2>/dev/null)" || return 1
  python3 - "$pid" "$state_root" "$legacy_root" "$payload" <<'PY' >/dev/null 2>&1
import hashlib, json, os, sys
pid = int(sys.argv[1])
root = os.path.realpath(sys.argv[2])
legacy = os.path.realpath(sys.argv[3])
health = json.loads(sys.argv[4])
identity = health.get("runtime_identity") or {}
source = os.path.realpath(str(identity.get("sourcePath") or health.get("source_path") or ""))
managed_valid = (
    health.get("status") == "ok"
    and identity.get("schemaVersion") == 1
    and identity.get("component") == "bridge"
    and identity.get("managed") is True
    and identity.get("pid") == pid
    and source.startswith(os.path.join(root, "runtime", "bridge", "versions") + os.sep)
    and os.path.isfile(source)
)
if managed_valid:
    with open(source, "rb") as handle:
        managed_valid = hashlib.sha256(handle.read()).hexdigest().casefold() == str(identity.get("sourceSha256") or "").casefold()
legacy_valid = False
if not identity and health.get("status") == "ok" and os.path.isfile(source):
    try:
        command = [item.decode(errors="replace") for item in open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0") if item]
        package_root = os.path.dirname(source)
        manifest = json.load(open(os.path.join(package_root, "manifest.json"), encoding="utf-8"))
        legacy_valid = (
            bool(command)
            and os.path.normpath(command[0]).startswith(os.path.join(legacy, "venv") + os.sep)
            and any(os.path.realpath(item) == source for item in command if item.endswith("proxy.py"))
            and manifest.get("schemaVersion") == 1
            and manifest.get("product") == "CSA - Claude Science Assistant"
            and manifest.get("profile") in {"release", "debug"}
            and "proxy.py" in (manifest.get("expectedRootFiles") or [])
            and os.path.isfile(os.path.join(package_root, "requirements.txt"))
            and os.path.isfile(os.path.join(package_root, "setup-token.py"))
        )
    except (OSError, ValueError, TypeError):
        legacy_valid = False
raise SystemExit(0 if managed_valid or legacy_valid else 1)
PY
}

bridge_health_source() {
  local payload
  command -v python3 >/dev/null 2>&1 || return 1
  payload="$(curl --noproxy '*' -fsS --connect-timeout 0.4 --max-time 1 \
    http://127.0.0.1:9876/health 2>/dev/null)" || return 1
  python3 - "$payload" <<'PY'
import json, os, sys
try:
    health = json.loads(sys.argv[1])
except (ValueError, TypeError):
    raise SystemExit(1)
identity = health.get("runtime_identity") or {}
source = os.path.realpath(str(identity.get("sourcePath") or health.get("source_path") or ""))
if not source:
    raise SystemExit(1)
print(source)
PY
}

bridge_unit_property() {
  local property="$1"
  systemctl --user show claude-science-bridge.service \
    --property="$property" --value 2>/dev/null || true
}

bridge_unit_matches_managed_runtime() {
  local exec_start environment
  exec_start="$(bridge_unit_property ExecStart)"
  environment="$(bridge_unit_property Environment)"
  [[ "$exec_start" == *"$state_root/runtime/bridge/current/proxy.py"* ]] \
    && [[ " $environment " == *" CSA_BRIDGE_MANAGED=1 "* ]] \
    && [[ " $environment " == *" PROXY_PORT=9876 "* ]]
}

bridge_unit_matches_verified_listener() {
  local exec_start source
  [ -n "$bridge_pids" ] || return 1
  source="$(bridge_health_source)" || return 1
  exec_start="$(bridge_unit_property ExecStart)"
  [ -n "$exec_start" ] && [[ "$exec_start" == *"$source"* ]]
}

claude_pids="$(for port in 8765 8766; do listener_pids "$port"; done | sort -u)"
bridge_pids="$(listener_pids 9876)"
claude_tokens=""
for pid in $claude_pids; do
  token="$(managed_claude_signal_token "$pid" || true)"
  [ -n "$token" ] || {
    echo "Refusing to stop Claude Science PID $pid: owner/starttime is unverified or a thread is in D/T/Z/unknown state. Bridge and WSL were left unchanged." >&2
    exit 1
  }
  claude_tokens="${claude_tokens}${token}"$'\n'
done
for pid in $bridge_pids; do
  verified_bridge_pid "$pid" || {
    echo "Refusing to stop unverified owner PID $pid on Bridge port 9876." >&2
    exit 1
  }
done

bridge_unit_owned=0
if bridge_unit_matches_managed_runtime || bridge_unit_matches_verified_listener; then
  bridge_unit_owned=1
fi
bridge_unit_active_state="$(bridge_unit_property ActiveState)"
case "$bridge_unit_active_state" in
  active|activating|reloading|deactivating)
    if [ "$bridge_unit_owned" != "1" ]; then
      echo "Refusing to stop claude-science-bridge.service because its unit identity is not owned by this CSA runtime." >&2
      exit 1
    fi
    ;;
esac

# Stop Claude Science completely before mutating Bridge, so a process that
# becomes uninterruptible after TERM cannot leave the runtime half-stopped.
for token in $claude_tokens; do
  pid="${token%%:*}"
  [ "$(managed_claude_signal_token "$pid" || true)" = "$token" ] || {
    echo "Claude Science identity/state changed before TERM; Bridge was left unchanged." >&2
    exit 1
  }
  kill "$pid" 2>/dev/null || true
done
grace_deadline=$((SECONDS + 4))
while [ "$SECONDS" -lt "$grace_deadline" ]; do
  remaining=0
  for pid in $claude_pids; do
    if kill -0 "$pid" 2>/dev/null; then
      remaining=1
      break
    fi
  done
  [ "$remaining" = "0" ] && break
  sleep 0.25
done
for token in $claude_tokens; do
  pid="${token%%:*}"
  if kill -0 "$pid" 2>/dev/null; then
    [ "$(managed_claude_signal_token "$pid" || true)" = "$token" ] || {
      echo "Claude Science became unsafe to signal after TERM; Bridge was left unchanged." >&2
      exit 1
    }
    kill -9 "$pid" 2>/dev/null || true
  fi
done
claude_deadline=$((SECONDS + 5))
while [ "$SECONDS" -lt "$claude_deadline" ]; do
  ss -ltn 2>/dev/null | grep -qE ':(8765|8766) ' || break
  sleep 0.25
done
if ss -ltn 2>/dev/null | grep -qE ':(8765|8766) '; then
  echo "Claude Science did not stop safely; Bridge and WSL were left unchanged." >&2
  exit 1
fi

if [ "$bridge_unit_owned" = "1" ]; then
  systemctl --user stop claude-science-bridge.service >/dev/null 2>&1 || true
fi
for pid in $bridge_pids; do
  verified_bridge_pid "$pid" && kill "$pid" 2>/dev/null || true
done
deadline=$((SECONDS + 5))
while [ "$SECONDS" -lt "$deadline" ]; do
  if ! curl --noproxy '*' -fsS --connect-timeout 0.3 --max-time 0.6 http://127.0.0.1:9876/health >/dev/null 2>&1 \
    && ! ss -ltn 2>/dev/null | grep -qE ':(8765|8766) '; then
    exit 0
  fi
  sleep 0.25
done
echo "CSA services did not stop within 5 seconds." >&2
exit 1
"#;

fn stop_services_raw(distro: &str) -> Result<(), String> {
    let output = run_wsl_bash_stdin_with_timeout(
        distro,
        STOP_SERVICES_SCRIPT,
        STOP_SERVICES_TIMEOUT,
        "停止 CSA 服务",
    )
    .map_err(|error| ensure_error_prefix("transport.stop_services_failed", error))?;
    if !output.status.success() {
        return Err(format!(
            "runtime.stop_process_failed: 停止服务失败：{}",
            command_error_text(&output)
        ));
    }
    Ok(())
}

fn stop_services_impl() -> Result<SystemStatus, String> {
    let _service_operation = service_operation_lock("stop-services")?;
    let before = current_status();
    let Some(distro) = before.distro else {
        return Ok(before);
    };
    stop_services_raw(&distro)?;
    Ok(current_status())
}

#[tauri::command]
async fn stop_services() -> Result<SystemStatus, String> {
    run_blocking(stop_services_impl)
        .await
        .map_err(|error| ensure_error_prefix("runtime.stop_failed", error))
}

fn restart_services_impl() -> Result<SystemStatus, String> {
    let _service_operation = service_operation_lock("restart-services")?;
    let before = current_status();
    ensure_no_legacy_windows_bridge(&before)?;
    if before.restart_blocked {
        return Err("work.restart_blocked: 当前诊断不允许自动重启；可能是磁盘空间不足、WSL 只读/无响应、Claude Science 正处于不可中断 I/O，或安装包不完整。现有服务、WSL 和无关端口不会被停止。".into());
    }
    let distro = before
        .distro
        .ok_or_else(|| "runtime.wsl_distro_missing: 请先安装 WSL2 和 Ubuntu".to_string())?;
    let user = before
        .linux_user
        .ok_or_else(|| "runtime.wsl_user_missing: 无法确定 WSL 默认用户".to_string())?;
    start_services_raw(&distro, &user, true)?;
    Ok(current_status())
}

#[tauri::command]
async fn restart_services() -> Result<SystemStatus, String> {
    run_blocking(restart_services_impl)
        .await
        .map_err(|error| ensure_error_prefix("work.restart_failed", error))
}

fn selected_distro_quick() -> Result<String, String> {
    let distros = discover_distros()?;
    preferred_distro(&distros).ok_or_else(|| "WSL 不可用".to_string())
}

const CLAUDE_URL_SHELL: &str = r#"
set -u
state_root="${CSA_STATE_ROOT:-$HOME/.local/share/csa}"
lifecycle_lock="$state_root/runtime/lifecycle.lock"

if ! command -v flock >/dev/null 2>&1; then
  echo "CSA_URL_FLOCK_MISSING" >&2
  exit 70
fi
if ! command -v timeout >/dev/null 2>&1; then
  echo "CSA_URL_TIMEOUT_MISSING" >&2
  exit 70
fi
if [ -f "$lifecycle_lock" ]; then
  if ! exec 9<>"$lifecycle_lock"; then
    echo "CSA_URL_LIFECYCLE_LOCK_UNREADABLE" >&2
    exit 74
  fi
  if ! flock -w 0.25 9; then
    echo "CSA_URL_LIFECYCLE_BUSY" >&2
    exit 75
  fi
else
  # Pre-v0.1.6 managed runtimes did not always create this lock. The Windows
  # cross-process mutex still serializes launcher operations; keep the legacy
  # URL path usable and rely on the bounded control-channel retries below.
  echo "CSA_URL_LEGACY_NO_LIFECYCLE_LOCK" >&2
fi

legacy_root="$HOME/.local/share/claude-science-api-bridge"

listener_pids() {
  local port="$1"
  command -v ss >/dev/null 2>&1 || return 1
  ss -ltnp "sport = :$port" 2>/dev/null \
    | grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u
}

single_listener_pid() {
  local port="$1" pids
  pids="$(listener_pids "$port")" || return 1
  set -- $pids
  [ "$#" -eq 1 ] || return 1
  case "$1" in ''|*[!0-9]*) return 1;; esac
  printf '%s\n' "$1"
}

process_start_ticks() {
  local pid="$1" payload suffix
  payload="$(<"/proc/$pid/stat")" 2>/dev/null || return 1
  suffix="${payload##*) }"
  set -- $suffix
  case "${20:-}" in ''|*[!0-9]*) return 1;; esac
  printf '%s\n' "${20}"
}

# Return 76 only for uninterruptible I/O. All other unknown/unsafe scheduler
# states fail closed as an unverifiable identity instead of being signalled.
process_threads_safe() {
  local pid="$1" task_dir payload suffix state count=0
  for task_dir in "/proc/$pid"/task/[0-9]*; do
    [ -d "$task_dir" ] || continue
    payload="$(<"$task_dir/stat")" 2>/dev/null || return 1
    suffix="${payload##*) }"
    state="${suffix%% *}"
    case "$state" in
      R|S|I) ;;
      D) return 76 ;;
      *) return 1 ;;
    esac
    count=$((count + 1))
  done
  [ "$count" -gt 0 ]
}

managed_claude_executable() {
  local pid="$1" executable raw_executable argv0_real
  local -a argv=()
  [ -r "/proc/$pid/cmdline" ] || return 1
  raw_executable="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
  case "$raw_executable" in ''|*' (deleted)') return 1;; esac
  executable="$raw_executable"
  mapfile -d '' -t argv <"/proc/$pid/cmdline" 2>/dev/null || return 1
  [ "${#argv[@]}" -ge 2 ] || return 1
  case "$executable" in
    "$state_root"/runtime/claude-science/patched/*/claude-science) ;;
    "$legacy_root"/patched/claude-science) ;;
    *) return 1 ;;
  esac
  case "${argv[0]:-}" in
    "$state_root"/runtime/claude-science/patched/*/claude-science|"$state_root"/runtime/claude-science/patched-current/claude-science|"$legacy_root"/patched/claude-science) ;;
    *) return 1 ;;
  esac
  argv0_real="$(readlink -f -- "${argv[0]}" 2>/dev/null || true)"
  [ "$argv0_real" = "$executable" ] || return 1
  [ "${argv[1]:-}" = "serve" ] || return 1
  [ -x "$executable" ] || return 1
  printf '%s\n' "$executable"
}

identity_failure() {
  echo "CSA_URL_DAEMON_IDENTITY_UNVERIFIED" >&2
  exit 77
}

threads_failure() {
  local pid="$1" rc
  process_threads_safe "$pid"
  rc=$?
  if [ "$rc" -eq 76 ]; then
    echo "CSA_URL_DAEMON_IO_BLOCKED" >&2
    exit 76
  fi
  [ "$rc" -eq 0 ] || identity_failure
}

# The lock file is vendor state, not an identity root. Establish ownership from
# both listeners plus /proc, and only then cross-check a lock PID when present.
daemon_pid_8765="$(single_listener_pid 8765)" || identity_failure
daemon_pid_8766="$(single_listener_pid 8766)" || identity_failure
[ "$daemon_pid_8765" = "$daemon_pid_8766" ] || identity_failure
daemon_pid="$daemon_pid_8765"
daemon_start="$(process_start_ticks "$daemon_pid")" || identity_failure
bin="$(managed_claude_executable "$daemon_pid")" || identity_failure
threads_failure "$daemon_pid"
[ "$(process_start_ticks "$daemon_pid" 2>/dev/null || true)" = "$daemon_start" ] || identity_failure

operon_lock="$HOME/.claude-science/operon.lock"
if [ -e "$operon_lock" ]; then
  [ -r "$operon_lock" ] || identity_failure
  lock_pid="$(grep -oE '"pid"[[:space:]]*:[[:space:]]*[0-9]+' "$operon_lock" 2>/dev/null \
    | head -n 1 | grep -oE '[0-9]+$' || true)"
  [ -n "$lock_pid" ] && [ "$lock_pid" = "$daemon_pid" ] || identity_failure
fi

verify_daemon_identity() {
  local pid_8765 pid_8766 current_start current_bin thread_rc
  pid_8765="$(single_listener_pid 8765)" || return 77
  pid_8766="$(single_listener_pid 8766)" || return 77
  [ "$pid_8765" = "$daemon_pid" ] && [ "$pid_8766" = "$daemon_pid" ] || return 77
  current_start="$(process_start_ticks "$daemon_pid")" || return 77
  [ "$current_start" = "$daemon_start" ] || return 77
  current_bin="$(managed_claude_executable "$daemon_pid")" || return 77
  [ "$current_bin" = "$bin" ] || return 77
  process_threads_safe "$daemon_pid"
  thread_rc=$?
  [ "$thread_rc" -eq 0 ] || return "$thread_rc"
  [ "$(process_start_ticks "$daemon_pid" 2>/dev/null || true)" = "$daemon_start" ] || return 77
}

attempt=1
last_rc=4
while [ "$attempt" -le 3 ]; do
  verify_daemon_identity
  identity_rc=$?
  if [ "$identity_rc" -eq 76 ]; then
    echo "CSA_URL_DAEMON_IO_BLOCKED" >&2
    exit 76
  elif [ "$identity_rc" -ne 0 ]; then
    identity_failure
  fi
  # Resolve through the already verified live process instead of reopening a
  # mutable filesystem path. Keep the nonce in-process until the same daemon
  # identity is revalidated after the control call.
  url_output="$(timeout --signal=TERM --kill-after=0.1s 0.9s "/proc/$daemon_pid/exe" url)"
  last_rc=$?
  if [ "$last_rc" -eq 0 ]; then
    verify_daemon_identity
    identity_rc=$?
    if [ "$identity_rc" -eq 76 ]; then
      echo "CSA_URL_DAEMON_IO_BLOCKED" >&2
      exit 76
    elif [ "$identity_rc" -ne 0 ]; then
      identity_failure
    fi
    printf '%s\n' "$url_output"
    exit 0
  fi
  case "$last_rc" in
    # Claude Science 0.1.25 can briefly return 2 while its daemon generation,
    # lock file, and control socket converge. This is not the launcher's own
    # runtime-missing condition, which always carries an explicit sentinel.
    1|2|4) ;;
    *)
      echo "CSA_URL_COMMAND_FAILED=$last_rc" >&2
      exit "$last_rc"
      ;;
  esac
  if [ "$attempt" -lt 3 ]; then
    echo "CSA_URL_TRANSIENT_RETRY=$last_rc" >&2
    if [ "$attempt" -eq 1 ]; then sleep 0.15; else sleep 0.30; fi
  fi
  attempt=$((attempt + 1))
done

if [ "$last_rc" -eq 1 ]; then
  echo "CSA_URL_CONTROL_UNAVAILABLE" >&2
elif [ "$last_rc" -eq 2 ]; then
  echo "CSA_URL_DAEMON_TRANSITION" >&2
else
  echo "CSA_URL_DAEMON_NOT_READY" >&2
fi
exit "$last_rc"
"#;

fn safe_claude_loopback_url(output: &str) -> Result<String, String> {
    let mut saw_url = false;
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if !(line.starts_with("http://") || line.starts_with("https://")) {
            continue;
        }
        saw_url = true;
        let Ok(url) = tauri::Url::parse(line) else {
            continue;
        };
        let host = url.host_str().unwrap_or_default();
        let host_without_brackets = host.trim_start_matches('[').trim_end_matches(']');
        let loopback_host = host_without_brackets.eq_ignore_ascii_case("localhost")
            || host_without_brackets
                .parse::<std::net::IpAddr>()
                .map(|address| address.is_loopback())
                .unwrap_or(false);
        let has_nonce = url
            .query_pairs()
            .any(|(key, value)| key == "nonce" && !value.is_empty());
        if url.scheme() == "http"
            && loopback_host
            && url.port() == Some(8765)
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.fragment().is_none()
            && has_nonce
        {
            return Ok(line.to_string());
        }
    }
    if saw_url {
        Err("login.untrusted_url: Claude Science 返回了非受信任的登录地址；CSA 已拒绝打开。".into())
    } else {
        Err("login.url_missing: Claude Science 登录控制通道没有返回可打开的本机地址。".into())
    }
}

fn safe_claude_url_error_detail(stderr: &str) -> Option<String> {
    let cleaned = clean_diagnostic_text(stderr);
    let mut lines = Vec::new();
    for line in cleaned
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if line.starts_with("CSA_URL_") {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.contains("nonce")
            || lower.contains("token")
            || lower.contains("secret")
            || lower.contains("api_key")
            || lower.contains("apikey")
        {
            lines.push("<敏感诊断已隐藏>".to_string());
            continue;
        }
        let redacted = line
            .split_whitespace()
            .map(|word| {
                if word.starts_with("http://") || word.starts_with("https://") {
                    "<地址已隐藏>"
                } else {
                    word
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(redacted);
    }
    let mut detail = lines.join("；");
    if detail.chars().count() > 400 {
        detail = detail.chars().take(400).collect::<String>();
        detail.push('…');
    }
    (!detail.trim().is_empty()).then_some(detail)
}

fn claude_url_result(exit_code: Option<i32>, stdout: &str, stderr: &str) -> Result<String, String> {
    if exit_code == Some(0) {
        return safe_claude_loopback_url(stdout);
    }

    let cleaned_stderr = clean_diagnostic_text(stderr);
    // Only classify launcher-owned failures by their explicit sentinel. The
    // vendor CLI also uses small exit codes (including 2), so raw-code mapping
    // creates false "runtime missing" diagnostics while the binary is present.
    let base = if cleaned_stderr.contains("CSA_URL_RUNTIME_MISSING") {
        "runtime.entry_missing: Claude Science 受管运行时入口缺失；请从完整 V0.1.6 便携包执行修复。"
    } else if cleaned_stderr.contains("CSA_URL_LIFECYCLE_BUSY") {
        "login.lifecycle_busy: Claude Science 仍在启动或重启；生命周期锁当前正忙，请稍后重试。"
    } else if cleaned_stderr.contains("CSA_URL_FLOCK_MISSING") {
        "runtime.flock_missing: WSL 缺少 CSA 生命周期锁工具 flock，无法安全等待服务切换。"
    } else if cleaned_stderr.contains("CSA_URL_TIMEOUT_MISSING") {
        "runtime.timeout_missing: WSL 缺少 timeout，无法为登录预检建立硬期限。"
    } else if cleaned_stderr.contains("CSA_URL_LIFECYCLE_LOCK_UNREADABLE") {
        "runtime.lifecycle_lock_unreadable: CSA 生命周期锁不可读写，无法安全生成 Claude Science 登录地址。"
    } else if cleaned_stderr.contains("CSA_URL_DAEMON_IDENTITY_UNVERIFIED") {
        "login.daemon_identity_unverified: Claude Science 的 8765/8766 端口未通过同一受管进程身份校验；CSA 已拒绝向未知本地服务发送登录命令。"
    } else if cleaned_stderr.contains("CSA_URL_DAEMON_IO_BLOCKED") {
        "login.daemon_io_blocked: Claude Science 当前处于不可中断 WSL I/O；本地端口仍可能监听，但登录入口暂时不能安全生成。"
    } else if cleaned_stderr.contains("CSA_URL_CONTROL_UNAVAILABLE") {
        "login.control_unavailable: Claude Science 端口已出现，但登录控制通道暂时没有响应；CSA 已完成两次退避重试。"
    } else if cleaned_stderr.contains("CSA_URL_DAEMON_TRANSITION") {
        "login.daemon_transition: Claude Science 运行时入口存在，但登录命令在两次退避后仍处于 daemon 切换状态；这不是运行时缺失。"
    } else if cleaned_stderr.contains("CSA_URL_DAEMON_NOT_READY") {
        "login.daemon_not_ready: Claude Science 当前没有稳定的锁文件或 control socket，服务可能仍在切换。"
    } else {
        "login.url_command_failed: Claude Science 登录地址生成失败。"
    };
    if let Some(code) = exit_code {
        if let Some(detail) = safe_claude_url_error_detail(stderr) {
            Err(format!("{base}（退出码 {code}）诊断：{detail}"))
        } else {
            Err(format!("{base}（退出码 {code}）"))
        }
    } else if let Some(detail) = safe_claude_url_error_detail(stderr) {
        Err(format!("{base} 诊断：{detail}"))
    } else {
        Err(base.into())
    }
}

fn open_deadline_remaining(deadline: Instant) -> Result<Duration, String> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err("login.open_timeout".into())
    } else {
        Ok(remaining)
    }
}

fn get_claude_url_impl() -> Result<String, String> {
    let deadline = Instant::now()
        .checked_add(CLAUDE_OPEN_HARD_BUDGET)
        .ok_or_else(|| "login.open_timeout".to_string())?;
    let lock_timeout = open_deadline_remaining(deadline)?.min(CLAUDE_OPEN_LOCK_BUDGET);
    let _service_operation = service_operation_quick_lock("open-claude-science", lock_timeout)?;
    // Do not put the full storage/network/status inspection on the login
    // critical path. The WSL script below verifies the lifecycle lock,
    // managed executable and daemon scheduler state in one bounded operation.
    let distro_timeout = open_deadline_remaining(deadline)?.min(CLAUDE_OPEN_DISTRO_BUDGET);
    let distros = discover_distros_with_timeout(distro_timeout)
        .map_err(|_| "transport.wsl_distro_probe".to_string())?;
    let distro =
        preferred_distro(&distros).ok_or_else(|| "runtime.wsl_distro_missing".to_string())?;
    let host_timeout = open_deadline_remaining(deadline)?
        .checked_sub(CLAUDE_OPEN_CLEANUP_RESERVE)
        .filter(|value| *value >= Duration::from_millis(500))
        .ok_or_else(|| "login.open_timeout".to_string())?;
    let output = run_wsl_default_user_script_with_guest_timeout(
        &distro,
        CLAUDE_URL_SHELL,
        host_timeout,
        "Claude Science 登录地址生成",
    )
    .map_err(|error| {
        if error.contains("login.open_timeout") || error.contains("没有响应") {
            "login.open_timeout".to_string()
        } else {
            "transport.claude_url_failed".to_string()
        }
    })?;
    claude_url_result(
        output.status.code(),
        &decode_console_output(&output.stdout),
        &decode_console_output(&output.stderr),
    )
}

#[tauri::command]
async fn open_claude_science(app: tauri::AppHandle) -> Result<(), String> {
    let url = run_blocking(get_claude_url_impl)
        .await
        .map_err(|error| ensure_error_prefix("login.open_failed", error))?;
    app.opener().open_url(url, None::<&str>).map_err(|_| {
        "login.browser_open_failed: 已生成本机登录地址，但 Windows 无法打开默认浏览器；请检查默认浏览器关联后重试。"
            .to_string()
    })
}

fn get_dashboard_url_impl() -> Result<String, String> {
    // The dashboard is opened by Windows, so verify the exact Windows-facing
    // loopback route. Sending `--noproxy *` through wsl.exe is unsafe here:
    // WSL can re-parse the argv and expand `*` in the current DrvFS directory,
    // making curl treat workspace entries as URLs until the host watchdog fires.
    let health_output = command_output_with_timeout(
        dashboard_health_command(),
        Duration::from_secs(4),
        "Bridge 配置面板健康检查",
    )?;
    if !health_output.status.success() {
        let detail = command_error_text(&health_output);
        return Err(if detail.trim().is_empty() {
            "Windows 无法访问当前 Bridge 配置面板，请先刷新状态".to_string()
        } else {
            format!("Windows 无法访问当前 Bridge 配置面板：{detail}")
        });
    }
    let health = serde_json::from_str::<serde_json::Value>(&output_text(&health_output))
        .map_err(|_| "当前 Bridge 健康信息无效，请先从本目录启动/迁移 Bridge".to_string())?;
    runtime_identity_from_health(&health)
        .map_err(|_| "9876 端口不是 CSA 受管 Bridge；请先执行修复并重启".to_string())?;

    if !dashboard_requires_auth_config(&health) {
        return Ok(dashboard_url_from_config(&health));
    }

    // Required control authentication is the exceptional path. Read only the
    // four dashboard fields over stdin so neither shell text nor a wildcard is
    // transported in the outer wsl.exe argv, and do not copy unrelated API keys.
    let distro = selected_distro_quick()?;
    let output = run_wsl_bash_stdin_with_timeout(
        &distro,
        DASHBOARD_AUTH_CONFIG_SCRIPT,
        Duration::from_secs(8),
        "Bridge 配置面板认证读取",
    )?;
    if !output.status.success() {
        return Err("无法读取 Bridge 配置，无法打开配置面板".into());
    }
    let text = output_text(&output);
    let data: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({}));
    Ok(dashboard_url_from_config(&data))
}

#[tauri::command]
async fn open_bridge_dashboard(app: tauri::AppHandle) -> Result<(), String> {
    let url = run_blocking(get_dashboard_url_impl)
        .await
        .map_err(|error| ensure_error_prefix("bridge.dashboard_url_failed", error))?;
    app.opener().open_url(url, None::<&str>).map_err(|_| {
        "bridge.dashboard_browser_open_failed: 已验证本机 Bridge 配置面板，但 Windows 无法打开默认浏览器；请检查默认浏览器关联后重试。"
            .to_string()
    })
}

fn stop_legacy_windows_bridge_impl() -> Result<SystemStatus, String> {
    let _service_operation = service_operation_lock("stop-legacy-windows-bridge")?;
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
    run_blocking(stop_legacy_windows_bridge_impl)
        .await
        .map_err(|error| ensure_error_prefix("runtime.legacy_bridge_stop_failed", error))
}

#[tauri::command]
async fn run_bridge_egress_check(confirm_billable: bool) -> Result<BridgeEgressReport, String> {
    run_blocking(move || run_bridge_egress_probe(confirm_billable)).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_allow_status,
            get_grade_status,
            get_system_status,
            initialize_runtime,
            run_network_quality_check,
            get_runtime_update_status,
            start_services,
            stop_services,
            restart_services,
            open_claude_science,
            open_bridge_dashboard,
            stop_legacy_windows_bridge,
            run_bridge_egress_check,
            apply_bridge_egress_fix,
            get_provider_catalog,
            get_launcher_settings,
            get_ui_preferences,
            save_ui_skin,
            save_provider_selection,
            save_api_key,
            activate_api_key,
            activate_aggregate_scheme,
            save_and_activate_aggregate_scheme,
            test_api_key,
            auto_map_api_key,
            rename_api_key,
            delete_api_key
        ])
        .run(tauri::generate_context!())
        .expect("error while running Claude Science Assistant");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_allow_status() -> AllowStatus {
        AllowStatus {
            wsl_installed: true,
            distro: Some("Ubuntu-24.04".into()),
            linux_user: Some("test-user".into()),
            runtime_present: true,
            claude_running: true,
            claude_pid: Some(42),
            listener_present: true,
            pid_8765: Some(42),
            pid_8766: Some(42),
            daemon_state: "managed_ready".into(),
            listener_probe_ok: true,
            control_socket_present: true,
            windows_bridge_pid: None,
            windows_bridge_probe: "checked".into(),
            can_open: true,
            can_start: false,
        }
    }

    fn ts_member_inputs(source: &str, receiver: &str) -> std::collections::BTreeSet<String> {
        let needle = format!("{receiver}.");
        let mut inputs = std::collections::BTreeSet::new();
        let mut remaining = source;
        while let Some(offset) = remaining.find(&needle) {
            let tail = &remaining[offset + needle.len()..];
            let name = tail
                .chars()
                .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
                .collect::<String>();
            if !name.is_empty() {
                inputs.insert(name);
            }
            remaining = tail;
        }
        inputs
    }

    #[test]
    fn allow_inputs_frozen() {
        const TAX_MESSAGE: &str =
            "ALLOW 输入集合被改动。改它要走 §4.4 的税单（改测试 + 改合同 + 反例测试 + 错误前缀）。";
        let contract = include_str!("../../src/laneContract.ts");
        let app = include_str!("../../src/App.tsx");
        let can_open_start = contract
            .find("export const canOpenFromAllow =")
            .expect("canOpenFromAllow should exist");
        let can_open_end = contract[can_open_start..]
            .find("export const primaryLabelFromAllow =")
            .map(|offset| can_open_start + offset)
            .expect("primaryLabelFromAllow should follow canOpenFromAllow");
        let can_open = &contract[can_open_start..can_open_end];
        let actual_can_open_inputs = ts_member_inputs(can_open, "allow");
        let expected_can_open_inputs = ALLOW_OPEN_INPUTS
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            actual_can_open_inputs, expected_can_open_inputs,
            "{TAX_MESSAGE}"
        );
        assert!(
            contract.contains(
                "export const ALLOW_OPEN_INPUTS = [\"claudeRunning\", \"windowsBridgePid\"] as const"
            ),
            "{TAX_MESSAGE}"
        );
        assert!(
            app.contains("() => primaryButtonView(allowStatus, allowLoaded, allowActionBusy)"),
            "产品主路径必须调用冻结后的主按钮选择器。{TAX_MESSAGE}"
        );

        let label_start = can_open_end;
        let label_end = contract[label_start..]
            .find("export type PrimaryAllowAction")
            .map(|offset| label_start + offset)
            .expect("primary action type should follow the label helper");
        let label_helper = &contract[label_start..label_end];
        let mut label_inputs = ts_member_inputs(label_helper, "allow");
        if label_helper.contains("canOpenFromAllow(allow)") {
            label_inputs.extend(actual_can_open_inputs);
        }
        let allow_fields = serde_json::to_value(sample_allow_status())
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            label_inputs.is_subset(&allow_fields),
            "主按钮文案读取了非 ALLOW 字段：{:?}。{TAX_MESSAGE}",
            label_inputs.difference(&allow_fields).collect::<Vec<_>>()
        );
        for label in [
            "打开 Claude Science",
            "启动 Claude Science",
            "先停止旧 Windows Bridge",
            "安装运行环境",
        ] {
            assert_eq!(
                label_helper.matches(&format!("return \"{label}\"")).count(),
                1,
                "主按钮固定文案缺失或重复：{label}。{TAX_MESSAGE}"
            );
        }
        assert_eq!(
            label_helper.matches("return \"").count(),
            4,
            "主按钮文案不再是固定四条。{TAX_MESSAGE}"
        );
        for forbidden in [
            "先处理诊断问题",
            "尚未启动",
            "restartBlocked",
            "status.state",
            "status.network",
        ] {
            assert!(
                !label_helper.contains(forbidden),
                "主按钮文案混入了 GRADE/WORK 输入 {forbidden}。{TAX_MESSAGE}"
            );
        }

        let view_start = contract
            .find("export function primaryButtonView(")
            .expect("primaryButtonView should exist");
        let view_end = contract[view_start..]
            .find("export function nonGatingFailurePresentation(")
            .map(|offset| view_start + offset)
            .expect("non-gating presentation helper should follow primary button selector");
        let view = &contract[view_start..view_end];
        assert!(view.contains("label: primaryLabelFromAllow(allow)"));
        assert!(view.contains("disabled: !allowLoaded || allowActionBusy"));
        for forbidden in [
            "status",
            "network",
            "workWarnings",
            "restartBlocked",
            "busyRef",
        ] {
            assert!(
                !view.contains(forbidden),
                "主按钮选择器混入了 GRADE/WORK/全局忙状态 {forbidden}。{TAX_MESSAGE}"
            );
        }

        assert!(
            app.contains("const primaryButton = useMemo("),
            "产品主路径必须保存冻结选择器的结果。{TAX_MESSAGE}"
        );
        let selector_start = app
            .find("const primaryButton = useMemo(")
            .expect("primary button selector should exist");
        let selector_end = app[selector_start..]
            .find("const mutationBusy")
            .map(|offset| selector_start + offset)
            .expect("mutation busy declaration should follow primary selector");
        let selector = &app[selector_start..selector_end];
        assert!(selector.contains("primaryButtonView(allowStatus, allowLoaded, allowActionBusy)"));
        assert!(selector.contains("const canOpenClaude = primaryButton.action === \"open\""));
        for forbidden in [
            "status",
            "network",
            "workWarnings",
            "restartBlocked",
            "busyRef",
        ] {
            assert!(
                !selector.contains(forbidden),
                "产品主按钮选择路径混入了 GRADE/WORK/全局忙状态 {forbidden}。{TAX_MESSAGE}"
            );
        }

        let button_start = app
            .find("className=\"primary-button\"")
            .expect("primary button should exist");
        let button_end = app[button_start..]
            .find("</button>")
            .map(|offset| button_start + offset)
            .expect("primary button should close");
        let button = &app[button_start..button_end];
        assert!(button.contains("disabled={primaryButton.disabled}"));
        assert!(button.contains("{primaryButton.label}"));
        for forbidden in [
            "status",
            "network",
            "workWarnings",
            "restartBlocked",
            "busy",
            "正在处理",
        ] {
            assert!(
                !button.contains(forbidden),
                "主按钮渲染混入了 GRADE/WORK 输入 {forbidden}。{TAX_MESSAGE}"
            );
        }
    }

    #[test]
    fn status_lane_serialization_keeps_allow_grade_and_work_separate() {
        let allow = serde_json::to_value(sample_allow_status()).unwrap();
        for forbidden in [
            "state",
            "bridgeHealthy",
            "restartBlocked",
            "storageWarning",
            "network",
            "deep",
            "warnings",
        ] {
            assert!(
                allow.get(forbidden).is_none(),
                "ALLOW leaked {forbidden} from GRADE/WORK"
            );
        }

        let work_warning = "Sandbox deep egress quality check to pypi failed".to_string();
        let grade_warning = "Bridge identity mismatch".to_string();
        let grade_value = GradeStatus::from(SystemStatus {
            warnings: vec![work_warning.clone(), grade_warning.clone()],
            ..Default::default()
        });
        assert_eq!(grade_value.warnings, [grade_warning]);
        let grade = serde_json::to_value(grade_value).unwrap();
        for forbidden in [
            "claudeRunning",
            "claudePid",
            "windowsBridgePid",
            "canOpen",
            "canStart",
            "deepChecked",
            "sandboxEgressState",
            "sandboxUnixSocketState",
            "sandboxSocksHandshakeState",
        ] {
            assert!(
                grade.get(forbidden).is_none(),
                "GRADE leaked {forbidden} from ALLOW/WORK"
            );
            assert!(
                grade
                    .get("network")
                    .and_then(|value| value.get(forbidden))
                    .is_none(),
                "GRADE.network leaked WORK field {forbidden}"
            );
        }

        let work = network_quality_work_report(SystemStatus {
            claude_running: true,
            claude_pid: Some(42),
            windows_bridge_pid: Some(7),
            network: NetworkQualityStatus {
                deep_checked: true,
                sandbox_egress_state: "failed".into(),
                ..Default::default()
            },
            warnings: vec![work_warning.clone(), "Bridge identity mismatch".into()],
            ..Default::default()
        });
        assert_eq!(work.warnings, [work_warning]);
        let work = serde_json::to_value(work).unwrap();
        for forbidden in [
            "claudeRunning",
            "claudePid",
            "windowsBridgePid",
            "canOpen",
            "canStart",
            "state",
            "runtimeReady",
            "listenerPresent",
            "pid8765",
            "pid8766",
            "network",
        ] {
            assert!(
                work.get(forbidden).is_none(),
                "WORK leaked {forbidden} from ALLOW/GRADE"
            );
        }
    }

    #[test]
    fn status_commands_and_smoke_share_the_authoritative_implementations() {
        let backend = include_str!("lib.rs");
        let smoke = include_str!("smoke.rs");
        assert!(backend.contains("run_blocking(allow_status_impl)"));
        assert!(backend.contains("Ok(grade_status_impl())"));
        assert!(smoke.contains("allow_status_impl()"));
        assert!(smoke.contains("let status = grade_status_impl();"));
    }

    #[test]
    fn layered_error_prefixes_preserve_known_codes_and_classify_unknown_errors() {
        assert_eq!(
            ensure_error_prefix(
                "runtime.start_failed",
                "login.daemon_not_ready: wait".into()
            ),
            "login.daemon_not_ready: wait"
        );
        assert_eq!(
            ensure_error_prefix("runtime.start_failed", "raw failure".into()),
            "runtime.start_failed: raw failure"
        );
        assert!(classify_service_operation_error(
            "provider-transition",
            "Another CSA service operation owns the lock".into()
        )
        .starts_with("bridge.transition_lock_held:"));
        assert!(classify_service_operation_error(
            "start-services",
            "Another CSA service operation owns the lock".into()
        )
        .starts_with("runtime.lock_held:"));
        assert!(ensure_no_legacy_windows_bridge(&SystemStatus {
            windows_bridge_pid: Some(99),
            ..Default::default()
        })
        .unwrap_err()
        .starts_with("runtime.legacy_windows_bridge_present:"));

        let source = include_str!("lib.rs");
        for boundary in [
            "bridge.api_key_test_failed",
            "bridge.api_key_auto_map_failed",
            "bridge.provider_save_failed",
            "bridge.api_key_save_failed",
            "bridge.api_key_activate_failed",
            "bridge.aggregate_save_activate_failed",
            "bridge.aggregate_activate_failed",
            "bridge.api_key_delete_failed",
            "work.runtime_update_status_failed",
            "bridge.dashboard_url_failed",
        ] {
            assert!(
                source.contains(&format!("ensure_error_prefix(\"{boundary}\"")),
                "user-reachable command is missing its layered error boundary: {boundary}"
            );
        }
    }

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

    #[cfg(windows)]
    #[test]
    fn blocked_stdin_writer_cannot_bypass_the_process_timeout() {
        let mut command = background_command("powershell.exe");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 20",
        ]);
        let input = vec![b'x'; 4 * 1024 * 1024];
        let started = Instant::now();
        let error = command_output_with_stdin_timeout(
            command,
            &input,
            Duration::from_millis(250),
            "stdin timeout regression",
        )
        .unwrap_err();

        assert!(error.contains("没有响应"));
        assert!(started.elapsed() < Duration::from_secs(5));
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
    fn claude_login_url_accepts_only_the_managed_loopback_origin() {
        let nonce = "a".repeat(64);
        for origin in [
            "http://localhost:8765",
            "http://127.0.0.1:8765",
            "http://127.0.0.2:8765",
            "http://[::1]:8765",
        ] {
            let expected = format!("{origin}/?nonce={nonce}");
            let output = format!("startup warning\n  {expected}  \n");
            assert_eq!(safe_claude_loopback_url(&output).unwrap(), expected);
        }

        for candidate in [
            format!("https://127.0.0.1:8765/?nonce={nonce}"),
            format!("http://evil.example:8765/?nonce={nonce}"),
            format!("http://localhost.evil:8765/?nonce={nonce}"),
            format!("http://localhost@evil.example:8765/?nonce={nonce}"),
            format!("http://127.0.0.1:9876/?nonce={nonce}"),
            format!("http://127.0.0.1:8765/login?nonce={nonce}"),
            format!("http://127.0.0.1:8765/?nonce={nonce}#fragment"),
            "http://127.0.0.1:8765/".to_string(),
        ] {
            let error = safe_claude_loopback_url(&candidate).unwrap_err();
            assert!(error.contains("非受信任") || error.contains("没有返回"));
            assert!(!error.contains(&nonce));
        }
    }

    #[test]
    fn claude_login_url_failures_are_classified_without_leaking_nonce() {
        let nonce = "deadbeef".repeat(8);
        let secret_stdout = format!("http://127.0.0.1:8765/?nonce={nonce}\n");
        let control = claude_url_result(
            Some(1),
            &secret_stdout,
            &format!("failed to mint nonce at http://127.0.0.1:8765/?nonce={nonce}\nCSA_URL_CONTROL_UNAVAILABLE\n"),
        )
        .unwrap_err();
        assert!(control.starts_with("login.control_unavailable:"));
        assert!(control.contains("登录控制通道"));
        assert!(!control.contains(&nonce));
        assert!(!control.contains("nonce="));

        let missing =
            claude_url_result(Some(2), &secret_stdout, "CSA_URL_RUNTIME_MISSING\n").unwrap_err();
        assert!(missing.starts_with("runtime.entry_missing:"));
        assert!(missing.contains("运行时入口缺失"));
        assert!(!missing.contains(&nonce));

        let vendor_two =
            claude_url_result(Some(2), &secret_stdout, "CSA_URL_COMMAND_FAILED=2\n").unwrap_err();
        assert!(vendor_two.starts_with("login.url_command_failed:"));
        assert!(vendor_two.contains("登录地址生成失败"));
        assert!(!vendor_two.contains("运行时入口缺失"));
        assert!(!vendor_two.contains(&nonce));

        let transition =
            claude_url_result(Some(2), &secret_stdout, "CSA_URL_DAEMON_TRANSITION\n").unwrap_err();
        assert!(transition.starts_with("login.daemon_transition:"));
        assert!(transition.contains("运行时入口存在"));
        assert!(transition.contains("不是运行时缺失"));
        assert!(!transition.contains(&nonce));

        let io_blocked =
            claude_url_result(Some(76), &secret_stdout, "CSA_URL_DAEMON_IO_BLOCKED\n").unwrap_err();
        assert!(io_blocked.starts_with("login.daemon_io_blocked:"));
        assert!(io_blocked.contains("不可中断 WSL I/O"));
        assert!(!io_blocked.contains(&nonce));

        let unverified = claude_url_result(
            Some(77),
            &secret_stdout,
            "CSA_URL_DAEMON_IDENTITY_UNVERIFIED\n",
        )
        .unwrap_err();
        assert!(unverified.starts_with("login.daemon_identity_unverified:"));
        assert!(unverified.contains("同一受管进程身份校验"));
        assert!(unverified.contains("未知本地服务"));
        assert!(!unverified.contains(&nonce));

        for vendor_code in [70, 74, 75] {
            let vendor_failure = claude_url_result(
                Some(vendor_code),
                &secret_stdout,
                &format!("CSA_URL_COMMAND_FAILED={vendor_code}\n"),
            )
            .unwrap_err();
            assert!(vendor_failure.starts_with("login.url_command_failed:"));
            assert!(vendor_failure.contains("登录地址生成失败"));
            assert!(!vendor_failure.contains("生命周期"));
            assert!(!vendor_failure.contains("flock"));
            assert!(!vendor_failure.contains(&nonce));
        }

        let busy =
            claude_url_result(Some(75), &secret_stdout, "CSA_URL_LIFECYCLE_BUSY\n").unwrap_err();
        assert!(busy.starts_with("login.lifecycle_busy:"));
        assert!(busy.contains("启动或重启"));
        assert!(!busy.contains(&nonce));

        let unknown = claude_url_result(
            Some(9),
            &secret_stdout,
            "provider failed at https://example.invalid/private\n",
        )
        .unwrap_err();
        assert!(unknown.starts_with("login.url_command_failed:"));
        assert!(unknown.contains("退出码 9"));
        assert!(unknown.contains("<地址已隐藏>"));
        assert!(!unknown.contains("example.invalid"));
        assert!(!unknown.contains(&nonce));
    }

    #[test]
    fn claude_login_url_has_one_hard_deadline_and_retries_only_transient_codes() {
        assert!(CLAUDE_OPEN_HARD_BUDGET < Duration::from_secs(8));
        assert!(CLAUDE_OPEN_LOCK_BUDGET <= Duration::from_millis(300));
        assert!(CLAUDE_URL_SHELL.contains("flock -w 0.25 9"));
        assert!(!CLAUDE_URL_SHELL.contains("flock -w 25 9"));
        assert!(CLAUDE_URL_SHELL.contains("CSA_URL_LEGACY_NO_LIFECYCLE_LOCK"));
        assert!(CLAUDE_URL_SHELL.contains("while [ \"$attempt\" -le 3 ]"));
        assert!(CLAUDE_URL_SHELL.contains("timeout --signal=TERM --kill-after=0.1s 0.9s"));
        assert!(CLAUDE_URL_SHELL.contains("1|2|4) ;;"));
        assert!(CLAUDE_URL_SHELL.contains("CSA_URL_DAEMON_TRANSITION"));
        assert!(CLAUDE_URL_SHELL.contains("CSA_URL_DAEMON_IO_BLOCKED"));
        assert!(CLAUDE_URL_SHELL.contains("CSA_URL_DAEMON_IDENTITY_UNVERIFIED"));
        assert!(CLAUDE_URL_SHELL.contains("single_listener_pid 8765"));
        assert!(CLAUDE_URL_SHELL.contains("single_listener_pid 8766"));
        assert!(CLAUDE_URL_SHELL.contains("managed_claude_executable \"$daemon_pid\""));
        assert!(CLAUDE_URL_SHELL.contains("process_start_ticks \"$daemon_pid\""));
        assert!(CLAUDE_URL_SHELL.contains("process_threads_safe \"$daemon_pid\""));
        assert!(CLAUDE_URL_SHELL.contains("[ \"${argv[1]:-}\" = \"serve\" ]"));
        assert!(CLAUDE_URL_SHELL.contains("verify_daemon_identity"));
        assert!(CLAUDE_URL_SHELL.contains("CSA_URL_TRANSIENT_RETRY"));
        assert!(CLAUDE_URL_SHELL.contains("CSA_URL_COMMAND_FAILED"));
        assert!(!CLAUDE_URL_SHELL.contains("echo \"$bin\""));
    }

    #[test]
    fn claude_login_script_is_streamed_to_bash_without_outer_wsl_expansion() {
        let source = include_str!("lib.rs");
        let helper_start = source
            .find("fn run_wsl_default_user_script_with_guest_timeout(")
            .expect("WSL stdin script helper should exist");
        let helper_end = source[helper_start..]
            .find("fn parse_first_pid(")
            .map(|offset| helper_start + offset)
            .expect("PID parser should follow the WSL script helper");
        let helper = &source[helper_start..helper_end];

        assert!(helper.contains(".args([\"bash\", \"-s\"])"));
        assert!(helper.contains("command_output_with_stdin_timeout"));
        assert!(helper.contains("script.as_bytes()"));
        assert!(helper.contains("--kill-after=0.2s"));
        assert!(helper.contains("CLAUDE_OPEN_GUEST_RESERVE"));
        assert!(!helper.contains("\"-lc\""));
    }

    #[test]
    #[ignore = "requires the current user's live managed WSL daemon and mints one unused login URL"]
    fn live_claude_url_stdin_transport_diagnostic() {
        let started = Instant::now();
        let url = get_claude_url_impl()
            .unwrap_or_else(|error| panic!("live stdin URL transport failed: {error}"));
        assert!(started.elapsed() < Duration::from_secs(8));
        assert!(url.starts_with("http://"));
        assert!(url.contains(":8765/?nonce="));
    }

    #[test]
    fn claude_runtime_warning_distinguishes_stopped_partial_and_ready() {
        let mut runtime = WslProbeRuntime::default();
        let mut warnings = Vec::new();
        append_claude_runtime_warning(&mut warnings, false, &runtime);
        assert!(warnings.is_empty());

        append_claude_runtime_warning(&mut warnings, true, &runtime);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("核心服务尚未运行"));

        runtime.port_8765 = true;
        runtime.claude_unverified_pid = Some(41);
        warnings.clear();
        append_claude_runtime_warning(&mut warnings, true, &runtime);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("双端口拓扑尚未就绪"));
        assert!(!warnings[0].contains("核心服务尚未运行"));

        runtime.port_8766 = true;
        warnings.clear();
        append_claude_runtime_warning(&mut warnings, true, &runtime);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("尚未通过同一受管进程身份校验"));

        runtime.claude_unverified_pid = None;
        runtime.claude_pid = Some(41);
        runtime.claude_owner_verified = true;
        warnings.clear();
        append_claude_runtime_warning(&mut warnings, true, &runtime);
        assert!(warnings.is_empty());
    }

    #[test]
    fn bridge_only_is_stopped_but_partial_or_unverified_claude_is_degraded() {
        let windows_only = SystemStateInputs {
            windows_bridge_present: true,
            wsl_runtime_writable: true,
            ..Default::default()
        };
        assert_eq!(classify_system_state(&windows_only), "degraded");

        let mut input = SystemStateInputs {
            bridge_healthy: true,
            bridge_running: true,
            unit_contract_ok: true,
            runtime_ready: true,
            wsl_runtime_writable: true,
            ..Default::default()
        };
        assert_eq!(classify_system_state(&input), "stopped");

        input.runtime_ready = false;
        assert_eq!(classify_system_state(&input), "degraded");
        input.runtime_ready = true;

        input.claude_listener_present = true;
        assert_eq!(classify_system_state(&input), "degraded");

        input.claude_listener_present = false;
        input.windows_bridge_present = true;
        assert_eq!(classify_system_state(&input), "degraded");

        input.windows_bridge_present = false;
        input.claude_running = true;
        input.claude_listener_present = true;
        input.network_ready = true;
        assert_eq!(classify_system_state(&input), "running");
    }

    #[test]
    fn external_deep_probe_does_not_gate_the_local_managed_ui() {
        assert!(core_runtime_ready_for_ui(true, false));
        assert!(!core_runtime_ready_for_ui(false, false));
        assert!(!core_runtime_ready_for_ui(true, true));
    }

    #[test]
    fn failed_deep_inspection_cannot_replace_verified_core_status() {
        let failed_inspection = SystemStatus {
            state: "degraded".into(),
            claude_running: false,
            bridge_healthy: false,
            restart_blocked: true,
            warnings: vec!["WSL deep inspection timed out".into()],
            network: NetworkQualityStatus {
                deep_checked: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = network_quality_work_report(failed_inspection);

        assert!(result.deep.is_none());
        assert_eq!(result.operation, "network_quality");
        assert!(!result.ok);
        assert!(result.warnings.is_empty());
        let serialized = serde_json::to_value(&result).unwrap();
        assert!(serialized.get("deep").is_none());
        assert!(serialized.get("state").is_none());
        assert!(serialized.get("claudeRunning").is_none());
        assert!(serialized.get("bridgeHealthy").is_none());
        assert!(serialized.get("restartBlocked").is_none());

        let completed = SystemStatus {
            network: NetworkQualityStatus {
                deep_checked: true,
                sandbox_egress_state: "failed".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = network_quality_work_report(completed);
        assert!(!result.ok);
        assert_eq!(result.deep.unwrap().sandbox_egress_state, "failed");

        let busy = SystemStatus {
            network: NetworkQualityStatus {
                deep_checked: true,
                daemon_io_blocked: true,
                daemon_mount_io_blocked: true,
                sandbox_egress_state: "daemon_mount_io_busy".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let serialized = serde_json::to_value(network_quality_work_report(busy)).unwrap();
        let top_level = serialized.as_object().unwrap();
        assert_eq!(
            top_level
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            ["code", "deep", "ok", "operation", "warnings"]
                .into_iter()
                .collect()
        );
        let deep = serialized.get("deep").unwrap();
        assert_eq!(
            deep.get("sandboxEgressState").unwrap(),
            "daemon_mount_io_busy"
        );
        assert!(deep.get("daemonIoBlocked").is_none());
        assert!(deep.get("daemonMountIoBlocked").is_none());
        assert!(deep.get("localReady").is_none());
        assert!(deep.get("ready").is_none());
    }

    #[test]
    fn core_runtime_auto_initialization_is_one_safe_absent_daemon_case() {
        let mut status = SystemStatus {
            wsl_installed: true,
            runtime_ready: true,
            ..Default::default()
        };
        assert!(should_initialize_core_runtime(&status));

        status.claude_listener_present = true;
        assert!(!should_initialize_core_runtime(&status));
        status.claude_listener_present = false;

        status.claude_running = true;
        assert!(!should_initialize_core_runtime(&status));
        status.claude_running = false;

        status.restart_blocked = true;
        assert!(!should_initialize_core_runtime(&status));
        status.restart_blocked = false;

        status.windows_bridge_pid = Some(99);
        assert!(!should_initialize_core_runtime(&status));
        assert!(ensure_no_legacy_windows_bridge(&status)
            .unwrap_err()
            .contains("双 Bridge"));

        status.windows_bridge_pid = None;
        assert!(ensure_no_legacy_windows_bridge(&status).is_ok());
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
        fs::write(root.join("scripts").join("csa-runtime-layout.sh"), "").unwrap();

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
    fn provider_catalog_v018_exact_snapshot() {
        let catalog = provider_catalog();
        let groups: Vec<_> = catalog
            .iter()
            .map(|group| (group.title.as_str(), group.tier.as_str()))
            .collect();
        assert_eq!(
            groups,
            vec![
                ("官方直连", "official"),
                ("聚合与编程订阅", "aggregator"),
                ("中转服务", "custom"),
            ]
        );

        let entries: Vec<_> = catalog
            .iter()
            .flat_map(|group| {
                group.providers.iter().map(move |provider| {
                    (
                        group.tier.as_str(),
                        provider.id.as_str(),
                        provider.name.as_str(),
                        provider.badge.as_str(),
                    )
                })
            })
            .collect();
        assert_eq!(
            entries,
            vec![
                ("official", "glm", "GLM-5.2", "官方"),
                ("official", "longcat", "LongCat", "官方"),
                ("official", "deepseek", "DeepSeek", "官方"),
                ("official", "minimax", "MiniMax", "官方"),
                ("official", "claude", "Claude", "官方"),
                ("official", "openai", "OpenAI / GPT", "官方"),
                ("aggregator", "opencode-go", "OpenCode Go", "聚合"),
                ("aggregator", "openrouter", "OpenRouter", "聚合"),
                ("custom", "builtin-relay", "项目方自建中转", "自建"),
                ("custom", "custom", "自定义中转", "自定义"),
            ]
        );
    }

    #[test]
    fn custom_url_must_be_https() {
        assert!(validate_base_url("").is_ok());
        assert!(validate_base_url("https://10521052.xyz/v1").is_ok());
        assert!(validate_base_url("http://10521052.xyz/v1").is_err());
        assert!(validate_base_url("https://user:secret@example.com/v1").is_err());
        assert!(validate_base_url("https://example.com/v1?token=secret").is_err());
        assert!(validate_base_url("https://example.com/v1#secret").is_err());
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
    fn role_bindings_validate_against_saved_subscription_models() {
        let entry = StoredApiKey {
            id: "key-vision".into(),
            provider_id: "custom".into(),
            label: "Vision relay".into(),
            base_url: "https://example.com/v1".into(),
            model: "text-model".into(),
            custom_confirmed: true,
            model_aliases: vec![StoredModelAlias {
                id: "vision-alias".into(),
                display_name: "Vision".into(),
                model: "vision-model".into(),
            }],
            encrypted_api_key: "ciphertext".into(),
        };
        let settings = LauncherSettings {
            api_keys: vec![entry],
            ..LauncherSettings::default()
        };
        let normalized = validate_role_bindings(
            &settings,
            &[
                StoredRoleBinding {
                    role: "vision".into(),
                    provider_id: "custom".into(),
                    api_key_id: "key-vision".into(),
                    model: "vision-model".into(),
                },
                StoredRoleBinding {
                    role: "default".into(),
                    provider_id: "custom".into(),
                    api_key_id: "key-vision".into(),
                    model: "text-model".into(),
                },
            ],
        )
        .unwrap();
        assert_eq!(normalized[0].role, "default");
        assert_eq!(normalized[1].role, "vision");
    }

    #[test]
    fn role_bindings_reject_stale_keys_and_unknown_models() {
        let entry = StoredApiKey {
            id: "key-default".into(),
            provider_id: "deepseek".into(),
            label: "DeepSeek".into(),
            base_url: String::new(),
            model: "deepseek-v4-pro".into(),
            custom_confirmed: false,
            model_aliases: Vec::new(),
            encrypted_api_key: "ciphertext".into(),
        };
        let settings = LauncherSettings {
            api_keys: vec![entry],
            ..LauncherSettings::default()
        };
        let stale = StoredRoleBinding {
            role: "default".into(),
            provider_id: "deepseek".into(),
            api_key_id: "missing".into(),
            model: "deepseek-v4-pro".into(),
        };
        assert!(validate_role_bindings(&settings, &[stale]).is_err());

        let unknown_model = StoredRoleBinding {
            role: "fast".into(),
            provider_id: "deepseek".into(),
            api_key_id: "key-default".into(),
            model: "not-imported".into(),
        };
        assert!(validate_role_bindings(&settings, &[unknown_model]).is_err());
    }

    #[test]
    fn role_activation_retargets_all_stored_aliases() {
        let entry = StoredApiKey {
            id: "key-fast".into(),
            provider_id: "custom".into(),
            label: "Fast".into(),
            base_url: "https://example.com/v1".into(),
            model: "primary-model".into(),
            custom_confirmed: true,
            model_aliases: vec![
                StoredModelAlias {
                    id: "claude-sonnet-4-5".into(),
                    display_name: "Sonnet".into(),
                    model: "primary-model".into(),
                },
                StoredModelAlias {
                    id: "claude-haiku-4-5-20251001".into(),
                    display_name: "Haiku".into(),
                    model: "fast-model".into(),
                },
            ],
            encrypted_api_key: "ciphertext".into(),
        };
        let aliases = aliases_for_role(&entry, "fast-model");
        assert_eq!(aliases.len(), 2);
        assert!(aliases.iter().all(|alias| alias.model == "fast-model"));
    }

    #[test]
    fn aggregate_patch_routes_claude_slots_to_three_independent_upstreams() {
        let routes = vec![
            AggregateRuntimeRoute {
                role: "default".into(),
                backend: "custom".into(),
                api_key: "decision-key".into(),
                base_url: "https://decision.example/v1".into(),
                upstream_mode: "openai".into(),
                model: "decision-model".into(),
            },
            AggregateRuntimeRoute {
                role: "vision".into(),
                backend: "custom".into(),
                api_key: "vision-key".into(),
                base_url: "https://vision.example/v1".into(),
                upstream_mode: "openai".into(),
                model: "vision-model".into(),
            },
            AggregateRuntimeRoute {
                role: "fast".into(),
                backend: "deepseek".into(),
                api_key: "daily-key".into(),
                base_url: "https://daily.example/anthropic".into(),
                upstream_mode: "anthropic".into(),
                model: "daily-model".into(),
            },
        ];
        let patch = bridge_config_patch_for_aggregate_routes("scheme-1", &routes).unwrap();
        assert_eq!(patch["force_model"], "");
        assert_eq!(patch["active_aggregate_scheme_id"], "scheme-1");
        assert_eq!(patch["aggregate_upstreams"].as_array().unwrap().len(), 3);
        let aliases = patch["model_aliases"].as_array().unwrap();
        assert!(aliases
            .iter()
            .any(|alias| { alias["id"] == "claude-opus-4-8" && alias["route_id"] == "default" }));
        assert!(aliases
            .iter()
            .any(|alias| { alias["id"] == "claude-sonnet-5" && alias["route_id"] == "vision" }));
        assert!(aliases.iter().any(|alias| {
            alias["id"] == "claude-haiku-4-5-20251001" && alias["route_id"] == "fast"
        }));
    }

    #[test]
    fn single_api_patch_explicitly_disables_aggregate_mode() {
        let profile = BridgeRuntimeProfile {
            provider_id: "custom".into(),
            label: "Custom".into(),
            backend: "custom",
            api_key_field: "custom_api_key",
            base_url: "https://example.com/v1".into(),
            upstream_mode: "openai",
            default_model: "model-a".into(),
            default_fast_model: "model-a".into(),
            requires_explicit_model: true,
        };
        let patch = bridge_config_patch_for_runtime_profile(&profile, "key-a", "model-a", &[]);
        assert_eq!(patch["aggregate_upstreams"], serde_json::json!([]));
        assert_eq!(patch["active_aggregate_scheme_id"], "");
    }

    #[test]
    fn legacy_role_bindings_migrate_to_scheme_one_without_auto_activation() {
        let mut settings = LauncherSettings {
            role_bindings: vec![StoredRoleBinding {
                role: "default".into(),
                provider_id: "custom".into(),
                api_key_id: "key-a".into(),
                model: "model-a".into(),
            }],
            ..LauncherSettings::default()
        };
        normalize_aggregate_schemes(&mut settings);
        assert_eq!(settings.aggregate_schemes.len(), 2);
        assert_eq!(settings.aggregate_schemes[0].routes, settings.role_bindings);
        assert!(settings.aggregate_schemes[1].routes.is_empty());
        assert!(settings.active_aggregate_scheme_id.is_none());
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
    fn skin_choice_persists_and_invalid_values_fall_back_to_console() {
        let legacy: LauncherSettings = serde_json::from_str(
            r#"{"selectedProviderId":"deepseek","customBaseUrl":"","customConfirmed":false}"#,
        )
        .unwrap();
        assert_eq!(ui_preferences(&legacy).skin, None);

        let mut selected = LauncherSettings::default();
        selected.ui_skin = Some("classic".into());
        let encoded = serde_json::to_string(&selected).unwrap();
        let decoded: LauncherSettings = serde_json::from_str(&encoded).unwrap();
        assert_eq!(ui_preferences(&decoded).skin.as_deref(), Some("classic"));

        selected.ui_skin = Some("unknown-skin".into());
        assert_eq!(ui_preferences(&selected).skin.as_deref(), Some("console"));

        let wrong_type: LauncherSettings = serde_json::from_str(
            r#"{"selectedProviderId":"deepseek","customBaseUrl":"","customConfirmed":false,"uiSkin":{"bad":true}}"#,
        )
        .unwrap();
        assert_eq!(ui_preferences(&wrong_type).skin.as_deref(), Some("console"));
        assert_eq!(
            ui_preferences_from_text("{not-json").skin.as_deref(),
            Some("console")
        );
        assert_eq!(
            ui_preferences_from_text(r#"{"uiSkin":42}"#).skin.as_deref(),
            Some("console")
        );
    }

    #[test]
    fn custom_label_is_accepted_for_any_provider_and_blank_uses_fallback() {
        let settings = LauncherSettings::default();
        let glm = provider_by_id("glm").unwrap();
        let openrouter = provider_by_id("openrouter").unwrap();
        assert_eq!(
            label_for_provider(&settings, &glm, "  实验室备用  ").unwrap(),
            "实验室备用"
        );
        assert_eq!(
            label_for_provider(&settings, &openrouter, "夜间线路").unwrap(),
            "夜间线路"
        );
        assert_eq!(label_for_provider(&settings, &glm, "").unwrap(), "GLM-5.2");
        assert!(label_for_provider(&settings, &glm, &"长".repeat(81)).is_err());
    }

    #[test]
    fn rename_api_key_uses_shared_display_name_validation() {
        let mut settings = LauncherSettings::default();
        settings.api_keys.push(StoredApiKey {
            id: "key-lab".into(),
            provider_id: "glm".into(),
            label: "GLM-5.2".into(),
            base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
            model: "glm-5.2".into(),
            custom_confirmed: false,
            model_aliases: Vec::new(),
            encrypted_api_key: "ciphertext".into(),
        });
        rename_api_key_in_settings(&mut settings, "key-lab", "  实验室备用  ").unwrap();
        assert_eq!(settings.api_keys[0].label, "实验室备用");
        assert!(rename_api_key_in_settings(&mut settings, "key-lab", "").is_err());
        assert!(rename_api_key_in_settings(&mut settings, "key-lab", "坏\n名称").is_err());
    }

    #[test]
    fn rename_changes_only_the_presentation_label() {
        let original = StoredApiKey {
            id: "key-lab".into(),
            provider_id: "glm".into(),
            label: "GLM-5.2".into(),
            base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
            model: "glm-5.2".into(),
            custom_confirmed: false,
            model_aliases: vec![StoredModelAlias {
                id: "decision".into(),
                display_name: "决策".into(),
                model: "glm-5.2".into(),
            }],
            encrypted_api_key: "ciphertext".into(),
        };
        let mut settings = LauncherSettings {
            api_keys: vec![original.clone()],
            ..LauncherSettings::default()
        };

        rename_api_key_in_settings(&mut settings, "key-lab", "实验室备用").unwrap();
        let renamed = &settings.api_keys[0];
        assert_eq!(renamed.label, "实验室备用");
        assert_eq!(renamed.id, original.id);
        assert_eq!(renamed.provider_id, original.provider_id);
        assert_eq!(renamed.base_url, original.base_url);
        assert_eq!(renamed.model, original.model);
        assert_eq!(renamed.model_aliases.len(), original.model_aliases.len());
        assert_eq!(renamed.model_aliases[0].id, original.model_aliases[0].id);
        assert_eq!(
            renamed.model_aliases[0].display_name,
            original.model_aliases[0].display_name
        );
        assert_eq!(
            renamed.model_aliases[0].model,
            original.model_aliases[0].model
        );
        assert_eq!(renamed.encrypted_api_key, original.encrypted_api_key);
    }

    #[test]
    fn appearance_and_rename_commands_persist_settings_only() {
        let source = include_str!("lib.rs");
        for (start_marker, end_marker) in [
            (
                "fn save_ui_skin_impl(",
                "#[tauri::command]\nasync fn save_ui_skin(",
            ),
            (
                "fn rename_api_key_impl(",
                "#[tauri::command]\nasync fn rename_api_key(",
            ),
        ] {
            let start = source
                .find(start_marker)
                .expect("settings command should exist");
            let end = source[start..]
                .find(end_marker)
                .map(|offset| start + offset)
                .expect("settings command boundary should exist");
            let implementation = &source[start..end];
            assert!(implementation.contains("persist_launcher_settings(&settings)?"));
            assert!(!implementation.contains("commit_launcher_settings_with_bridge"));
            assert!(!implementation.contains("restart_bridge"));
            assert!(!implementation.contains("apply_bridge_config"));
        }
    }

    #[test]
    fn api_key_test_budget_supports_reasoning_models() {
        assert!(API_KEY_TEST_INITIAL_MAX_TOKENS >= 256);
        assert!(API_KEY_TEST_RETRY_MAX_TOKENS >= API_KEY_TEST_INITIAL_MAX_TOKENS * 4);
    }

    #[test]
    fn preflight_error_attributes_layer() {
        let connection = preflight_failure_message(
            "All connection attempts failed",
            Some("work.bridge_egress.proxy_dead"),
            Some("http://127.0.0.1:10808"),
        );
        assert!(connection.starts_with("上游不可达。"));
        assert!(connection.contains("http://127.0.0.1:10808"));
        assert!(connection.contains("work.bridge_egress.proxy_dead"));
        assert!(connection.contains("这不是 Key 的问题"));
        assert!(connection.contains("详情：All connection attempts failed"));
        assert!(!connection.contains("API Key 或账号权限问题"));

        let authentication = preflight_failure_message(
            "HTTP 401: api key ****-key is invalid",
            Some("work.bridge_egress.proxy_dead"),
            Some("http://127.0.0.1:10808"),
        );
        assert!(authentication.starts_with("认证失败（HTTP 401/403"));
        assert!(authentication.contains("API Key 或账号权限问题"));
        assert!(authentication.contains("不是出口代理问题"));
        assert!(authentication.contains("详情：HTTP 401"));
        assert!(!authentication.contains("work.bridge_egress.proxy_dead"));
        assert!(!authentication.contains("能力体检"));

        assert_eq!(
            classify_preflight_failure("无法连接到远程服务器：连接被拒绝"),
            PreflightFailureLayer::Connection
        );
        assert_eq!(
            classify_preflight_failure("远程服务器返回错误：(401) 未经授权"),
            PreflightFailureLayer::Authentication
        );
        let localized = preflight_failure_message(
            "操作超时，无法连接到远程服务器",
            Some("work.bridge_egress.proxy_dead"),
            Some("http://127.0.0.1:10808"),
        );
        assert!(localized.starts_with("上游不可达。"));
        assert!(localized.contains("这不是 Key 的问题"));

        let source = include_str!("lib.rs");
        let activation = source
            .split_once("fn activate_api_key_impl")
            .unwrap()
            .1
            .split_once("#[tauri::command]")
            .unwrap()
            .0;
        assert!(activation.contains("reject_known_dead_bridge_proxy()?"));
        assert!(activation.contains("commit_launcher_settings_with_verified_bridge"));
        assert!(activation.contains("verify_bridge_after_api_key_switch"));
        assert!(!activation.contains("test_api_key_impl("));

        let aggregate = source
            .split_once("fn activate_aggregate_scheme_in_settings")
            .unwrap()
            .1
            .split_once("fn save_and_activate_aggregate_scheme_impl")
            .unwrap()
            .0;
        assert!(aggregate.contains("commit_launcher_settings_with_verified_bridge"));
        assert!(aggregate.contains("verify_bridge_after_aggregate_switch"));
        let aggregate_routes = source
            .split_once("fn aggregate_runtime_routes")
            .unwrap()
            .1
            .split_once("fn activate_aggregate_scheme_in_settings")
            .unwrap()
            .0;
        assert!(!aggregate_routes.contains("test_api_key_impl("));
        let aggregate_verify = source
            .split_once("fn verify_bridge_after_aggregate_switch")
            .unwrap()
            .1
            .split_once("fn reject_known_dead_bridge_proxy")
            .unwrap()
            .0;
        assert!(aggregate_verify.contains("run_bridge_egress_probe_for_models(true, &aliases)"));
    }

    #[cfg(windows)]
    #[test]
    fn invalid_saved_base_url_preflight_returns_visible_error() {
        let result = test_api_key_impl(
            "custom".into(),
            "diagnostic-key".into(),
            "https://127.0.0.1:9/v1".into(),
            true,
            "diagnostic-model".into(),
            "Reply only: OK".into(),
        )
        .unwrap();
        let error = require_successful_preflight(result).unwrap_err();
        assert!(!error.trim().is_empty());
        assert!(!error.contains("diagnostic-key"));
    }

    #[test]
    fn switch_path_keeps_required_observability_markers() {
        let source = include_str!("lib.rs");
        for marker in [
            "received configuration transition request",
            "writing Bridge config: revision=",
            "Bridge health verified: revision=",
            "activation failed; starting rollback",
            "transition complete",
        ] {
            assert!(
                source.contains(marker),
                "missing switch log marker: {marker}"
            );
        }
        assert!(
            BRIDGE_RESTART_GUEST_TIMEOUT_SECONDS + 2 < BRIDGE_RESTART_HOST_TIMEOUT.as_secs(),
            "the WSL restart watchdog must finish before its host watchdog"
        );
        let restart = source
            .split_once("fn restart_bridge_after_config")
            .unwrap()
            .1
            .split_once("fn dashboard_url_from_config")
            .unwrap()
            .0;
        assert!(restart.contains("\"timeout\""));
        assert!(restart.contains("\"--signal=TERM\""));
        assert!(restart.contains("\"--kill-after=2s\""));
        assert!(restart.contains("BRIDGE_RESTART_HOST_TIMEOUT"));

        let transition = source
            .split_once("fn apply_bridge_config_patch_value")
            .unwrap()
            .1
            .split_once("fn rollback_applied_bridge")
            .unwrap()
            .0;
        assert!(transition.contains("Bridge rollback completed and verified"));
        assert!(transition.contains("回滚重启/运行态确认失败"));
        assert!(!transition.contains("let _ = restart_bridge_after_config"));
    }

    #[test]
    fn saving_new_api_key_does_not_activate_bridge() {
        let source = include_str!("lib.rs");
        let start = source
            .find("fn save_api_key_impl(")
            .expect("save_api_key_impl should exist");
        let end = source[start..]
            .find("async fn save_api_key(")
            .map(|offset| start + offset)
            .expect("save_api_key command should follow the implementation");
        let implementation = &source[start..end];

        assert!(implementation.contains("persist_launcher_settings(&settings)?"));
        assert!(!implementation.contains("commit_launcher_settings_with_bridge"));
        assert!(!implementation.contains("settings.active_api_key_id ="));
    }

    #[test]
    fn api_key_ui_requires_pending_confirmation() {
        let source = include_str!("../../src/App.tsx");

        assert_eq!(
            source
                .matches("invoke<LauncherSettings>(\"activate_api_key\"")
                .count(),
            1
        );
        assert_eq!(source.matches("activateKey(pendingApiKeyId)").count(), 1);
        assert!(source.contains("onClick={() => preselectKey(entry.id)}"));
        assert!(source.contains("保存到列表"));
    }

    #[test]
    fn first_paint_allow_only_initialize_background() {
        let source = include_str!("../../src/App.tsx");
        let allow_start = source
            .find("const refreshAllow = useCallback(async () =>")
            .expect("ALLOW refresh should exist");
        let allow_end = source[allow_start..]
            .find("const refreshGrade = useCallback(async () =>")
            .map(|offset| allow_start + offset)
            .expect("GRADE refresh should follow ALLOW refresh");
        let allow = &source[allow_start..allow_end];
        assert!(allow.contains("invoke<AllowStatus>(\"get_allow_status\")"));
        assert!(allow.contains("setAllowLoaded(true)"));
        for forbidden in [
            "get_grade_status",
            "get_system_status",
            "initialize_runtime",
            "run_network_quality_check",
            "setStatus(",
            "setBusy(",
            "busyRef",
        ] {
            assert!(
                !allow.contains(forbidden),
                "first-paint ALLOW path contains blocking/non-ALLOW work: {forbidden}"
            );
        }

        let initialize_start = source
            .find("const initializeRuntimeInBackground = useCallback(async () =>")
            .expect("background initializer should exist");
        let initialize_end = source[initialize_start..]
            .find("const refresh = useCallback(async () =>")
            .map(|offset| initialize_start + offset)
            .expect("manual refresh should follow background initialize");
        let initialize = &source[initialize_start..initialize_end];
        assert!(initialize.contains("invoke<unknown>(\"initialize_runtime\")"));
        assert!(initialize.contains("await refreshAllow()"));
        assert!(initialize.contains("const requestEpoch = statusCommitEpoch.current;"));
        assert_eq!(
            initialize
                .matches("requestEpoch === statusCommitEpoch.current")
                .count(),
            2
        );
        for forbidden in ["get_system_status", "setBusy(", "busyRef", "setStatus("] {
            assert!(
                !initialize.contains(forbidden),
                "background initialize blocks or overwrites the first paint: {forbidden}"
            );
        }

        let boot_start = source
            .find("async function paintAllowThenStartBackground()")
            .expect("paint pipeline should exist");
        let boot_end = source[boot_start..]
            .find("async function loadProviderState()")
            .map(|offset| boot_start + offset)
            .expect("provider load should follow paint pipeline");
        let boot = &source[boot_start..boot_end];
        let allow_at = boot.find("await refreshAllow()").unwrap();
        let paint_at = boot.find("requestAnimationFrame").unwrap();
        let timer_at = boot
            .find("window.setInterval(refreshGrade, 30_000)")
            .unwrap();
        let initialize_at = boot.find("void initializeRuntimeInBackground()").unwrap();
        let grade_at = boot.find("void refreshGrade()").unwrap();
        assert!(allow_at < paint_at && paint_at < timer_at);
        assert!(timer_at < initialize_at && timer_at < grade_at);
        assert!(!boot.contains("await initializeRuntimeInBackground()"));
        assert!(!boot.contains("await refreshGrade()"));
    }

    #[test]
    fn periodic_refresh_is_grade_only_and_preserves_allow_and_work() {
        let source = include_str!("../../src/App.tsx");
        let grade_start = source
            .find("const refreshGrade = useCallback(async () =>")
            .expect("GRADE refresh should exist");
        let grade_end = source[grade_start..]
            .find("const initializeRuntimeInBackground = useCallback(async () =>")
            .map(|offset| grade_start + offset)
            .expect("initializer should follow GRADE refresh");
        let grade = &source[grade_start..grade_end];
        assert!(grade.contains("await runNonGatingProbe<GradeStatus>({"));
        assert!(grade.contains("lane: \"grade\""));
        assert!(grade.contains("key: \"status\""));
        assert!(grade.contains("timeoutMs: GRADE_STATUS_TIMEOUT_MS"));
        assert!(grade.contains("circuit: gradeStatusCircuitRef.current"));
        assert!(grade.contains("invoke<GradeStatus>(\"get_grade_status\")"));
        assert!(grade.contains("setStatus((current) => {"));
        assert!(grade.contains("const merged = mergeGradeStatus(current, result.value)"));
        assert!(
            grade.contains("laneReducer(laneStateRef.current, { lane: \"grade\", value: merged })")
        );
        assert!(grade.contains("return reduced.grade"));
        for forbidden in [
            "get_allow_status",
            "initialize_runtime",
            "get_system_status",
            "run_network_quality_check",
            "setAllowStatus",
        ] {
            assert!(
                !grade.contains(forbidden),
                "periodic GRADE path crossed into another lane: {forbidden}"
            );
        }
        assert!(source.contains("window.setInterval(refreshGrade, 30_000)"));
        assert!(!source.contains("window.setInterval(refresh, 30_000)"));

        let merge_start = source
            .find("const mergeGradeStatus =")
            .expect("GRADE merge should exist");
        let merge_end = source[merge_start..]
            .find("interface Provider")
            .map(|offset| merge_start + offset)
            .expect("provider types should follow GRADE merge");
        let merge = &source[merge_start..merge_end];
        assert!(merge.contains("...current.network"));
        assert!(merge.contains("...next.network"));
        assert!(merge.find("...current.network") < merge.find("...next.network"));
        assert!(merge.contains("current.network.deepChecked"));
        assert!(merge.contains("current.network.sandboxEgressState"));
        for forbidden in [
            "claudeRunning",
            "claudePid",
            "windowsBridgePid",
            "distro:",
            "linuxUser",
            "pid8765",
            "pid8766",
        ] {
            assert!(
                !merge.contains(forbidden),
                "GRADE merge attempted to write ALLOW-owned field: {forbidden}"
            );
        }

        let action_start = source
            .find("async function runAction(")
            .expect("lifecycle action should exist");
        let action_end = source[action_start..]
            .find("async function applyDraftKey()")
            .map(|offset| action_start + offset)
            .expect("API Key action should follow lifecycle action");
        let action = &source[action_start..action_end];
        assert!(action.contains("await invoke<unknown>(command)"));
        assert!(action.contains("await refreshAllow()"));
        assert!(action.contains("void refreshGrade()"));
        for forbidden in ["setStatus(", "get_system_status", "networkCheckingRef"] {
            assert!(
                !action.contains(forbidden),
                "lifecycle action overwrote ALLOW/GRADE or was blocked by WORK: {forbidden}"
            );
        }
    }

    #[test]
    fn primary_button_is_not_blocked_by_grade_or_work() {
        let source = include_str!("../../src/App.tsx");
        let selector_start = source
            .find("const primaryButton = useMemo(")
            .expect("primary button selector should exist");
        let selector_end = source[selector_start..]
            .find("const mutationBusy")
            .map(|offset| selector_start + offset)
            .expect("mutation busy state should follow primary selector");
        let selector = &source[selector_start..selector_end];
        assert!(selector.contains("primaryButtonView(allowStatus, allowLoaded, allowActionBusy)"));
        assert!(selector.contains("const canOpenClaude = primaryButton.action === \"open\""));

        let button_start = source
            .find("className=\"primary-button\"")
            .expect("primary button should exist");
        let button_end = source[button_start..]
            .find("</button>")
            .map(|offset| button_start + offset)
            .expect("primary button should close");
        let button = &source[button_start..button_end];
        assert!(button.contains("disabled={primaryButton.disabled}"));
        assert!(button.contains("{primaryButton.label}"));

        let action_start = source
            .find("async function primaryAction()")
            .expect("primary action should exist");
        let action_end = source[action_start..]
            .find("async function openDashboard(")
            .map(|offset| action_start + offset)
            .expect("dashboard helper should follow primary action");
        let action = &source[action_start..action_end];
        assert!(
            action.contains("if (primaryButton.disabled || allowActionBusyRef.current) return;")
        );
        assert!(action.contains("primaryButton.action === \"stop_legacy_bridge\""));
        assert!(action.contains("primaryButton.action === \"open\""));
        assert!(action.contains("primaryButton.action === \"install\""));
        assert!(action.contains("updateAllowActionBusy(true)"));
        assert!(action.contains("updateAllowActionBusy(false)"));

        for forbidden in [
            "status.",
            "status[",
            "network.",
            "network[",
            "networkChecking",
            "restartBlocked",
            "workWarnings",
            "busyRef",
            "updateBusy(",
            "mutationBusy",
        ] {
            assert!(!button.contains(forbidden));
            assert!(!selector.contains(forbidden));
            assert!(!action.contains(forbidden));
        }
    }

    #[test]
    fn grade_and_work_probes_have_hard_non_gating_deadlines() {
        let source = include_str!("../../src/App.tsx");
        let contract = include_str!("../../src/laneContract.ts");
        let css = include_str!("../../src/App.css");

        for deadline in [
            "const GRADE_STATUS_TIMEOUT_MS = 15_000;",
            "const NETWORK_QUALITY_TIMEOUT_MS = 25_000;",
            "const RUNTIME_UPDATE_TIMEOUT_MS = 45_000;",
        ] {
            assert!(
                source.contains(deadline),
                "missing hard probe deadline: {deadline}"
            );
        }

        let grade_start = source
            .find("const refreshGrade = useCallback(async () =>")
            .expect("GRADE status probe should exist");
        let grade_end = source[grade_start..]
            .find("const initializeRuntimeInBackground = useCallback(async () =>")
            .map(|offset| grade_start + offset)
            .expect("initializer should follow GRADE status probe");
        let grade = &source[grade_start..grade_end];
        for required in [
            "await runNonGatingProbe<GradeStatus>({",
            "lane: \"grade\"",
            "key: \"status\"",
            "timeoutMs: GRADE_STATUS_TIMEOUT_MS",
            "circuit: gradeStatusCircuitRef.current",
            "task: () => invoke<GradeStatus>(\"get_grade_status\")",
            "classifyValue: (value) => classifyNonGatingFailure(",
            "muted: nonGatingFailureIsMuted(result.code, result.timedOut, result.skipped)",
        ] {
            assert!(
                grade.contains(required),
                "GRADE probe lost non-gating guard: {required}"
            );
        }
        assert!(grade.contains("if (result.value.state === \"error\")"));
        assert!(grade.contains("muted: false"));

        let network_start = source
            .find("async function runNetworkQualityCheck()")
            .expect("network quality WORK probe should exist");
        let network_end = source[network_start..]
            .find("async function runAction(")
            .map(|offset| network_start + offset)
            .expect("lifecycle action should follow network quality probe");
        let network = &source[network_start..network_end];
        for required in [
            "await runNonGatingProbe<WorkReport>({",
            "lane: \"work\"",
            "key: \"network_quality\"",
            "timeoutMs: NETWORK_QUALITY_TIMEOUT_MS",
            "circuit: networkQualityCircuitRef.current",
            "invoke<WorkReport>(\"run_network_quality_check\")",
            "classifyValue: (value) => value.ok",
            "classifyNonGatingFailure(",
            "muted: nonGatingFailureIsMuted(result.code, result.timedOut, result.skipped)",
        ] {
            assert!(
                network.contains(required),
                "network WORK probe lost non-gating guard: {required}"
            );
        }
        assert!(network.contains("if (!result.value.ok)"));
        assert!(network.contains("const muted = nonGatingFailureIsMuted(result.value.code)"));
        assert!(network.contains("muted,"));

        let runtime_start = source
            .find("async function checkRuntimeUpdate()")
            .expect("runtime update WORK probe should exist");
        let runtime_end = source[runtime_start..]
            .find("function openRuntimePrompt(")
            .map(|offset| runtime_start + offset)
            .expect("runtime prompt should follow runtime update probe");
        let runtime = &source[runtime_start..runtime_end];
        for required in [
            "await runNonGatingProbe<RuntimeUpdateStatus>({",
            "lane: \"work\"",
            "key: \"runtime_update\"",
            "timeoutMs: RUNTIME_UPDATE_TIMEOUT_MS",
            "circuit: runtimeUpdateCircuitRef.current",
            "invoke<RuntimeUpdateStatus>(\"get_runtime_update_status\")",
            "if (runtimeCheckingRef.current) return;",
            "runtimeCheckingRef.current = true;",
            "runtimeCheckingRef.current = false;",
        ] {
            assert!(
                runtime.contains(required),
                "runtime WORK probe lost non-gating guard: {required}"
            );
        }
        assert!(runtime.contains(
            "const muted = nonGatingFailureIsMuted(result.code, result.timedOut, result.skipped)"
        ));
        assert!(runtime.contains("muted,"));

        for required in [
            "globalThis.setTimeout",
            "if (settled) return;",
            "if (generation === circuit.generation)",
            "circuit.openUntil = now() + cooldownMs",
            "`${lane}.${key}.timeout`",
            "`${lane}.${key}.circuit_open`",
            "`${lane}.${key}.superseded`",
            "const semanticFailure = classifyValue?.(value)",
            "const classified = classifyNonGatingFailure(lane, key, message)",
        ] {
            assert!(
                contract.contains(required),
                "probe runner lost deadline/circuit guarantee: {required}"
            );
        }

        assert!(source.contains(
            "className={`notice ${probeNotice.muted ? \"probe-muted\" : \"probe-failure\"}`}"
        ));
        assert!(source.contains(
            "<p>{nonGatingFailurePresentation(`${probeNotice.code}: ${probeNotice.message}`, allowStatus)}</p>"
        ));
        assert!(source
            .contains("detail: nonGatingFailurePresentation(baseSummary.detail, allowStatus)"));
        assert!(contract.contains("`${message}；仍可打开 Claude Science`"));
        assert!(css.contains(".notice.probe-muted"));
        assert!(css.contains(".notice.probe-failure"));
    }

    #[test]
    fn backend_probe_deadlines_are_machine_classified_and_bounded() {
        assert!(grade_probe_diagnostic("wsl_inspection", "8 秒内没有响应")
            .starts_with("grade.wsl_inspection.timeout:"));
        assert!(grade_probe_diagnostic("storage_snapshot", "invalid JSON")
            .starts_with("grade.storage_snapshot.failed:"));
        assert!(RUNTIME_UPDATE_BACKEND_BUDGET < Duration::from_secs(45));
        let expired = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .expect("test Instant should support a one millisecond subtraction");
        assert_eq!(
            runtime_update_deadline_remaining(expired).unwrap_err(),
            "work.runtime_update.timeout: runtime update probe exceeded its backend deadline"
        );

        let backend = include_str!("lib.rs");
        let auto_map_start = backend
            .find("fn auto_map_api_key_impl(")
            .expect("auto-map implementation should exist");
        let auto_map_end = backend[auto_map_start..]
            .find("async fn auto_map_api_key(")
            .map(|offset| auto_map_start + offset)
            .expect("auto-map command should follow its implementation");
        let auto_map = &backend[auto_map_start..auto_map_end];
        assert!(auto_map.contains("run_powershell_with_stdin_timeout("));
        assert!(auto_map.contains("Duration::from_secs(20)"));
        assert!(!auto_map.contains("run_powershell_with_stdin(script, &input)"));
    }

    #[test]
    fn claude_open_ui_serializes_clicks_and_keeps_nonce_off_frontend() {
        let source = include_str!("../../src/App.tsx");
        let action_start = source
            .find("async function primaryAction()")
            .expect("primary action should exist");
        let action_end = source[action_start..]
            .find("async function openDashboard(")
            .map(|offset| action_start + offset)
            .expect("dashboard helper should follow primary action");
        let action = &source[action_start..action_end];
        let refresh_start = source
            .find("const refresh = useCallback(async () =>")
            .expect("refresh should exist");
        let refresh_end = source[refresh_start..]
            .find("useEffect(() =>")
            .map(|offset| refresh_start + offset)
            .expect("refresh effect should follow refresh callback");
        let refresh = &source[refresh_start..refresh_end];
        let deep_start = source
            .find("async function runNetworkQualityCheck()")
            .expect("deep network handler should exist");
        let deep_end = source[deep_start..]
            .find("async function runAction(")
            .map(|offset| deep_start + offset)
            .expect("lifecycle action should follow deep network handler");
        let deep = &source[deep_start..deep_end];
        let can_open_start = source
            .find("const primaryButton = useMemo(")
            .expect("open readiness should exist");
        let can_open_end = source[can_open_start..]
            .find("const mutationBusy")
            .map(|offset| can_open_start + offset)
            .expect("mutation busy state should follow open readiness");
        let can_open = &source[can_open_start..can_open_end];
        let backend_start = include_str!("lib.rs")
            .find("fn get_claude_url_impl()")
            .expect("Claude URL implementation should exist");
        let backend_end = include_str!("lib.rs")[backend_start..]
            .find("async fn open_claude_science(")
            .map(|offset| backend_start + offset)
            .expect("open command should follow URL implementation");
        let backend = &include_str!("lib.rs")[backend_start..backend_end];

        assert!(
            action.contains("if (primaryButton.disabled || allowActionBusyRef.current) return;")
        );
        assert!(action.contains("if (primaryButton.action === \"open\")"));
        assert!(action.contains("updateAllowActionBusy(true);"));
        assert!(action.contains("setError(\"\");"));
        assert!(action.contains("await invoke<void>(\"open_claude_science\");"));
        assert!(!action.contains("get_system_status"));
        assert!(!action.contains("busyRef"));
        assert!(!action.contains("updateBusy("));
        assert!(!refresh.contains("run_network_quality_check"));
        assert!(deep.contains("runNonGatingProbe<WorkReport>"));
        assert!(deep.contains("timeoutMs: NETWORK_QUALITY_TIMEOUT_MS"));
        assert!(deep.contains("circuit: networkQualityCircuitRef.current"));
        assert!(deep.contains("invoke<WorkReport>(\"run_network_quality_check\")"));
        assert!(deep.contains("setStatus((current) =>"));
        assert!(deep.contains("...current"));
        assert!(deep.contains("...current.network"));
        assert!(deep.contains("...next.deep"));
        assert!(deep.contains("ready: current.network.localReady"));
        assert!(!deep.contains("invoke<SystemStatus>(\"run_network_quality_check\")"));
        assert!(!deep.contains("setStatus(next)"));
        assert!(source.contains("const [networkChecking, setNetworkChecking] = useState(false);"));
        assert!(source.contains("networkCheckingRef.current"));
        assert!(can_open.contains("primaryButtonView(allowStatus, allowLoaded, allowActionBusy)"));
        assert!(can_open.contains("const canOpenClaude = primaryButton.action === \"open\""));
        assert!(!can_open.contains("status.claudeRunning"));
        assert!(!can_open.contains("status.network.daemonIoBlocked"));
        assert!(!can_open.contains("status.network.localReady"));
        assert!(!can_open.contains("status.bridgeHealthy"));
        assert!(!refresh.contains("networkCheckingRef.current"));
        assert!(source.contains("const mutationBusy = busy || networkChecking;"));
        assert!(source.contains("function tryBeginMutation()"));
        assert!(source.contains("if (busyRef.current || networkCheckingRef.current) return false;"));
        assert!(backend.contains("discover_distros_with_timeout(distro_timeout)"));
        assert!(!backend.contains("selected_linux_user_quick"));
        assert!(backend.contains("run_wsl_default_user_script_with_guest_timeout"));
        assert!(backend.contains("CLAUDE_URL_SHELL"));
        assert!(!backend.contains("\"-lc\""));
        assert!(!backend.contains("current_status()"));
        assert!(source.contains("const statusCommitEpoch = useRef(0);"));
        assert!(source.contains("const requestEpoch = statusCommitEpoch.current;"));
        assert!(source.contains("requestEpoch !== statusCommitEpoch.current"));
        assert!(source.contains("statusCommitEpoch.current += 1;"));
        assert!(source.contains("深检时遇到瞬时 I/O；不影响本地打开，恢复后可重试"));
        assert!(source.contains("status.network.daemonIoBlocked"));
        assert!(!source.contains("? ` · 守护进程忙（${status.network.daemonWaitChannel"));
        assert!(action.contains("finally"));
        assert!(action.contains("updateAllowActionBusy(false);"));
        assert!(!source.contains("get_claude_url"));
    }

    #[test]
    fn aggregate_scheme_ui_requires_pending_confirmation() {
        let source = include_str!("../../src/App.tsx");
        let preselect_start = source
            .find("function preselectAggregateScheme(")
            .expect("aggregate preselection helper should exist");
        let confirm_start = source[preselect_start..]
            .find("async function confirmPendingAggregateScheme(")
            .map(|offset| preselect_start + offset)
            .expect("aggregate confirmation helper should exist");
        let cancel_start = source[confirm_start..]
            .find("function cancelPendingAggregateScheme(")
            .map(|offset| confirm_start + offset)
            .expect("aggregate cancellation helper should exist");
        let switch_mode_start = source[cancel_start..]
            .find("function switchAccessMode(")
            .map(|offset| cancel_start + offset)
            .expect("access mode helper should exist");
        let delete_start = source[switch_mode_start..]
            .find("async function deleteKey(")
            .map(|offset| switch_mode_start + offset)
            .expect("delete helper should follow access mode helper");

        assert!(!source[preselect_start..confirm_start].contains("invoke<LauncherSettings>"));
        assert!(source[confirm_start..cancel_start]
            .contains("invoke<LauncherSettings>(\"activate_aggregate_scheme\""));
        assert!(!source[switch_mode_start..delete_start].contains("activateKey("));
        assert!(!source[switch_mode_start..delete_start].contains("activate_aggregate_scheme"));
        assert!(source.contains("onClick={() => preselectAggregateScheme(scheme.id)}"));
        assert!(source.contains("onClick={() => void confirmPendingAggregateScheme()}"));
        assert!(source.contains("三条路由各验证 1 次，共 3 次 max_tokens=1 真实请求"));
        assert!(source.contains("共 3 次，可能产生费用"));
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
        assert!(settings.active_role.is_none());
        assert!(settings.role_bindings.is_empty());
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
    fn bridge_config_stdin_envelope_roundtrips_without_putting_secret_in_argv() {
        let sentinel = "csa-secret-sentinel-argv-must-not-contain-this";
        let value = serde_json::json!({
            "custom_base_url": "https://10521052.xyz/v1",
            "force_model": "glm-5.2",
            "custom_api_key": sentinel,
        });
        let frame = bridge_config_stdin_envelope("patch", &value).unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(&frame).unwrap();
        assert_eq!(decoded["schemaVersion"], 1);
        assert_eq!(decoded["operation"], "patch");
        assert_eq!(decoded["payload"], value);
        assert!(String::from_utf8_lossy(&frame).contains(sentinel));

        let command = wsl_python_stdin_command("Ubuntu-24.04", "fixed-safe-script");
        let argv = command
            .get_args()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let sentinel_hex = sentinel
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert!(!argv.contains(sentinel));
        assert!(!argv.contains(&sentinel_hex));
        assert!(bridge_config_stdin_envelope("invalid", &value).is_err());
    }

    #[test]
    fn bridge_config_rollback_records_restore_and_delete_keys() {
        let old_secret = "csa-old-secret-must-stay-on-stdin";
        let mut restore = serde_json::Map::new();
        restore.insert(
            "force_model".into(),
            serde_json::Value::String("old".into()),
        );
        restore.insert(
            "deepseek_api_key".into(),
            serde_json::Value::String(old_secret.into()),
        );
        let rollback = BridgeConfigRollback {
            restore,
            delete: vec!["custom_base_url".into()],
        };
        let frame = bridge_config_stdin_envelope("rollback", &rollback).unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(&frame).unwrap();
        assert_eq!(decoded["payload"]["restore"]["force_model"], "old");
        assert_eq!(
            decoded["payload"]["restore"]["deepseek_api_key"],
            old_secret
        );
        assert_eq!(decoded["payload"]["delete"][0], "custom_base_url");

        let command = wsl_python_stdin_command("Ubuntu-24.04", "fixed-safe-script");
        let argv = command
            .get_args()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!argv.contains(old_secret));

        let production = include_str!("lib.rs")
            .split_once("fn bridge_config_stdin_envelope")
            .and_then(|(_, value)| value.split_once("fn restart_bridge_after_config"))
            .map(|(value, _)| value)
            .unwrap();
        assert!(!production.contains("json_arg_hex"));
        assert!(!production.contains("bytes.fromhex(sys.argv[1])"));
        assert!(production.contains("command_output_with_stdin_timeout("));
        assert!(production.contains("Existing Bridge config is unreadable; refusing to replace it"));
        assert!(
            production.contains("Current Bridge config is unreadable; refusing unsafe rollback")
        );
    }

    #[test]
    fn bridge_only_restart_exits_before_claude_runtime_work() {
        let script = include_str!("../../../scripts/start-claude-science-wsl.sh");
        let unsafe_preflight = script
            .find("if [ -n \"$preflight_unsafe_pid\" ]; then")
            .expect("unsafe Claude preflight should be present");
        let bridge_stage = script
            .find("csa_stage_bridge_runtime \"$PROJECT_DIR\"")
            .expect("Bridge staging should be present");
        let full_activation_stop = script
            .find(
                "if [ \"${CSA_BRIDGE_ONLY:-0}\" != \"1\" ]; then\n  stop_existing_claude_for_activation || exit 1",
            )
            .expect("full activation should stop Claude before Bridge staging");
        let host_grant_repair = script
            .find("if [ \"${CSA_REPAIR_DRVFS_GRANTS:-0}\" = \"1\" ]; then")
            .expect("explicit DrvFS repair should be present");
        let bridge_only = script
            .find("if [ \"${CSA_BRIDGE_ONLY:-0}\" = \"1\" ]")
            .expect("bridge-only mode should be present");
        let token_refresh = script
            .find("TOKEN_FILE=")
            .expect("token refresh block should be present");
        let claude_start = script
            .find("launch_claude_daemon \"$PATCHED_BIN\"")
            .expect("Claude Science start command should be present");

        assert!(unsafe_preflight < bridge_stage);
        assert!(full_activation_stop < bridge_stage);
        assert!(full_activation_stop < host_grant_repair);
        assert!(host_grant_repair < bridge_stage);
        assert!(script[unsafe_preflight..bridge_stage]
            .contains("runtime pointers, and unrelated ports unchanged"));
        assert!(bridge_only < token_refresh);
        assert!(bridge_only < claude_start);
        assert!(script[bridge_only..token_refresh].contains("exit 0"));
        assert!(!script.contains("record_deep_network_quality || true"));
        assert_eq!(script.matches("record_deep_network_quality").count(), 1);
        assert!(!script.contains("CLAUDE_VALIDATED=1"));
        assert!(script.contains("External sandbox quality checking is independent"));
    }

    #[test]
    fn managed_runtime_layout_is_stable_and_downgrade_guarded() {
        let start = include_str!("../../../scripts/start-claude-science-wsl.sh");
        let windows_start = include_str!("../../../scripts/start-claude-science-wsl.ps1");
        let layout = include_str!("../../../scripts/csa-runtime-layout.sh");
        let service = include_str!("../../../scripts/install-wsl-bridge-service.sh");
        let inspect =
            include_str!("../../../skills/bootstrap-claude-science-wsl/scripts/inspect-wsl.sh");

        assert!(start.contains("csa_stage_bridge_runtime"));
        assert!(start.contains("csa_stage_claude_runtime"));
        assert!(start.contains("trap restore_runtime_after_failure EXIT"));
        assert!(start.contains("csa_print_bridge_identity"));
        assert!(start.contains("byok-demand-mcp-lazy-git-scan-v8"));
        assert!(start.contains("git_boot_warmup_old"));
        assert!(start.contains("conda_git_scan_old"));
        assert!(start.contains("conda_profile_old"));
        assert!(start.contains("Keep Fastify's NYz onReady hook intact"));
        assert!(start.contains("async _ensureGitScan()"));
        assert!(start.contains("curl --noproxy '*'"));
        assert!(windows_start.contains("CSA_REPAIR_DRVFS_GRANTS=1"));
        assert!(layout.contains("Implicit Claude Science downgrade rejected"));
        assert!(layout.contains("csa_backup_before_downgrade"));
        assert!(layout.contains("mv -Tf \"$temporary\" \"$link_path\""));
        assert!(service.contains("BRIDGE_CURRENT=\"$CSA_STATE_ROOT/runtime/bridge/current\""));
        assert!(service.contains("ExecStart=\"$python_escaped\" \"$proxy_escaped\""));
        assert!(!service.contains("proxy_escaped=\"$(unit_escape \"$PACKAGE_DIR/proxy.py\")\""));
        assert!(inspect.contains("bridge_proxy=\"$bridge_current/proxy.py\""));
        assert!(inspect.contains("broad_drvfs_write_grant_count"));
        assert!(inspect.contains("git_scan_boot_mode"));
    }

    #[test]
    fn powershell_wsl_scripts_keep_shell_text_out_of_outer_argv() {
        let windows_start = include_str!("../../../scripts/start-claude-science-wsl.ps1");
        let open_block = windows_start
            .split("if ($Open) {")
            .nth(1)
            .expect("Windows start script should retain the optional open block");
        assert!(open_block.contains("\"--\", \"printenv\", \"HOME\""));
        assert!(open_block.contains("\"--\", $managedClaude, \"url\""));
        assert!(!open_block.contains("\"bash\", \"-lc\""));
        assert!(!open_block.contains("$HOME/.local/share/csa"));
        assert!(!open_block.contains("$("));
        assert!(!open_block.contains('*'));

        let evidence = include_str!("../../../scripts/collect-acceptance-evidence.ps1");
        let status_block = evidence
            .split("$wslStatusScript = @'")
            .nth(1)
            .expect("evidence status script should use a literal stdin here-string");
        let argv_block = status_block
            .split("$wslArgs = @(")
            .nth(1)
            .and_then(|value| value.split("  )").next())
            .expect("evidence WSL argv block should be present");
        assert!(argv_block.contains("\"bash\""));
        assert!(argv_block.contains("\"-s\""));
        assert!(!argv_block.contains("-lc"));
        assert!(!argv_block.contains("$("));
        assert!(!argv_block.contains('*'));
        assert!(status_block.contains("-StandardInputText $wslStatusScript"));
        assert!(!evidence.contains("\"-lc\""));
    }

    #[test]
    fn validates_official_release_pointer_and_manifest() {
        assert!(valid_release_sha8("b7190511"));
        assert!(!valid_release_sha8("B7190511"));
        assert!(!valid_release_sha8("../latest"));

        let checksum = "c663367bbc7ec54e7d1e5a9102594a9e70804ed5070f5d7cd1117e665e3c376c";
        let text = format!(
            r#"{{"version":"0.1.25","sha8":"b7190511","buildDate":"2026-07-24","sha256":{{"linux-x64":"{checksum}"}}}}"#
        );
        let release = parse_official_release_manifest("b7190511", &text).unwrap();
        assert_eq!(release.version, "0.1.25");
        assert_eq!(release.sha8, "b7190511");
        assert!(parse_official_release_manifest("a8cf9eae", &text).is_err());
    }

    #[test]
    fn compares_runtime_versions_numerically() {
        assert!(release_is_newer("0.1.27", "0.1.25"));
        assert!(release_is_newer("0.2", "0.1.99"));
        assert!(!release_is_newer("0.1.25", "0.1.25"));
        assert!(!release_is_newer("invalid", "0.1.25"));
    }

    #[test]
    #[ignore = "requires the public Claude Science release index"]
    fn official_runtime_index_is_readable() {
        let status = get_runtime_update_status_impl().unwrap();
        assert_eq!(status.bundled_version, "0.1.25");
        assert!(valid_release_sha8(&status.latest.sha8));
        assert!(valid_release_sha8(&status.stable.sha8));
        assert!(!status.latest.version.is_empty());
        assert!(!status.stable.version.is_empty());
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
                "proxy_host": "attacker.example",
                "proxy_port": 80,
                "proxy_auth_mode": "required",
                "proxy_auth_token": "secret token"
            })),
            "http://127.0.0.1:9876/secret%20token/dashboard"
        );
    }

    #[test]
    fn dashboard_health_uses_windows_loopback_without_a_wsl_glob() {
        let command = dashboard_health_command();
        assert_eq!(command.get_program(), "curl.exe");
        let argv = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            argv,
            [
                "--noproxy",
                "127.0.0.1",
                "-fsS",
                "--connect-timeout",
                "0.4",
                "--max-time",
                "2",
                "http://127.0.0.1:9876/health",
            ]
        );
        assert!(!argv.iter().any(|arg| arg == "wsl.exe"));
    }

    #[test]
    fn direct_process_argv_never_carries_a_noproxy_glob() {
        let source = include_str!("lib.rs");
        let lines = source.lines().collect::<Vec<_>>();
        for pair in lines.windows(2) {
            if pair[0].trim() == "\"--noproxy\"," {
                assert_ne!(
                    pair[1].trim(),
                    "\"*\",",
                    "a literal no-proxy wildcard in direct argv can expand after wsl.exe reparses it"
                );
            }
        }
    }

    #[test]
    fn dashboard_reads_linux_config_only_for_required_auth() {
        assert!(!dashboard_requires_auth_config(&serde_json::json!({
            "proxy_auth_mode": "optional",
            "proxy_auth_configured": true
        })));
        assert!(!dashboard_requires_auth_config(&serde_json::json!({
            "proxy_auth_mode": "required",
            "proxy_auth_configured": false
        })));
        assert!(dashboard_requires_auth_config(&serde_json::json!({
            "proxy_auth_mode": "required",
            "proxy_auth_configured": true
        })));

        assert!(DASHBOARD_AUTH_CONFIG_SCRIPT.contains("python3 - <<'PY'"));
        assert!(DASHBOARD_AUTH_CONFIG_SCRIPT.contains("proxy_auth_token"));
        assert!(!DASHBOARD_AUTH_CONFIG_SCRIPT.contains("proxy_host"));
        assert!(!DASHBOARD_AUTH_CONFIG_SCRIPT.contains("proxy_port"));
        assert!(!DASHBOARD_AUTH_CONFIG_SCRIPT.contains("api_key"));
        assert!(!DASHBOARD_AUTH_CONFIG_SCRIPT.contains("--noproxy"));
    }

    #[test]
    fn dashboard_path_secret_never_crosses_the_frontend_invoke_boundary() {
        let frontend = include_str!("../../src/App.tsx");
        let open_start = frontend
            .find("async function openDashboard()")
            .expect("dashboard action should exist");
        let open_end = frontend[open_start..]
            .find("function openMigrationAssistant()")
            .map(|offset| open_start + offset)
            .expect("migration helper should follow dashboard action");
        let open_action = &frontend[open_start..open_end];
        assert!(open_action.contains("await invoke<void>(\"open_bridge_dashboard\")"));
        assert!(!open_action.contains("invoke<string>"));
        assert!(!open_action.contains("get_dashboard_url"));
        assert!(!open_action.contains("openUrl"));

        let backend = include_str!("lib.rs");
        let command_start = backend
            .find("async fn open_bridge_dashboard(app: tauri::AppHandle)")
            .expect("dashboard opener command should exist");
        let command_end = backend[command_start..]
            .find("fn stop_legacy_windows_bridge_impl()")
            .map(|offset| command_start + offset)
            .expect("legacy bridge action should follow dashboard opener");
        let command = &backend[command_start..command_end];
        assert!(command.contains("run_blocking(get_dashboard_url_impl)"));
        assert!(command.contains("app.opener().open_url(url"));
    }

    #[test]
    fn system_drive_detection_warns_only_for_windows_c_drive() {
        assert!(is_windows_system_drive(Some("C:")));
        assert!(is_windows_system_drive(Some("c:\\")));
        assert!(!is_windows_system_drive(Some("D:")));
        assert!(!is_windows_system_drive(Some("/mnt/c")));
        assert!(!is_windows_system_drive(None));
    }

    #[test]
    fn deep_network_readiness_requires_a_recent_nonfuture_timestamp() {
        let now = 10_000;
        assert!(deep_network_result_is_fresh(true, Some(now), now));
        assert!(deep_network_result_is_fresh(
            true,
            Some(now - NETWORK_DEEP_CACHE_MAX_AGE_SECONDS),
            now
        ));
        assert!(!deep_network_result_is_fresh(
            true,
            Some(now - NETWORK_DEEP_CACHE_MAX_AGE_SECONDS - 1),
            now
        ));
        assert!(!deep_network_result_is_fresh(true, Some(now + 1), now));
        assert!(!deep_network_result_is_fresh(true, None, now));
        assert!(!deep_network_result_is_fresh(false, Some(now), now));
    }

    #[test]
    fn recovered_transient_deep_daemon_state_is_not_current_health() {
        assert!(transient_deep_daemon_result_recovered(
            true,
            "daemon_mount_io_busy",
            true,
            true,
            "S",
            false,
        ));
        assert!(transient_deep_daemon_result_recovered(
            true,
            "contract_changed",
            true,
            true,
            "I",
            false,
        ));
        assert!(!transient_deep_daemon_result_recovered(
            true,
            "daemon_mount_io_busy",
            true,
            true,
            "D",
            true,
        ));
        assert!(!transient_deep_daemon_result_recovered(
            true, "http_502", false, false, "S", false,
        ));
        assert!(!transient_deep_daemon_result_recovered(
            false,
            "daemon_mount_io_busy",
            true,
            true,
            "S",
            false,
        ));
    }

    #[test]
    fn sandbox_readiness_requires_three_complete_role_pairs() {
        let mut network = WslProbeNetwork {
            sandbox_forwarder_count: 3,
            sandbox_forwarder_expected_count: 3,
            sandbox_forwarder_topology_state: "expected".into(),
            sandbox_http_forwarder_count: 3,
            sandbox_socks_forwarder_count: 3,
            sandbox_probe_identity: SANDBOX_NETWORK_PROBE_IDENTITY.into(),
            sandbox_probe_role: "analysis".into(),
            sandbox_probe_transport: "socks5h".into(),
            sandbox_egress_target: Some("pypi.org".into()),
            sandbox_egress_canary_identity: Some(SANDBOX_NETWORK_CANARY_IDENTITY.into()),
            ..Default::default()
        };
        assert!(sandbox_forwarder_topology_is_ready(&network));

        network.sandbox_forwarder_topology_state = "extended".into();
        network.sandbox_forwarder_count = 4;
        assert!(sandbox_forwarder_topology_is_ready(&network));

        network.sandbox_forwarder_topology_state = "incomplete".into();
        network.sandbox_forwarder_count = 3;
        assert!(!sandbox_forwarder_topology_is_ready(&network));

        network.sandbox_forwarder_topology_state = "expected".into();
        network.sandbox_forwarder_count = 2;
        assert!(!sandbox_forwarder_topology_is_ready(&network));

        network.sandbox_forwarder_count = 3;
        network.sandbox_forwarder_expected_count = 1;
        assert!(!sandbox_forwarder_topology_is_ready(&network));

        network.sandbox_forwarder_expected_count = 3;
        network.sandbox_probe_transport = "http".into();
        assert!(!sandbox_forwarder_topology_is_ready(&network));
    }

    #[test]
    fn sandbox_deep_readiness_binds_the_stable_single_probe_result() {
        let mut network = WslProbeNetwork {
            sandbox_contract_stable_during_probe: Some(true),
            sandbox_unix_socket_state: "connected".into(),
            sandbox_socks_handshake_state: "ok".into(),
            sandbox_egress_state: "ok".into(),
            sandbox_egress_failure_stage: "none".into(),
            sandbox_egress_http_status: Some(200),
            sandbox_forwarder_probe_count: 1,
            sandbox_forwarder_passed_count: 1,
            sandbox_forwarder_failed_count: 0,
            ..Default::default()
        };
        assert!(sandbox_deep_egress_is_ready(&network));

        network.sandbox_contract_stable_during_probe = Some(false);
        assert!(!sandbox_deep_egress_is_ready(&network));
        network.sandbox_contract_stable_during_probe = Some(true);
        network.sandbox_forwarder_failed_count = 1;
        assert!(!sandbox_deep_egress_is_ready(&network));
        network.sandbox_forwarder_failed_count = 0;
        network.sandbox_egress_http_status = Some(302);
        assert!(!sandbox_deep_egress_is_ready(&network));
    }

    #[test]
    fn shallow_wsl_probe_accepts_null_contract_stability() {
        let report: WslProbeReport = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "network": {
                "deep_checked": false,
                "sandbox_contract_stable_during_probe": null
            }
        }))
        .expect("a shallow probe may report unknown contract stability as null");

        assert!(report
            .network
            .sandbox_contract_stable_during_probe
            .is_none());
        assert!(!sandbox_deep_egress_is_ready(&report.network));
    }

    #[test]
    fn browser_preview_does_not_claim_network_readiness_while_stopped() {
        let source = include_str!("../../src/App.tsx");
        let preview_start = source
            .find("const browserPreviewStatus: SystemStatus = {")
            .expect("browser preview should exist");
        let preview_end = source[preview_start..]
            .find("const stateText:")
            .map(|offset| preview_start + offset)
            .expect("state text should follow browser preview");
        let preview = &source[preview_start..preview_end];
        assert!(preview.contains("state: \"stopped\""));
        assert!(preview.contains("proxyState: \"not_running\""));
        assert!(preview.contains("localReady: false"));
        assert!(preview.contains("ready: false"));
        assert!(preview.contains("deepChecked: false"));
    }

    #[cfg(windows)]
    fn live_bridge_json(path: &str) -> Result<serde_json::Value, String> {
        let mut command = background_command("curl.exe");
        command.args([
            "--fail",
            "--silent",
            "--show-error",
            "--noproxy",
            "127.0.0.1",
            "--connect-timeout",
            "1",
            "--max-time",
            "10",
            &format!("http://127.0.0.1:9876{path}"),
        ]);
        let output =
            command_output_with_timeout(command, Duration::from_secs(12), "读取本地 Bridge 诊断")?;
        if !output.status.success() {
            return Err(command_error_text(&output));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("本地 Bridge 诊断响应无法解析：{error}"))
    }

    #[cfg(windows)]
    fn live_bridge_request() -> Result<serde_json::Value, String> {
        let body = serde_json::json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 48,
            "messages": [{"role": "user", "content": "Reply only: SWITCH_OK"}]
        })
        .to_string();
        let mut command = background_command("curl.exe");
        command.args([
            "--fail",
            "--silent",
            "--show-error",
            "--noproxy",
            "127.0.0.1",
            "--connect-timeout",
            "2",
            "--max-time",
            "90",
            "-H",
            "content-type: application/json",
            "-X",
            "POST",
            "--data-binary",
            &body,
            "http://127.0.0.1:9876/v1/messages",
        ]);
        let output = command_output_with_timeout(
            command,
            Duration::from_secs(95),
            "发送本地 Bridge 切换验证请求",
        )?;
        if !output.status.success() {
            return Err(command_error_text(&output));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("Bridge 切换验证响应无法解析：{error}"))
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "mutates the current user's active API Key and sends one max_tokens=1 Bridge verification request"]
    fn live_api_key_switch_diagnostic() {
        let target_id = std::env::var("CSA_LIVE_SWITCH_API_KEY_ID")
            .expect("set CSA_LIVE_SWITCH_API_KEY_ID to a saved API Key id");
        let before_settings = load_settings();
        let target = before_settings
            .api_keys
            .iter()
            .find(|entry| entry.id == target_id)
            .cloned()
            .expect("target API Key should exist");
        let before_status = current_status();
        let before_health = live_bridge_json("/health").ok();

        let switch_result = activate_api_key_impl(target_id.clone());
        let after_status = current_status();
        let after_health = live_bridge_json("/health").ok();
        let recent = live_bridge_json("/api/recent-requests").ok();
        let routed = recent
            .as_ref()
            .and_then(|value| value.get("requests"))
            .and_then(serde_json::Value::as_array)
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("backend").and_then(serde_json::Value::as_str) != Some("local")
                })
            });

        let evidence = serde_json::json!({
            "target": {
                "id": target.id,
                "provider": target.provider_id,
                "model": target.model,
            },
            "before": {
                "bridgeRunning": before_status.bridge_running,
                "bridgeHealthy": before_status.bridge_healthy,
                "revision": before_health.as_ref().and_then(|value| value.get("config_revision")).and_then(serde_json::Value::as_str),
            },
            "switchReturnedOk": switch_result.is_ok(),
            "switchError": switch_result.as_ref().err(),
            "after": {
                "bridgeRunning": after_status.bridge_running,
                "bridgeHealthy": after_status.bridge_healthy,
                "revision": after_health.as_ref().and_then(|value| value.get("config_revision")).and_then(serde_json::Value::as_str),
                "forceModel": after_health.as_ref().and_then(|value| value.get("force_model")).and_then(serde_json::Value::as_str),
            },
            "verificationRequestObserved": routed.is_some(),
            "verificationRequestOk": routed.and_then(|value| value.get("status")).and_then(serde_json::Value::as_str) == Some("success"),
            "routedBackend": routed.and_then(|value| value.get("backend")).and_then(serde_json::Value::as_str),
            "routedModel": routed.and_then(|value| value.get("model")).and_then(serde_json::Value::as_str),
            "routedStatus": routed.and_then(|value| value.get("status")).and_then(serde_json::Value::as_str),
        });
        println!("CSA_SWITCH_EVIDENCE={evidence}");
    }

    #[cfg(windows)]
    fn restore_live_launcher_settings(settings: &LauncherSettings) -> Result<(), String> {
        let Some(active_id) = settings.active_api_key_id.as_deref() else {
            return persist_launcher_settings(settings);
        };
        let entry = settings
            .api_keys
            .iter()
            .find(|entry| entry.id == active_id)
            .cloned()
            .ok_or_else(|| "原活动 API Key 已不存在".to_string())?;
        let api_key = unprotect_api_key(&entry.encrypted_api_key)?;
        let active_binding = settings.active_role.as_deref().and_then(|role| {
            settings
                .role_bindings
                .iter()
                .find(|binding| binding.role == role && binding.api_key_id == entry.id)
        });
        let model = active_binding
            .map(|binding| binding.model.as_str())
            .unwrap_or(entry.model.as_str());
        let aliases = active_binding
            .map(|binding| aliases_for_role(&entry, &binding.model))
            .unwrap_or_else(|| entry.model_aliases.clone());
        let patch = bridge_config_patch_for_api_key(settings, &api_key, model, &aliases)?;
        commit_launcher_settings_with_bridge(settings, patch)
    }

    #[cfg(windows)]
    fn live_role_result(role: &str) -> Result<serde_json::Value, String> {
        activate_role_impl(role.to_string())?;
        let health = live_bridge_json("/health")?;
        live_bridge_request()?;
        let recent = live_bridge_json("/api/recent-requests")?;
        let routed = recent
            .get("requests")
            .and_then(serde_json::Value::as_array)
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("backend").and_then(serde_json::Value::as_str) != Some("local")
                })
            });
        Ok(serde_json::json!({
            "role": role,
            "forceModel": health.get("force_model"),
            "revision": health.get("config_revision"),
            "routedBackend": routed.and_then(|value| value.get("backend")),
            "routedModel": routed.and_then(|value| value.get("model")),
            "routedStatus": routed.and_then(|value| value.get("status")),
        }))
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "temporarily changes role mappings and sends two real requests"]
    fn live_subscription_role_switch_diagnostic() {
        let default_id =
            std::env::var("CSA_LIVE_DEFAULT_API_KEY_ID").expect("set CSA_LIVE_DEFAULT_API_KEY_ID");
        let fast_id =
            std::env::var("CSA_LIVE_FAST_API_KEY_ID").expect("set CSA_LIVE_FAST_API_KEY_ID");
        let original = load_settings();
        let run_result = (|| -> Result<serde_json::Value, String> {
            let default_entry = original
                .api_keys
                .iter()
                .find(|entry| entry.id == default_id)
                .ok_or_else(|| "默认角色测试订阅不存在".to_string())?;
            let fast_entry = original
                .api_keys
                .iter()
                .find(|entry| entry.id == fast_id)
                .ok_or_else(|| "快速角色测试订阅不存在".to_string())?;
            let bindings = vec![
                StoredRoleBinding {
                    role: "default".into(),
                    provider_id: default_entry.provider_id.clone(),
                    api_key_id: default_entry.id.clone(),
                    model: default_entry.model.clone(),
                },
                StoredRoleBinding {
                    role: "vision".into(),
                    provider_id: default_entry.provider_id.clone(),
                    api_key_id: default_entry.id.clone(),
                    model: default_entry.model.clone(),
                },
                StoredRoleBinding {
                    role: "fast".into(),
                    provider_id: fast_entry.provider_id.clone(),
                    api_key_id: fast_entry.id.clone(),
                    model: fast_entry.model.clone(),
                },
            ];
            save_role_bindings_impl(bindings)?;
            let default_result = live_role_result("default")?;
            let fast_result = live_role_result("fast")?;
            Ok(serde_json::json!({
                "default": default_result,
                "fast": fast_result,
            }))
        })();
        let restore_result = restore_live_launcher_settings(&original);
        println!(
            "CSA_ROLE_EVIDENCE={}",
            serde_json::json!({
                "run": run_result.as_ref().ok(),
                "runError": run_result.as_ref().err(),
                "restored": restore_result.is_ok(),
                "restoreError": restore_result.as_ref().err(),
            })
        );
        assert!(run_result.is_ok(), "role switch diagnostic failed");
        assert!(restore_result.is_ok(), "original settings restore failed");
    }

    #[test]
    fn stop_services_streams_bash_over_stdin_and_requires_verified_listener_owners() {
        let source = include_str!("lib.rs");
        let helper = source
            .split("fn run_wsl_bash_stdin_with_timeout(")
            .nth(1)
            .and_then(|value| {
                value
                    .split("fn run_wsl_default_user_script_with_guest_timeout(")
                    .next()
            })
            .expect("the general WSL stdin helper should be present");
        assert!(helper.contains("command_output_with_stdin_timeout"));
        assert!(helper.contains("script.as_bytes()"));
        let stop_transport = source
            .split("fn stop_services_raw(distro: &str)")
            .nth(1)
            .and_then(|value| value.split("fn stop_services_impl()").next())
            .expect("the stop transport should be present");
        assert!(stop_transport.contains("run_wsl_bash_stdin_with_timeout"));
        assert!(stop_transport.contains("STOP_SERVICES_SCRIPT"));
        assert!(!stop_transport.contains("-lc"));

        let command = wsl_bash_stdin_command("Ubuntu-24.04");
        let argv = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(argv, ["--distribution", "Ubuntu-24.04", "--", "bash", "-s"]);
        let outer_expanded = argv
            .iter()
            .map(|arg| {
                arg.replace('*', "OUTER_GLOB")
                    .replace("$(", "OUTER_SUBSHELL(")
            })
            .collect::<Vec<_>>();
        assert_eq!(outer_expanded, argv);
        assert!(STOP_SERVICES_SCRIPT.contains("$("));
        assert!(STOP_SERVICES_SCRIPT.contains("--noproxy '*'"));
        assert!(START_SERVICES_TIMEOUT >= Duration::from_secs(120));
        assert!(STOP_SERVICES_TIMEOUT >= Duration::from_secs(45));
        assert!(STOP_SERVICES_SCRIPT.contains("grep -oE 'pid=[0-9]+'"));
        assert!(STOP_SERVICES_SCRIPT.contains("runtime/lifecycle.lock"));
        assert!(STOP_SERVICES_SCRIPT.contains("flock -w"));
        assert!(STOP_SERVICES_SCRIPT.contains("managed_claude_pid \"$pid\" ||"));
        assert!(STOP_SERVICES_SCRIPT.contains("verified_bridge_pid \"$pid\" ||"));
        assert!(STOP_SERVICES_SCRIPT.contains("Refusing to stop unverified owner PID"));
        assert!(STOP_SERVICES_SCRIPT.contains("bridge_unit_matches_managed_runtime"));
        assert!(STOP_SERVICES_SCRIPT.contains("bridge_unit_matches_verified_listener"));
        assert!(STOP_SERVICES_SCRIPT.contains(
            "Refusing to stop claude-science-bridge.service because its unit identity is not owned"
        ));
        assert!(STOP_SERVICES_SCRIPT
            .contains("if [ \"$bridge_unit_owned\" = \"1\" ]; then\n  systemctl --user stop"));
        assert!(STOP_SERVICES_SCRIPT.contains("manifest.get(\"product\")"));
        assert!(!STOP_SERVICES_SCRIPT.contains("ps -eo pid=,args="));
        assert!(STOP_SERVICES_SCRIPT.contains("process_threads_signalable \"$pid\""));
        let claude_term = STOP_SERVICES_SCRIPT
            .find("kill \"$pid\" 2>/dev/null || true")
            .expect("Claude TERM should be present");
        let bridge_stop = STOP_SERVICES_SCRIPT
            .find("systemctl --user stop claude-science-bridge.service")
            .expect("owned Bridge stop should be present");
        assert!(claude_term < bridge_stop);
        assert!(!STOP_SERVICES_SCRIPT.contains("wsl --shutdown"));
        assert!(!STOP_SERVICES_SCRIPT.contains("wsl --terminate"));
    }
}
