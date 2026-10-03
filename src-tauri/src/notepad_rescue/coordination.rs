use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::AppError;

pub const AGENT_MUTEX_NAME: &str = "Local\\ApiDesk_NotepadRescue_Agent_Instance";
pub const GC_MUTEX_NAME: &str = "Local\\ApiDesk_NotepadRescue_GcLock";
pub const AGENT_STATUS_FILE: &str = "agent_status.json";
pub const AGENT_STOP_FLAG: &str = "agent_stop.flag";
pub const AGENT_VERSION: &str = "1.0.0";
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub pid: u32,
    pub started_at: String,
    pub last_heartbeat: String,
    pub status: String,
    pub version: String,
    pub watcher_active: bool,
    pub captured_count: usize,
    pub last_captured_at: Option<String>,
    pub provider: String,
}

impl Default for AgentStatus {
    fn default() -> Self {
        Self {
            pid: std::process::id(),
            started_at: now_rfc3339(),
            last_heartbeat: now_rfc3339(),
            status: "RUNNING".to_string(),
            version: AGENT_VERSION.to_string(),
            watcher_active: false,
            captured_count: 0,
            last_captured_at: None,
            provider: "background_agent".to_string(),
        }
    }
}

pub fn now_rfc3339() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let base = crate::usage::epoch_to_rfc3339(secs);
    if let Some(stripped) = base.strip_suffix('Z') {
        format!("{stripped}.{millis:03}Z")
    } else {
        base
    }
}

// =========================================================================
// Windows Named Mutex / Cross-Process Lock
// =========================================================================

#[cfg(windows)]
#[allow(dead_code)]
mod win32 {
    use std::ffi::c_void;

    extern "system" {
        pub fn CreateMutexW(
            lpMutexAttributes: *mut c_void,
            bInitialOwner: i32,
            lpName: *const u16,
        ) -> *mut c_void;
        pub fn WaitForSingleObject(hHandle: *mut c_void, dwMilliseconds: u32) -> u32;
        pub fn ReleaseMutex(hMutex: *mut c_void) -> i32;
        pub fn CloseHandle(hObject: *mut c_void) -> i32;
        pub fn GetLastError() -> u32;
    }

    pub const WAIT_OBJECT_0: u32 = 0x0000_0000;
    pub const WAIT_ABANDONED: u32 = 0x0000_0080;
    pub const WAIT_TIMEOUT: u32 = 0x0000_0102;
    pub const ERROR_ALREADY_EXISTS: u32 = 183;
    pub const INFINITE: u32 = 0xFFFF_FFFF;
}

pub struct NamedMutexGuard {
    #[cfg(windows)]
    handle: *mut std::ffi::c_void,
    _lock_file: Option<std::fs::File>,
}

unsafe impl Send for NamedMutexGuard {}
unsafe impl Sync for NamedMutexGuard {}

impl Drop for NamedMutexGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        if !self.handle.is_null() {
            unsafe {
                win32::ReleaseMutex(self.handle);
                win32::CloseHandle(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

pub struct AgentInstanceLock {
    #[cfg(windows)]
    handle: *mut std::ffi::c_void,
    _lock_path: Option<PathBuf>,
}

unsafe impl Send for AgentInstanceLock {}
unsafe impl Sync for AgentInstanceLock {}

impl AgentInstanceLock {
    /// 단일 인스턴스 락 획득 시도. 이미 다른 인스턴스가 실행 중이면 None을 반환.
    pub fn try_acquire(_storage_root: &Path) -> Option<Self> {
        #[cfg(windows)]
        {
            let wide_name: Vec<u16> = AGENT_MUTEX_NAME
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let handle =
                unsafe { win32::CreateMutexW(std::ptr::null_mut(), 1, wide_name.as_ptr()) };
            if handle.is_null() {
                return None;
            }
            let err = unsafe { win32::GetLastError() };
            if err == win32::ERROR_ALREADY_EXISTS {
                unsafe { win32::CloseHandle(handle) };
                return None;
            }
            Some(Self {
                handle,
                _lock_path: None,
            })
        }
        #[cfg(not(windows))]
        {
            let lock_path = _storage_root.join("agent.instance.lock");
            if let Ok(file) = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&lock_path)
            {
                Some(Self {
                    _lock_path: Some(lock_path),
                })
            } else {
                None
            }
        }
    }
}

impl Drop for AgentInstanceLock {
    fn drop(&mut self) {
        #[cfg(windows)]
        if !self.handle.is_null() {
            unsafe {
                win32::ReleaseMutex(self.handle);
                win32::CloseHandle(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
        #[cfg(not(windows))]
        if let Some(ref path) = self._lock_path {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 프로세스 간 상호 배제 GC/CAS 락
pub struct CrossProcessGcLock;

impl CrossProcessGcLock {
    pub fn acquire(storage_root: &Path, timeout: Duration) -> Result<NamedMutexGuard, AppError> {
        let _ = storage_root;
        let _ = std::fs::create_dir_all(storage_root);

        #[cfg(windows)]
        {
            let wide_name: Vec<u16> = GC_MUTEX_NAME
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let handle =
                unsafe { win32::CreateMutexW(std::ptr::null_mut(), 0, wide_name.as_ptr()) };
            if handle.is_null() {
                return Err(AppError::InvalidRequest(
                    "GC 프로세스 락 생성 실패".to_string(),
                ));
            }

            let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
            let wait_res = unsafe { win32::WaitForSingleObject(handle, ms) };
            if wait_res == win32::WAIT_OBJECT_0 || wait_res == win32::WAIT_ABANDONED {
                Ok(NamedMutexGuard {
                    handle,
                    _lock_file: None,
                })
            } else {
                unsafe { win32::CloseHandle(handle) };
                Err(AppError::InvalidRequest(
                    "GC 프로세스 락 획득 타임아웃".to_string(),
                ))
            }
        }
        #[cfg(not(windows))]
        {
            let start = Instant::now();
            loop {
                match std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&lock_file_path)
                {
                    Ok(file) => {
                        return Ok(NamedMutexGuard {
                            _lock_file: Some(file),
                        });
                    }
                    Err(_) => {
                        if start.elapsed() >= timeout {
                            return Err(AppError::InvalidRequest(
                                "GC 프로세스 락 획득 타임아웃".to_string(),
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        }
    }
}

// =========================================================================
// Heartbeat & IPC Status Management
// =========================================================================

pub fn status_file_path(storage_root: &Path) -> PathBuf {
    storage_root.join(AGENT_STATUS_FILE)
}

pub fn stop_flag_path(storage_root: &Path) -> PathBuf {
    storage_root.join(AGENT_STOP_FLAG)
}

pub fn write_heartbeat(storage_root: &Path, status: &AgentStatus) -> Result<(), AppError> {
    let _ = std::fs::create_dir_all(storage_root);
    let path = status_file_path(storage_root);
    let tmp_path = storage_root.join(format!("{AGENT_STATUS_FILE}.tmp"));

    let json = serde_json::to_string_pretty(status)
        .map_err(|e| AppError::InvalidRequest(format!("상태 직렬화 실패: {e}")))?;

    std::fs::write(&tmp_path, json.as_bytes())
        .map_err(|e| AppError::Io(format!("상태 임시 파일 작성 실패: {e}")))?;

    std::fs::rename(&tmp_path, &path)
        .map_err(|e| AppError::Io(format!("상태 파일 원자적 교체 실패: {e}")))?;

    Ok(())
}

pub fn read_heartbeat(storage_root: &Path) -> Result<Option<AgentStatus>, AppError> {
    let path = status_file_path(storage_root);
    if !path.is_file() {
        return Ok(None);
    }

    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };

    if bytes.is_empty() {
        return Ok(None);
    }

    match serde_json::from_slice::<AgentStatus>(&bytes) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Ok(None),
    }
}

pub fn clear_heartbeat(storage_root: &Path) {
    let path = status_file_path(storage_root);
    let _ = std::fs::remove_file(path);
}

/// 에이전트 프로세스가 현재 살아있고 하트비트가 최신인지 검증
pub fn is_agent_alive(storage_root: &Path) -> bool {
    let status = match read_heartbeat(storage_root) {
        Ok(Some(s)) => s,
        _ => return false,
    };

    if status.status != "RUNNING" {
        return false;
    }

    // 하트비트 시간 확인
    if let Ok(hb_time) = chrono_parse_rfc3339(&status.last_heartbeat) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now.saturating_sub(hb_time) > HEARTBEAT_TIMEOUT.as_secs() {
            return false;
        }
    }

    // Windows에서 PID 실제 생존 여부 확인
    #[cfg(windows)]
    {
        extern "system" {
            fn OpenProcess(
                dwDesiredAccess: u32,
                bInheritHandle: i32,
                dwProcessId: u32,
            ) -> *mut std::ffi::c_void;
            fn CloseHandle(hObject: *mut std::ffi::c_void) -> i32;
        }
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, status.pid) };
        if handle.is_null() {
            return false;
        }
        unsafe { CloseHandle(handle) };
    }

    true
}

fn chrono_parse_rfc3339(s: &str) -> Result<u64, ()> {
    // 간단한 ISO 8601 / RFC 3339 파싱
    let parts: Vec<&str> = s.split('T').collect();
    if parts.len() != 2 {
        return Err(());
    }
    let date_parts: Vec<u32> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    let time_clean = parts[1].trim_end_matches('Z');
    let time_parts: Vec<&str> = time_clean.split('.').collect();
    let hms: Vec<u32> = time_parts[0]
        .split(':')
        .filter_map(|p| p.parse().ok())
        .collect();

    if date_parts.len() != 3 || hms.len() != 3 {
        return Err(());
    }

    let y = date_parts[0] as i64;
    let m = date_parts[1] as i64;
    let d = date_parts[2] as i64;
    let hour = hms[0] as u64;
    let min = hms[1] as u64;
    let sec = hms[2] as u64;

    // 대략적인 epoch 초 계산
    let mut days = 0i64;
    for year in 1970..y {
        let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        days += if leap { 366 } else { 365 };
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    for &md in &month_days[..((m - 1) as usize)] {
        days += md as i64;
    }
    days += d - 1;

    let total_secs = (days as u64) * 86400 + hour * 3600 + min * 60 + sec;
    Ok(total_secs)
}

pub fn request_agent_stop(storage_root: &Path) -> Result<(), AppError> {
    let path = stop_flag_path(storage_root);
    std::fs::write(&path, b"STOP")
        .map_err(|e| AppError::Io(format!("정지 플래그 생성 실패: {e}")))?;
    Ok(())
}

pub fn has_stop_request(storage_root: &Path) -> bool {
    stop_flag_path(storage_root).is_file()
}

pub fn clear_stop_request(storage_root: &Path) {
    let path = stop_flag_path(storage_root);
    let _ = std::fs::remove_file(path);
}

// =========================================================================
// Windows Startup Registration (Per-User, No Elevation)
// =========================================================================

pub const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE_NAME: &str = "NotepadRescueAgent";

pub fn is_startup_enabled() -> bool {
    #[cfg(windows)]
    {
        let output = crate::process::hidden_command("reg")
            .args(["query", RUN_KEY, "/v", RUN_VALUE_NAME])
            .output();
        if let Ok(out) = output {
            out.status.success()
        } else {
            false
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn set_startup_enabled(enabled: bool, agent_exe_path: Option<&Path>) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        if enabled {
            let exe = match agent_exe_path {
                Some(p) => p.to_path_buf(),
                None => default_agent_executable_path()?,
            };
            let cmd_str = format!("\"{}\" --background", exe.display());

            let output = crate::process::hidden_command("reg")
                .args([
                    "add",
                    RUN_KEY,
                    "/v",
                    RUN_VALUE_NAME,
                    "/t",
                    "REG_SZ",
                    "/d",
                    &cmd_str,
                    "/f",
                ])
                .output()
                .map_err(|e| AppError::Io(format!("시작프로그램 등록 실패: {e}")))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(AppError::InvalidRequest(format!(
                    "시작프로그램 등록 실패: {stderr}"
                )));
            }
        } else {
            let output = crate::process::hidden_command("reg")
                .args(["delete", RUN_KEY, "/v", RUN_VALUE_NAME, "/f"])
                .output()
                .map_err(|e| AppError::Io(format!("시작프로그램 제거 실패: {e}")))?;

            // 키가 없어서 실패한 경우(status != 0)도 정상 해제로 간주
            let _ = output;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        let _ = agent_exe_path;
        Ok(())
    }
}

pub fn default_agent_executable_path() -> Result<PathBuf, AppError> {
    let current_exe = std::env::current_exe()
        .map_err(|e| AppError::Io(format!("현재 실행 파일 경로를 확인할 수 없습니다: {e}")))?;
    let parent = current_exe
        .parent()
        .ok_or_else(|| AppError::Io("실행 파일 부모 디렉터리가 없습니다".to_string()))?;

    let agent_name = if cfg!(windows) {
        "notepad-rescue-agent.exe"
    } else {
        "notepad-rescue-agent"
    };

    let target = parent.join(agent_name);
    Ok(target)
}
