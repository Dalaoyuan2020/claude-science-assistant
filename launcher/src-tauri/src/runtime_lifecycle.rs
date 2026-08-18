use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::{ffi::c_void, os::windows::ffi::OsStrExt};

#[cfg(windows)]
#[link(name = "Kernel32")]
extern "system" {
    fn CreateMutexW(
        mutex_attributes: *const c_void,
        initial_owner: i32,
        name: *const u16,
    ) -> *mut c_void;
    fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
    fn ReleaseMutex(handle: *mut c_void) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

#[cfg(windows)]
const WAIT_OBJECT_0: u32 = 0;
#[cfg(windows)]
const WAIT_ABANDONED: u32 = 0x0000_0080;
#[cfg(windows)]
const WAIT_TIMEOUT: u32 = 0x0000_0102;

pub(crate) const RUNTIME_IDENTITY_PREFIX: &str = "CSA_RUNTIME_IDENTITY=";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeIdentity {
    pub schema_version: u32,
    pub component: String,
    pub runtime_id: String,
    pub version: String,
    pub build_id: String,
    pub source_path: String,
    pub source_sha256: String,
    pub pid: u32,
    pub capabilities: Vec<String>,
    pub managed: bool,
}

impl RuntimeIdentity {
    pub(crate) fn validate_bridge(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "Bridge runtime identity schema is unsupported: {}",
                self.schema_version
            ));
        }
        if self.component != "bridge" || !self.managed {
            return Err("Bridge did not report a managed runtime identity".into());
        }
        if self.runtime_id.is_empty()
            || !self
                .runtime_id
                .chars()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, '.' | '-' | '_'))
        {
            return Err("Bridge runtime identity contains an invalid runtimeId".into());
        }
        if self.version.trim().is_empty() {
            return Err("Bridge runtime identity is missing its version".into());
        }
        if self.build_id.len() != 16
            || !self.build_id.chars().all(|value| value.is_ascii_hexdigit())
        {
            return Err("Bridge runtime identity contains an invalid buildId".into());
        }
        if self.source_path.trim().is_empty() {
            return Err("Bridge runtime identity is missing its sourcePath".into());
        }
        if self.source_sha256.len() != 64
            || !self
                .source_sha256
                .chars()
                .all(|value| value.is_ascii_hexdigit())
        {
            return Err("Bridge runtime identity contains an invalid SHA-256".into());
        }
        if !self
            .build_id
            .eq_ignore_ascii_case(&self.source_sha256[..16])
        {
            return Err("Bridge runtime identity buildId does not match its source SHA-256".into());
        }
        if self.pid == 0 {
            return Err("Bridge runtime identity contains an invalid PID".into());
        }
        if !self.capabilities.iter().any(|value| value == "health") {
            return Err("Bridge runtime identity is missing the health capability".into());
        }
        Ok(())
    }
}

pub(crate) fn parse_runtime_identity(output: &str) -> Result<RuntimeIdentity, String> {
    let payload = output
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix(RUNTIME_IDENTITY_PREFIX))
        .ok_or_else(|| "Service start completed without a runtime identity marker".to_string())?;
    let identity: RuntimeIdentity = serde_json::from_str(payload)
        .map_err(|error| format!("Runtime identity marker is invalid: {error}"))?;
    identity.validate_bridge()?;
    Ok(identity)
}

pub(crate) fn runtime_identity_from_health(
    health: &serde_json::Value,
) -> Result<RuntimeIdentity, String> {
    let value = health
        .get("runtime_identity")
        .cloned()
        .ok_or_else(|| "Bridge health is missing its managed runtime identity".to_string())?;
    let identity: RuntimeIdentity = serde_json::from_value(value)
        .map_err(|error| format!("Bridge health runtime identity is invalid: {error}"))?;
    identity.validate_bridge()?;
    Ok(identity)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationOwner<'a> {
    schema_version: u32,
    pid: u32,
    operation: &'a str,
    operation_id: &'a str,
    started_at_unix_ms: u128,
}

pub(crate) struct ServiceOperationLock {
    _file: File,
    #[allow(dead_code)]
    path: PathBuf,
    #[cfg(windows)]
    mutex_handle: *mut c_void,
    #[cfg(not(windows))]
    #[allow(dead_code)]
    fallback_guard: std::sync::MutexGuard<'static, ()>,
}

impl ServiceOperationLock {
    pub(crate) fn acquire(path: &Path, operation: &str, timeout: Duration) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Unable to create CSA state directory: {error}"))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| format!("Unable to open CSA service operation lock: {error}"))?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let operation_id = format!("{}-{}", std::process::id(), now.as_nanos());
        let owner = OperationOwner {
            schema_version: 1,
            pid: std::process::id(),
            operation,
            operation_id: &operation_id,
            started_at_unix_ms: now.as_millis(),
        };
        let body = serde_json::to_vec(&owner)
            .map_err(|error| format!("Unable to serialize service lock owner: {error}"))?;

        #[cfg(windows)]
        let mutex_handle = acquire_windows_mutex(path, timeout)?;

        #[cfg(not(windows))]
        let fallback_guard = acquire_fallback_mutex(timeout)?;

        let write_result = file
            .set_len(0)
            .and_then(|_| file.seek(SeekFrom::Start(0)))
            .and_then(|_| file.write_all(&body))
            .and_then(|_| file.sync_all());
        if let Err(error) = write_result {
            #[cfg(windows)]
            unsafe {
                ReleaseMutex(mutex_handle);
                CloseHandle(mutex_handle);
            }
            return Err(format!("Unable to record service lock owner: {error}"));
        }

        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
            #[cfg(windows)]
            mutex_handle,
            #[cfg(not(windows))]
            fallback_guard,
        })
    }
}

impl Drop for ServiceOperationLock {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            ReleaseMutex(self.mutex_handle);
            CloseHandle(self.mutex_handle);
        }
    }
}

fn lock_name_hash(path: &Path) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in path.to_string_lossy().to_ascii_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(windows)]
fn acquire_windows_mutex(path: &Path, timeout: Duration) -> Result<*mut c_void, String> {
    let name = format!("Local\\CSA.ServiceLifecycle.{:016x}", lock_name_hash(path));
    let wide: Vec<u16> = std::ffi::OsStr::new(&name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
    if handle.is_null() {
        return Err("Unable to create the CSA service operation mutex".into());
    }
    let timeout_ms = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
    let result = unsafe { WaitForSingleObject(handle, timeout_ms) };
    if result == WAIT_OBJECT_0 || result == WAIT_ABANDONED {
        return Ok(handle);
    }
    unsafe {
        CloseHandle(handle);
    }
    if result == WAIT_TIMEOUT {
        let owner = fs::read_to_string(path).unwrap_or_default();
        let owner = owner.trim();
        let suffix = if owner.is_empty() {
            String::new()
        } else {
            format!(" Current owner: {owner}")
        };
        Err(format!(
            "Another CSA service operation is still running.{suffix}"
        ))
    } else {
        Err(format!(
            "Unable to acquire the CSA service operation mutex (wait result {result})"
        ))
    }
}

#[cfg(not(windows))]
fn acquire_fallback_mutex(timeout: Duration) -> Result<std::sync::MutexGuard<'static, ()>, String> {
    use std::sync::{Mutex, OnceLock, TryLockError};
    use std::thread;
    use std::time::Instant;

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = LOCK.get_or_init(|| Mutex::new(()));
    let started = Instant::now();
    loop {
        match lock.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::WouldBlock) if started.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(TryLockError::WouldBlock) => {
                return Err("Another CSA service operation is still running.".into())
            }
            Err(TryLockError::Poisoned(_)) => {
                return Err("CSA service operation mutex is poisoned".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_lock_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "csa-runtime-lifecycle-{name}-{}-{}.lock",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn parses_and_validates_managed_bridge_identity() {
        let output = format!(
            "starting\n{}{}\n",
            RUNTIME_IDENTITY_PREFIX,
            serde_json::json!({
                "schemaVersion": 1,
                "component": "bridge",
                "runtimeId": "bridge-aabbccdd",
                "version": "0.2.0-restart",
                "buildId": "aaaaaaaaaaaaaaaa",
                "sourcePath": "/home/test/.local/share/csa/runtime/bridge/versions/bridge-aabbccdd/proxy.py",
                "sourceSha256": "a".repeat(64),
                "pid": 1234,
                "capabilities": ["anthropicBridge", "configRevision", "health"],
                "managed": true
            })
        );
        let identity = parse_runtime_identity(&output).unwrap();
        assert_eq!(identity.runtime_id, "bridge-aabbccdd");
        assert!(identity.managed);
    }

    #[test]
    fn rejects_missing_or_unmanaged_bridge_identity() {
        assert!(parse_runtime_identity("start complete").is_err());
        let output = format!(
            "{}{}",
            RUNTIME_IDENTITY_PREFIX,
            serde_json::json!({
                "schemaVersion": 1,
                "component": "bridge",
                "runtimeId": "bridge-aabbccdd",
                "version": "0.2.0-restart",
                "buildId": "aaaaaaaaaaaaaaaa",
                "sourcePath": "/tmp/proxy.py",
                "sourceSha256": "a".repeat(64),
                "pid": 1234,
                "capabilities": ["health"],
                "managed": false
            })
        );
        assert!(parse_runtime_identity(&output).is_err());
    }

    #[test]
    fn reads_runtime_identity_from_bridge_health() {
        let health = serde_json::json!({
            "status": "ok",
            "runtime_identity": {
                "schemaVersion": 1,
                "component": "bridge",
                "runtimeId": "bridge-aabbccdd",
                "version": "0.2.0-restart",
                "buildId": "bbbbbbbbbbbbbbbb",
                "sourcePath": "/managed/proxy.py",
                "sourceSha256": "b".repeat(64),
                "pid": 1234,
                "capabilities": ["health"],
                "managed": true
            }
        });
        assert_eq!(
            runtime_identity_from_health(&health).unwrap().runtime_id,
            "bridge-aabbccdd"
        );
        assert!(runtime_identity_from_health(&serde_json::json!({"status": "ok"})).is_err());
    }

    #[test]
    fn runtime_identity_equality_detects_a_foreign_source() {
        let expected = RuntimeIdentity {
            schema_version: 1,
            component: "bridge".into(),
            runtime_id: "bridge-0.2.0-aabbccdd".into(),
            version: "0.2.0".into(),
            build_id: "aaaaaaaaaaaaaaaa".into(),
            source_path: "/managed/current/proxy.py".into(),
            source_sha256: "a".repeat(64),
            pid: 1234,
            capabilities: vec!["health".into()],
            managed: true,
        };
        let mut actual = expected.clone();
        actual.source_path = "/old-package/proxy.py".into();

        assert_ne!(actual, expected);
    }

    #[test]
    fn service_operation_lock_serializes_process_operations() {
        let path = temporary_lock_path("serialize");
        let first = ServiceOperationLock::acquire(&path, "first", Duration::from_secs(1))
            .expect("first owner should acquire the lock");
        let contender_path = path.clone();
        let second_error = std::thread::spawn(move || {
            match ServiceOperationLock::acquire(
                &contender_path,
                "second",
                Duration::from_millis(100),
            ) {
                Ok(_) => None,
                Err(error) => Some(error),
            }
        })
        .join()
        .unwrap()
        .expect("a concurrent owner must not acquire the lock");
        assert!(second_error.contains("Another CSA service operation"));
        drop(first);
        ServiceOperationLock::acquire(&path, "third", Duration::from_secs(1))
            .expect("lock should be released when the owner exits");
        let _ = fs::remove_file(path);
    }
}
