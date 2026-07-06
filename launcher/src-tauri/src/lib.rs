use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn background_command(program: &str) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
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
    warnings: Vec<String>,
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
    let output = background_command("wsl.exe")
        .args(["--list", "--quiet"])
        .output()
        .map_err(|error| format!("无法运行 wsl.exe：{error}"))?;
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
    background_command("wsl.exe")
        .arg("--distribution")
        .arg(distro)
        .arg("--")
        .args(args)
        .output()
        .map_err(|error| format!("无法调用 WSL {distro}：{error}"))
}

fn wsl_shell(distro: &str, script: &str) -> Result<Output, String> {
    run_wsl(distro, &["sh", "-lc", script])
}

fn parse_first_pid(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| line.trim().parse::<u32>().ok())
}

fn process_pid(distro: &str, pattern: &str) -> Option<u32> {
    let script = format!("pgrep -f -- '{}' | head -n 1", pattern.replace('\'', ""));
    wsl_shell(distro, &script)
        .ok()
        .and_then(|output| parse_first_pid(&output_text(&output)))
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
    background_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()
        .and_then(|output| parse_first_pid(&output_text(&output)))
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
            warnings,
        };
    };

    if !distro.eq_ignore_ascii_case("Ubuntu-24.04") {
        warnings.push(format!("推荐使用 Ubuntu-24.04；当前兼容使用 {}。", distro));
    }

    let linux_user = run_wsl(&distro, &["id", "-un"])
        .ok()
        .map(|output| output_text(&output))
        .filter(|text| !text.is_empty());
    let bridge_pid = process_pid(&distro, "[p]ython.*claude-science-api-bridge.*/proxy.py");
    let claude_pid = process_pid(
        &distro,
        "claude-science-api-bridge/patched/[c]laude-science serve",
    );
    let source_binary_present =
        wsl_path_exists(
            &distro,
            "$HOME/.local/share/claude-science-api-bridge/bin/claude-science",
            true,
        ) || wsl_path_exists(&distro, "$HOME/.local/bin/claude-science", true);
    let bridge_venv_present = wsl_path_exists(
        &distro,
        "$HOME/.local/share/claude-science-api-bridge/venv/bin/python",
        true,
    );
    let project_files_present = project_runtime_files_present();
    let runtime_ready = source_binary_present && bridge_venv_present && project_files_present;
    let bridge_healthy = bridge_pid.is_some()
        && wsl_shell(
            &distro,
            "curl -fsS --max-time 2 http://127.0.0.1:9876/health >/dev/null 2>&1",
        )
        .map(|output| output.status.success())
        .unwrap_or(false);
    let windows_bridge_pid = legacy_windows_bridge_pid();

    if bridge_pid.is_some() && !bridge_healthy {
        warnings.push("Bridge 进程存在，但健康检查失败".into());
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

    let state = if !runtime_ready && bridge_pid.is_none() && claude_pid.is_none() {
        "notInstalled"
    } else if bridge_pid.is_some() && windows_bridge_pid.is_some() {
        "degraded"
    } else if bridge_healthy && claude_pid.is_some() {
        "running"
    } else if bridge_pid.is_some() || claude_pid.is_some() {
        "degraded"
    } else {
        "stopped"
    };

    SystemStatus {
        state: state.into(),
        wsl_installed: true,
        distro: Some(distro),
        linux_user,
        bridge_running: bridge_pid.is_some(),
        bridge_pid,
        claude_running: claude_pid.is_some(),
        claude_pid,
        bridge_healthy,
        windows_bridge_pid,
        runtime_ready,
        source_binary_present,
        bridge_venv_present,
        warnings,
    }
}

fn wsl_path_exists(distro: &str, path: &str, executable: bool) -> bool {
    let escaped = path.replace('"', "\\\"");
    let flag = if executable { "-x" } else { "-e" };
    let script = format!("test {flag} \"{escaped}\"");
    wsl_shell(distro, &script)
        .map(|output| output.status.success())
        .unwrap_or(false)
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

fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "配置路径无父目录".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建配置目录：{error}"))?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut file =
            fs::File::create(&tmp).map_err(|error| format!("无法写入临时配置：{error}"))?;
        file.write_all(content.as_bytes())
            .map_err(|error| format!("无法写入配置内容：{error}"))?;
        file.sync_all()
            .map_err(|error| format!("无法同步配置：{error}"))?;
    }
    fs::rename(&tmp, path).map_err(|error| format!("无法替换配置文件：{error}"))
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
                    default_model: Some("glm-5.2".into()),
                },
                ProviderPreset {
                    id: "longcat".into(),
                    name: "LongCat".into(),
                    meta: "Anthropic 兼容".into(),
                    badge: "官方".into(),
                    trust: "official".into(),
                    protocol: "openai-compatible".into(),
                    base_url: Some("https://api.longcat.chat/openai".into()),
                    default_model: Some("LongCat-2.0".into()),
                },
                ProviderPreset {
                    id: "deepseek".into(),
                    name: "DeepSeek".into(),
                    meta: "官方 API".into(),
                    badge: "官方".into(),
                    trust: "official".into(),
                    protocol: "anthropic-compatible".into(),
                    base_url: Some("https://api.deepseek.com/anthropic".into()),
                    default_model: Some("deepseek-v4-pro".into()),
                },
                ProviderPreset {
                    id: "minimax".into(),
                    name: "MiniMax".into(),
                    meta: "Official API / Anthropic compatible".into(),
                    badge: "瀹樻柟".into(),
                    trust: "official".into(),
                    protocol: "anthropic-compatible".into(),
                    base_url: Some("https://api.minimax.io/anthropic".into()),
                    default_model: Some("MiniMax-M3".into()),
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
                    default_model: Some("gpt-5.5".into()),
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
                    default_model: Some("glm-5.2".into()),
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
            title: "第三方中转".into(),
            tier: "custom".into(),
            providers: vec![
                ProviderPreset {
                    id: "builtin-relay".into(),
                    name: "内置中转".into(),
                    meta: "10521052.xyz/v1".into(),
                    badge: "中转".into(),
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
    let mut child = background_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
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
            default_model: "glm-5.2".into(),
            default_fast_model: "glm-5.2".into(),
            requires_explicit_model: false,
        },
        "longcat" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "custom",
            api_key_field: "custom_api_key",
            base_url: "https://api.longcat.chat/openai".into(),
            upstream_mode: "openai",
            default_model: "LongCat-2.0".into(),
            default_fast_model: "LongCat-2.0".into(),
            requires_explicit_model: false,
        },
        "deepseek" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "deepseek",
            api_key_field: "deepseek_api_key",
            base_url: "https://api.deepseek.com/anthropic".into(),
            upstream_mode: "anthropic",
            default_model: "deepseek-v4-pro".into(),
            default_fast_model: "deepseek-v4-flash".into(),
            requires_explicit_model: false,
        },
        "minimax" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "custom",
            api_key_field: "custom_api_key",
            base_url: "https://api.minimax.io/anthropic".into(),
            upstream_mode: "anthropic",
            default_model: "MiniMax-M3".into(),
            default_fast_model: "MiniMax-M2.7-highspeed".into(),
            requires_explicit_model: false,
        },
        "claude" => return Ok(None),
        "openai" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "openai",
            api_key_field: "openai_api_key",
            base_url: "https://api.openai.com/v1".into(),
            upstream_mode: "openai",
            default_model: "gpt-5.5".into(),
            default_fast_model: "gpt-5.5".into(),
            requires_explicit_model: false,
        },
        "opencode-go" => BridgeRuntimeProfile {
            provider_id: provider_id.into(),
            label: provider.name,
            backend: "custom",
            api_key_field: "custom_api_key",
            base_url: "https://opencode.ai/zen/go/v1".into(),
            upstream_mode: "openai",
            default_model: "glm-5.2".into(),
            default_fast_model: "deepseek-v4-flash".into(),
            requires_explicit_model: false,
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
    if !profile.default_model.trim().is_empty() {
        return Ok(profile.default_model.trim().to_string());
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
            "deep-chat" | "deepseek-chat" | "deepseek-reasoner" | "deepseek-v4" | "glm-5.2" => {
                "deepseek-v4-pro".into()
            }
            "deepseek-fast" | "deepseek-v4-fast" | "deepseek-v4-flash" => {
                "deepseek-v4-flash".into()
            }
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
            "minimax-m2.7-highspeed" => "MiniMax-M2.7-highspeed".into(),
            "minimax-m2.5-highspeed" => "MiniMax-M2.5-highspeed".into(),
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
            | "kimi-k2.7-code"
            | "kimi-k2.6"
            | "deepseek-v4-pro"
            | "deepseek-v4-flash"
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
        profile.default_model.trim().to_string()
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
        patch.insert("model_list_mode".into(), "aliases_first".into());
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

fn restart_bridge_after_config(status: &SystemStatus) -> Result<(), String> {
    if !status.bridge_running {
        return Ok(());
    }
    let Some(distro) = status.distro.as_deref() else {
        return Ok(());
    };
    let restart_output = run_wsl(
        distro,
        &[
            "systemctl",
            "--user",
            "restart",
            "claude-science-bridge.service",
        ],
    )?;
    if !restart_output.status.success() {
        return Err(format!(
            "Bridge 配置已写入，但重启失败：{}",
            command_error_text(&restart_output)
        ));
    }

    let health_script = r#"
import sys
import time
import urllib.error
import urllib.request

last_error = ""
for _ in range(20):
    try:
        with urllib.request.urlopen("http://127.0.0.1:9876/health", timeout=1) as response:
            if response.status == 200:
                sys.exit(0)
            last_error = f"HTTP {response.status}"
    except Exception as error:
        last_error = str(error)
    time.sleep(0.5)

print(last_error or "health endpoint did not become ready", file=sys.stderr)
sys.exit(1)
"#;
    let health_output = run_wsl(distro, &["python3", "-c", health_script])?;
    if health_output.status.success() {
        return Ok(());
    }
    Err(format!(
        "Bridge 配置已写入，但健康检查失败：{}",
        command_error_text(&health_output)
    ))
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

fn apply_bridge_config_patch_value(patch: serde_json::Value) -> Result<(), String> {
    let status = current_status();
    let Some(distro) = status.distro.as_deref() else {
        return Err("未检测到可用 WSL 发行版，暂不能应用 Provider 配置".into());
    };
    let rollback = write_bridge_config_patch(distro, &patch)?;
    match restart_bridge_after_config(&status) {
        Ok(()) => Ok(()),
        Err(error) => {
            let rollback_message = match restore_bridge_config(distro, &rollback) {
                Ok(()) => {
                    let _ = restart_bridge_after_config(&status);
                    "已回滚 Bridge 配置".to_string()
                }
                Err(rollback_error) => format!("回滚失败：{rollback_error}"),
            };
            Err(format!("{error}；{rollback_message}"))
        }
    }
}

fn apply_bridge_provider_config(settings: &LauncherSettings) -> Result<(), String> {
    let Some(patch) = bridge_config_patch_for_provider(settings)? else {
        return Ok(());
    };
    apply_bridge_config_patch_value(patch)
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

#[tauri::command]
fn test_api_key(
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
        return Err("第三方中转需要先确认域名后再测试，避免 API Key 发到错误地址。".into());
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
        "prompt": clean_prompt
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
$prompt = ([string]$req.prompt).Trim()
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
  $candidates = New-Object System.Collections.Generic.List[string]
  foreach ($item in @($requestedModel, $defaultModel, "deepseek-v4-pro", "deepseek-v4-flash", "MiniMax-M3", "LongCat-2.0", "claude-3-5-haiku-latest")) {
    if ($item -and -not $candidates.Contains($item)) { [void]$candidates.Add($item) }
  }
  foreach ($item in $models) {
    if ($item -and -not $candidates.Contains($item)) { [void]$candidates.Add($item) }
    if ($candidates.Count -ge 8) { break }
  }
  $lastError = ""
  foreach ($candidate in $candidates) {
    $body = @{
      model = $candidate
      max_tokens = 8
      messages = @(@{ role = "user"; content = $prompt })
    } | ConvertTo-Json -Depth 8 -Compress
    try {
      $chat = Invoke-RestMethod -Method Post -Uri "$anthropicBase/messages" -Headers $headers -Body $body -TimeoutSec 18
      $reply = ""
      foreach ($part in @($chat.content)) {
        if ($part.type -eq "text") { $reply += [string]$part.text }
      }
      if ($reply -and $reply.Trim()) {
        Emit $true $candidate $reply $models "连通成功。"
      }
      $lastError = "HTTP 200，但模型返回空内容：" + $candidate
    } catch {
      $lastError = ShortError $_
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
  if (-not $requestedModel -and -not $defaultModel) {
    Emit $false "" "" @() ("无法读取模型列表：" + $last)
  }
}

$preferred = @(
  $requestedModel,
  $defaultModel,
  "glm-5.2",
  "qwen3.7-max",
  "deepseek-v4-pro",
  "deepseek-v4-flash",
  "grok-4.20-fast",
  "LongCat-2.0",
  "gpt-4o-mini",
  "gpt-4.1-mini",
  "glm-4.5",
  "glm-4.5-air",
  "step-router-v1",
  "deepseek-ai/deepseek-v3.2",
  "deepseek-ai/deepseek-v3.1-terminus",
  "MiniMax-M3",
  "MiniMax-M2.7-highspeed",
  "minimax-m2.7",
  "minimax-m2"
)
$candidates = New-Object System.Collections.Generic.List[string]
foreach ($item in $preferred) {
  if ($item -and ($models.Count -eq 0 -or $models -contains $item) -and -not $candidates.Contains($item)) {
    [void]$candidates.Add($item)
  }
}
foreach ($item in $models) {
  if ($item -and -not $candidates.Contains($item)) { [void]$candidates.Add($item) }
  if ($candidates.Count -ge 10) { break }
}

$lastError = ""
foreach ($candidate in $candidates) {
  $body = @{
    model = $candidate
    messages = @(@{ role = "user"; content = $prompt })
    max_tokens = 8
    temperature = 0
    stream = $false
  } | ConvertTo-Json -Depth 8 -Compress
  try {
    $chat = Invoke-RestMethod -Method Post -Uri "$openaiBase/chat/completions" -Headers $headers -Body $body -TimeoutSec 18
    $reply = [string]$chat.choices[0].message.content
    if ($reply -and $reply.Trim()) {
      Emit $true $candidate $reply $models "连通成功。"
    }
    $lastError = "HTTP 200，但模型返回空内容：" + $candidate
  } catch {
    $lastError = ShortError $_
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
fn auto_map_api_key(
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
        return Err("第三方中转需要先确认域名后再自动映射，避免 API Key 发到错误地址。".into());
    }
    let Some(profile) =
        runtime_profile_for_provider(&selected_provider_id, &custom_base_url, custom_confirmed)?
    else {
        return Err("Claude 官方登录模式不需要在这里自动映射模型。".into());
    };
    let fallback_model = if model.trim().is_empty() {
        canonical_model_for_profile(&profile, &profile.default_model)
    } else {
        canonical_model_for_profile(&profile, &model)
    };
    let payload = serde_json::json!({
        "provider_id": profile.provider_id.clone(),
        "api_key": clean_key,
        "base_url": profile.base_url.clone(),
        "upstream_mode": profile.upstream_mode,
    });
    let script = r#"
$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13
$req = [Console]::In.ReadToEnd() | ConvertFrom-Json
$apiKey = [string]$req.api_key
$baseUrl = ([string]$req.base_url).Trim().TrimEnd("/")
$upstreamMode = ([string]$req.upstream_mode).Trim().ToLowerInvariant()

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
    Emit $models "模型列表读取成功。"
  } catch {
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
  Emit $models "模型列表读取成功。"
} catch {
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

fn persist_launcher_settings(settings: &LauncherSettings) -> Result<(), String> {
    let body = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("无法序列化配置：{error}"))?;
    atomic_write(&settings_path()?, &(body + "\n"))
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
fn save_provider_selection(
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
    apply_bridge_provider_config(&settings)?;
    persist_launcher_settings(&settings)?;
    Ok(launcher_state(&settings))
}

#[tauri::command]
fn save_api_key(
    selected_provider_id: String,
    api_key: String,
    custom_base_url: String,
    custom_confirmed: bool,
    model: String,
    model_aliases: Vec<StoredModelAlias>,
) -> Result<LauncherState, String> {
    let provider =
        provider_by_id(&selected_provider_id).ok_or_else(|| "未知 API Key 服务商".to_string())?;
    if provider.trust.starts_with("untrusted") && !custom_confirmed {
        return Err("第三方中转需要确认后才能保存 API Key".into());
    }
    if selected_provider_id == "custom" && custom_base_url.trim().is_empty() {
        return Err("确认自定义中转前，请先填写 Base URL".into());
    }
    let clean_key = api_key.trim();
    if selected_provider_id != "claude" && clean_key.is_empty() {
        return Err("添加 API Key 时请填写 Key；已保存的 Key 可直接从列表切换".into());
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
        primary_model_from_aliases(&sanitized_aliases)
            .or_else(|| {
                runtime_profile
                    .as_ref()
                    .map(|profile| profile.default_model.clone())
            })
            .unwrap_or_default()
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
    if let Some(patch) =
        bridge_config_patch_for_api_key(&settings, clean_key, &stored_model, &sanitized_aliases)?
    {
        apply_bridge_config_patch_value(patch)?;
    }
    let entry = StoredApiKey {
        id: next_api_key_id(),
        provider_id: selected_provider_id,
        label: provider.name,
        base_url: validated_base_url,
        model: stored_model,
        custom_confirmed,
        model_aliases: sanitized_aliases,
        encrypted_api_key,
    };
    settings.active_api_key_id = Some(entry.id.clone());
    settings.api_keys.push(entry);
    persist_launcher_settings(&settings)?;
    Ok(launcher_state(&settings))
}

#[tauri::command]
fn activate_api_key(api_key_id: String) -> Result<LauncherState, String> {
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
    if let Some(patch) =
        bridge_config_patch_for_api_key(&settings, &api_key, &entry.model, &entry.model_aliases)?
    {
        apply_bridge_config_patch_value(patch)?;
    }
    settings.active_api_key_id = Some(entry.id);
    persist_launcher_settings(&settings)?;
    Ok(launcher_state(&settings))
}

#[tauri::command]
fn delete_api_key(api_key_id: String) -> Result<LauncherState, String> {
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
fn get_system_status() -> SystemStatus {
    current_status()
}

#[tauri::command]
fn start_services() -> Result<SystemStatus, String> {
    let before = current_status();
    if before.state == "running" {
        return Ok(before);
    }
    let distro = before
        .distro
        .ok_or_else(|| "请先安装 WSL2 和 Ubuntu".to_string())?;
    let user = before
        .linux_user
        .ok_or_else(|| "无法确定 WSL 默认用户".to_string())?;
    let script = project_root()?
        .join("scripts")
        .join("start-claude-science-wsl.ps1");
    let output = background_command("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .arg("-Distro")
        .arg(&distro)
        .arg("-User")
        .arg(&user)
        .output()
        .map_err(|error| format!("启动失败：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Claude Science 启动失败：{}",
            command_error_text(&output)
        ));
    }
    Ok(current_status())
}

#[tauri::command]
fn stop_services() -> Result<SystemStatus, String> {
    let before = current_status();
    let Some(distro) = before.distro else {
        return Ok(before);
    };
    let script = r#"
systemctl --user stop claude-science-bridge.service >/dev/null 2>&1 || true
for pid in $(ps -eo pid=,args= | awk '/claude-science-api-bridge\/patched\/claude-science serve/ && !/awk/ {print $1}'); do kill "$pid" 2>/dev/null || true; done
for pid in $(ps -eo pid=,args= | awk '/python.*claude-science-api-bridge.*\/proxy.py/ && !/awk/ {print $1}'); do kill "$pid" 2>/dev/null || true; done
"#;
    let output = wsl_shell(&distro, script)?;
    if !output.status.success() {
        return Err(format!("停止服务失败：{}", command_error_text(&output)));
    }
    Ok(current_status())
}

#[tauri::command]
fn restart_services() -> Result<SystemStatus, String> {
    stop_services()?;
    start_services()
}

#[tauri::command]
fn get_claude_url() -> Result<String, String> {
    let status = current_status();
    let distro = status.distro.ok_or_else(|| "WSL 不可用".to_string())?;
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
fn get_dashboard_url() -> Result<String, String> {
    let status = current_status();
    let distro = status.distro.ok_or_else(|| "WSL 不可用".to_string())?;
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
fn stop_legacy_windows_bridge() -> Result<SystemStatus, String> {
    let Some(pid) = legacy_windows_bridge_pid() else {
        return Ok(current_status());
    };
    let command = format!("Stop-Process -Id {pid} -Force -ErrorAction Stop");
    let output = background_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &command])
        .output()
        .map_err(|error| format!("无法停止旧 Windows Bridge：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "无法停止旧 Windows Bridge：{}",
            command_error_text(&output)
        ));
    }
    Ok(current_status())
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
        assert_eq!(glm.default_model.as_deref(), Some("glm-5.2"));
        let deepseek = &catalog[0].providers[2];
        assert_eq!(
            deepseek.base_url.as_deref(),
            Some("https://api.deepseek.com/anthropic")
        );
        assert_eq!(deepseek.default_model.as_deref(), Some("deepseek-v4-pro"));
        let minimax = &catalog[0].providers[3];
        assert_eq!(
            minimax.base_url.as_deref(),
            Some("https://api.minimax.io/anthropic")
        );
        let openai = &catalog[0].providers[5];
        assert_eq!(openai.default_model.as_deref(), Some("gpt-5.5"));
        let opencode_go = &catalog[1].providers[0];
        assert_eq!(opencode_go.default_model.as_deref(), Some("glm-5.2"));
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

        let patch_without_key = bridge_config_patch_for_api_key(&settings, "", "", &[])
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
    fn deepseek_profile_uses_v4_and_maps_fast_role() {
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
        assert_eq!(patch["force_model"], "deepseek-v4-pro");
        let rows = patch["model_aliases"].as_array().unwrap();
        let haiku = rows
            .iter()
            .find(|row| row["id"] == "claude-haiku-4-5-20251001")
            .unwrap();
        assert_eq!(haiku["model"], "deepseek-v4-flash");

        let stale_glm_patch =
            bridge_config_patch_for_api_key(&settings, "deepseek-key", "glm-5.2", &[])
                .unwrap()
                .unwrap();
        assert_eq!(stale_glm_patch["force_model"], "deepseek-v4-pro");
    }

    #[test]
    fn opencode_go_defaults_to_glm_and_fast_maps_to_deepseek_flash() {
        let settings = LauncherSettings {
            selected_provider_id: "opencode-go".into(),
            custom_base_url: String::new(),
            custom_confirmed: false,
            ..LauncherSettings::default()
        };
        let patch = bridge_config_patch_for_api_key(&settings, "opencode-key", "", &[])
            .unwrap()
            .unwrap();
        assert_eq!(patch["default_backend"], "custom");
        assert_eq!(patch["custom_base_url"], "https://opencode.ai/zen/go/v1");
        assert_eq!(patch["custom_upstream_mode"], "openai");
        assert_eq!(patch["force_model"], "glm-5.2");
        let rows = patch["model_aliases"].as_array().unwrap();
        let haiku = rows
            .iter()
            .find(|row| row["id"] == "claude-haiku-4-5-20251001")
            .unwrap();
        assert_eq!(haiku["model"], "deepseek-v4-flash");

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
                "deepseek-v4-pro".to_string(),
                "deepseek-v4-flash".to_string()
            ]
        );
        assert_eq!(fallback, "glm-5.2");

        let (primary, fast, aliases, candidates) = auto_model_mapping(&models, &fallback).unwrap();
        assert_eq!(primary, "glm-5.2");
        assert_eq!(fast, "deepseek-v4-flash");
        assert!(!candidates.contains(&"qwen3.7-max".to_string()));
        assert!(!candidates.contains(&"MiniMax-M3".to_string()));
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
