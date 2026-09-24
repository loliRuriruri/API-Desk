use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::AppError;
use crate::vault::{self, VaultState};

const LOG_LIMIT: usize = 400;
const START_PORT_WAIT_MS: u64 = 2_000;

fn vault_unlocked(vault: &VaultState) -> bool {
    vault.with_stronghold(|_| Ok(())).is_ok()
}

fn fresh_config(id: &str) -> ServiceConfig {
    ServiceConfig {
        id: id.to_string(),
        keep_alive_on_exit: true,
        ..Default::default()
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ServiceEnvVar {
    pub key: String,
    pub value: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LocalServiceDefinition {
    pub id: String,
    pub label: String,
    pub description: String,
    pub default_endpoint: String,
    pub default_port: u16,
    pub default_device: String,
    pub device_env: String,
    pub static_env: Vec<ServiceEnvVar>,
    pub secret_env: String,
    pub secret_id: String,
    pub api_key_label: String,
    pub default_health_path: String,
    pub supports_login_startup: bool,
}

fn env_var(key: &str, value: &str) -> ServiceEnvVar {
    ServiceEnvVar {
        key: key.to_string(),
        value: value.to_string(),
    }
}

pub fn definitions() -> Vec<LocalServiceDefinition> {
    vec![LocalServiceDefinition {
        id: "laya".into(),
        label: "Laya".into(),
        description: "Laya 로컬 AI 서비스 (FastAPI 계열 추정) · API Desk에서 실행/중지/상태 확인".into(),
        default_endpoint: "http://127.0.0.1:8000".into(),
        default_port: 8000,
        default_device: "cuda".into(),
        device_env: "LAYA_DEVICE".into(),
        static_env: vec![env_var("USE_TF", "0"), env_var("LAYA_PRELOAD", "1")],
        secret_env: "LAYA_API_KEY".into(),
        secret_id: "local:laya:api_key".into(),
        api_key_label: "LAYA_API_KEY".into(),
        default_health_path: "/health".into(),
        supports_login_startup: false,
    }]
}

fn definition(id: &str) -> Option<LocalServiceDefinition> {
    definitions().into_iter().find(|item| item.id == id)
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfig {
    pub id: String,
    #[serde(default)]
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub workdir: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub health_path: String,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default = "default_true")]
    pub keep_alive_on_exit: bool,
    /// API 키를 저장한 적이 있는지(비밀 값 아님). Vault가 잠겨 있어도 알 수 있어야 한다.
    #[serde(default)]
    pub api_key_configured: bool,
}

fn default_true() -> bool {
    true
}

impl ServiceConfig {
    fn merged(self, def: &LocalServiceDefinition) -> Self {
        Self {
            id: def.id.clone(),
            executable: self.executable.trim().to_string(),
            args: self
                .args
                .into_iter()
                .map(|arg| arg.trim().to_string())
                .filter(|arg| !arg.is_empty())
                .collect(),
            workdir: self.workdir.trim().to_string(),
            endpoint: if self.endpoint.trim().is_empty() {
                def.default_endpoint.clone()
            } else {
                self.endpoint.trim().trim_end_matches('/').to_string()
            },
            port: if self.port == 0 {
                def.default_port
            } else {
                self.port
            },
            device: if self.device.trim().is_empty() {
                def.default_device.clone()
            } else {
                self.device.trim().to_string()
            },
            health_path: if self.health_path.trim().is_empty() {
                def.default_health_path.clone()
            } else {
                let trimmed = self.health_path.trim();
                if trimmed.starts_with('/') {
                    trimmed.to_string()
                } else {
                    format!("/{trimmed}")
                }
            },
            auto_start: self.auto_start,
            keep_alive_on_exit: self.keep_alive_on_exit,
            api_key_configured: self.api_key_configured,
        }
    }
}

fn config_path(app: &AppHandle) -> Result<PathBuf, AppError> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Io(e.to_string()))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("local-services.json"))
}

fn read_configs(app: &AppHandle) -> Vec<ServiceConfig> {
    let Ok(path) = config_path(app) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn write_configs(app: &AppHandle, configs: &[ServiceConfig]) -> Result<(), AppError> {
    let path = config_path(app)?;
    let text = serde_json::to_string_pretty(configs).map_err(|e| AppError::Io(e.to_string()))?;
    std::fs::write(path, text)?;
    Ok(())
}

/// 상태 변경을 다른 창(메인/미니)에 알린다. 실패해도 기능에는 영향이 없다.
fn emit_status_changed(app: &AppHandle) {
    let _ = app.emit("local-service-status-changed", ());
}

fn set_api_key_configured(app: &AppHandle, id: &str, configured: bool) -> Result<(), AppError> {
    let mut stored = read_configs(app);
    let Some(def) = definition(id) else {
        return Ok(());
    };
    if let Some(entry) = stored.iter_mut().find(|config| config.id == id) {
        entry.api_key_configured = configured;
    } else {
        let mut fresh = fresh_config(id);
        fresh.api_key_configured = configured;
        stored.push(fresh.merged(&def));
    }
    write_configs(app, &stored)
}

fn merged_config(app: &AppHandle, id: &str) -> Result<(LocalServiceDefinition, ServiceConfig), AppError> {
    let def = definition(id)
        .ok_or_else(|| AppError::InvalidRequest(format!("알 수 없는 로컬 서비스입니다: {id}")))?;
    let stored = read_configs(app)
        .into_iter()
        .find(|config| config.id == id)
        .unwrap_or_else(|| fresh_config(&id));
    let merged = stored.merged(&def);
    Ok((def, merged))
}

#[derive(Default)]
pub struct LocalServicesState {
    processes: Mutex<HashMap<String, ManagedProcess>>,
    logs: Mutex<HashMap<String, VecDeque<String>>>,
}

impl LocalServicesState {
    pub fn new() -> Self {
        Self::default()
    }

    fn push_log(&self, id: &str, line: String) {
        let mut logs = self.logs.lock().unwrap();
        let buffer = logs.entry(id.to_string()).or_default();
        buffer.push_back(line);
        while buffer.len() > LOG_LIMIT {
            buffer.pop_front();
        }
    }

    fn logs_of(&self, id: &str, limit: usize) -> Vec<String> {
        let logs = self.logs.lock().unwrap();
        let Some(buffer) = logs.get(id) else {
            return Vec::new();
        };
        let take = limit.min(buffer.len());
        buffer.iter().skip(buffer.len() - take).cloned().collect()
    }

    fn log_count(&self, id: &str) -> usize {
        self.logs
            .lock()
            .unwrap()
            .get(id)
            .map(|buffer| buffer.len())
            .unwrap_or(0)
    }


}

struct ManagedProcess {
    child: Child,
    pid: u32,
    started_unix: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalServiceStatus {
    pub id: String,
    pub label: String,
    pub description: String,
    pub state: String,
    pub endpoint: String,
    pub port: u16,
    pub device: String,
    pub pid: Option<u32>,
    pub managed: bool,
    pub uptime_secs: Option<i64>,
    pub api_key_set: bool,
    pub api_key_hint: Option<String>,
    pub api_key_configured: bool,
    pub vault_locked: bool,
    pub executable: String,
    pub executable_set: bool,
    pub args: Vec<String>,
    pub workdir: String,
    pub health_path: String,
    pub auto_start: bool,
    pub keep_alive_on_exit: bool,
    pub log_lines: usize,
    pub last_error: Option<String>,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(windows)]
fn process_start_unix(pid: u32) -> Option<i64> {
    use std::ffi::c_void;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetProcessTimes(
            handle: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut creation = FileTime { low: 0, high: 0 };
        let mut exit = FileTime { low: 0, high: 0 };
        let mut kernel = FileTime { low: 0, high: 0 };
        let mut user = FileTime { low: 0, high: 0 };
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let ticks = ((creation.high as u64) << 32) | creation.low as u64;
        // FILETIME = 100ns ticks since 1601-01-01
        let unix_100ns = ticks as i128 - 116_444_736_000_000_000i128;
        Some((unix_100ns / 10_000_000) as i64)
    }
}

#[cfg(not(windows))]
fn process_start_unix(_pid: u32) -> Option<i64> {
    None
}

pub fn parse_port_owner(netstat_output: &str, port: u16) -> Option<u32> {
    let suffix = format!(":{port}");
    for line in netstat_output.lines() {
        let columns: Vec<&str> = line.split_whitespace().collect();
        if columns.len() < 5 {
            continue;
        }
        if !columns[0].eq_ignore_ascii_case("TCP") {
            continue;
        }
        if !columns[1].ends_with(&suffix) {
            continue;
        }
        if !columns[3].eq_ignore_ascii_case("LISTENING") {
            continue;
        }
        if let Ok(pid) = columns[4].parse::<u32>() {
            return Some(pid);
        }
    }
    None
}

fn port_owner_pid(port: u16) -> Option<u32> {
    let output = Command::new("netstat")
        .args(["-ano", "-p", "tcp"])
        .creation_flags_no_window()
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    parse_port_owner(&text, port)
}

fn tcp_alive(port: u16) -> bool {
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, Duration::from_millis(350)).is_ok()
}

pub fn scrub_secrets(line: &str, secrets: &[String]) -> String {
    let mut result = line.to_string();
    for secret in secrets {
        if secret.len() >= 4 && result.contains(secret.as_str()) {
            result = result.replace(secret.as_str(), "***");
        }
    }
    result
}

pub fn service_env(
    def: &LocalServiceDefinition,
    config: &ServiceConfig,
    secret: Option<&str>,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = def
        .static_env
        .iter()
        .map(|item| (item.key.clone(), item.value.clone()))
        .collect();
    if !def.device_env.is_empty() && !config.device.is_empty() {
        env.push((def.device_env.clone(), config.device.clone()));
    }
    if let Some(secret) = secret {
        if !def.secret_env.is_empty() {
            env.push((def.secret_env.clone(), secret.to_string()));
        }
    }
    env
}

fn api_key_hint(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 4 {
        return "••••".to_string();
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("••••{tail}")
}

/// Start를 막아야 하는 사유를 계산한다(순수 함수).
/// - 키를 저장해 둔 서비스인데 Vault가 잠겨 있으면 키를 주입할 수 없으므로 차단한다.
/// - 키를 저장한 적이 없으면(무인증 모드) Vault가 잠겨 있어도 실행할 수 있다.
fn start_block_reason(
    config: &ServiceConfig,
    vault_locked: bool,
    key_present: bool,
) -> Option<String> {
    if config.api_key_configured {
        if vault_locked {
            return Some("Vault 잠금 해제 필요 — 저장된 API 키를 주입할 수 없습니다".into());
        }
        if !key_present {
            return Some(
                "저장된 API 키를 Vault에서 찾을 수 없습니다. 키를 다시 저장하거나 삭제하세요".into(),
            );
        }
    }
    None
}

fn spawn_log_reader<R: std::io::Read + Send + 'static>(
    app: AppHandle,
    id: String,
    reader: R,
    secrets: Vec<String>,
) {
    std::thread::spawn(move || {
        let buffered = BufReader::new(reader);
        for line in buffered.lines() {
            let Ok(line) = line else { break };
            let cleaned = scrub_secrets(&line, &secrets);
            if let Some(state) = app.try_state::<LocalServicesState>() {
                state.push_log(&id, cleaned);
            }
        }
    });
}

trait NoWindow {
    fn creation_flags_no_window(&mut self) -> &mut Self;
}

impl NoWindow for Command {
    #[cfg(windows)]
    fn creation_flags_no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(0x0800_0000)
    }

    #[cfg(not(windows))]
    fn creation_flags_no_window(&mut self) -> &mut Self {
        self
    }
}

fn kill_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags_no_window()
        .output();
}

fn service_state_from(
    managed: Option<(u32, i64, Option<i32>)>,
    owner_pid: Option<u32>,
    port_open: bool,
) -> (&'static str, Option<u32>, bool, Option<i64>) {
    match managed {
        Some((_pid, _started_unix, Some(code))) => {
            if code == 0 {
                ("stopped", None, true, None)
            } else {
                ("error", None, true, None)
            }
        }
        Some((pid, started_unix, None)) => {
            let uptime = unix_now() - started_unix;
            if port_open {
                ("running", Some(pid), true, Some(uptime.max(0)))
            } else {
                ("starting", Some(pid), true, Some(uptime.max(0)))
            }
        }
        None => match owner_pid {
            Some(pid) => {
                let uptime = process_start_unix(pid).map(|start| (unix_now() - start).max(0));
                ("running", Some(pid), false, uptime)
            }
            None => ("stopped", None, false, None),
        },
    }
}

fn build_status(
    app: &AppHandle,
    state: &LocalServicesState,
    vault: &VaultState,
) -> Vec<LocalServiceStatus> {
    let mut result = Vec::new();
    for def in definitions() {
        let Ok((def, config)) = merged_config(app, &def.id) else {
            continue;
        };
        let vault_locked = !vault_unlocked(vault);
        let secret = if vault_locked {
            None
        } else {
            vault::read_secret_string(vault, &def.secret_id)
                .ok()
                .map(|value| value.to_string())
        };
        let mut exited = false;
        let managed_snapshot = {
            let mut processes = state.processes.lock().unwrap();
            match processes.get_mut(&def.id) {
                Some(entry) => {
                    let exit = entry.child.try_wait().ok().flatten();
                    if exit.is_some() {
                        let pid = entry.pid;
                        let started = entry.started_unix;
                        let code = exit.map(|status| status.code().unwrap_or(-1));
                        processes.remove(&def.id);
                        exited = true;
                        Some((pid, started, code))
                    } else {
                        Some((entry.pid, entry.started_unix, None))
                    }
                }
                None => None,
            }
        };
        let owner_pid = port_owner_pid(config.port);
        let port_open = owner_pid.is_some() || tcp_alive(config.port);
        let (service_state, pid, managed, uptime_secs) =
            service_state_from(managed_snapshot, owner_pid, port_open);
        let last_error = match service_state {
            "error" => Some(format!(
                "프로세스가 비정상 종료되었습니다 (PID {}). 최근 로그를 확인하세요",
                pid.unwrap_or(0)
            )),
            _ => None,
        };
        if exited {
            emit_status_changed(app);
        }
        result.push(LocalServiceStatus {
            id: def.id.clone(),
            label: def.label.clone(),
            description: def.description.clone(),
            state: service_state.to_string(),
            endpoint: config.endpoint.clone(),
            port: config.port,
            device: config.device.clone(),
            pid,
            managed,
            uptime_secs,
            api_key_set: secret.is_some(),
            api_key_hint: secret.as_deref().map(api_key_hint),
            api_key_configured: config.api_key_configured,
            vault_locked,
            executable: config.executable.clone(),
            executable_set: !config.executable.is_empty(),
            args: config.args.clone(),
            workdir: config.workdir.clone(),
            health_path: config.health_path.clone(),
            auto_start: config.auto_start,
            keep_alive_on_exit: config.keep_alive_on_exit,
            log_lines: state.log_count(&def.id),
            last_error,
        });
    }
    result
}

#[tauri::command]
pub fn local_service_definitions() -> Vec<LocalServiceDefinition> {
    definitions()
}

#[tauri::command]
pub fn local_service_configs(app: AppHandle) -> Vec<ServiceConfig> {
    let stored = read_configs(&app);
    definitions()
        .into_iter()
        .map(|def| {
            stored
                .iter()
                .find(|config| config.id == def.id)
                .cloned()
                .unwrap_or_else(|| fresh_config(&def.id))
                .merged(&def)
        })
        .collect()
}

#[tauri::command]
pub fn local_service_save_config(
    app: AppHandle,
    config: ServiceConfig,
) -> Result<Vec<ServiceConfig>, AppError> {
    let def = definition(&config.id)
        .ok_or_else(|| AppError::InvalidRequest(format!("알 수 없는 로컬 서비스입니다: {}", config.id)))?;
    let merged = config.merged(&def);
    let mut stored = read_configs(&app);
    stored.retain(|item| item.id != merged.id);
    stored.push(merged.clone());
    write_configs(&app, &stored)?;
    emit_status_changed(&app);
    Ok(local_service_configs(app))
}

#[tauri::command]
pub fn local_service_status(
    app: AppHandle,
    state: State<'_, LocalServicesState>,
    vault: State<'_, VaultState>,
) -> Vec<LocalServiceStatus> {
    build_status(&app, &state, &vault)
}

#[tauri::command]
pub fn local_service_logs(
    state: State<'_, LocalServicesState>,
    id: String,
    limit: Option<usize>,
) -> Vec<String> {
    state.logs_of(&id, limit.unwrap_or(120))
}

fn start_service(
    app: &AppHandle,
    state: &LocalServicesState,
    vault: &VaultState,
    id: &str,
) -> Result<LocalServiceStatus, AppError> {
    let (def, config) = merged_config(app, id)?;
    if config.executable.is_empty() {
        return Err(AppError::InvalidRequest(
            "실행 파일 경로가 설정되지 않았습니다. 설정에서 Laya 실행 파일 또는 venv 파이썬 경로를 지정하세요"
                .into(),
        ));
    }
    {
        let mut processes = state.processes.lock().unwrap();
        if let Some(entry) = processes.get_mut(id) {
            match entry.child.try_wait() {
                Ok(None) => {
                    return Err(AppError::InvalidRequest(
                        "이미 API Desk가 실행 중인 서비스입니다 (중복 실행 방지)".into(),
                    ))
                }
                Ok(Some(_)) | Err(_) => {
                    processes.remove(id);
                }
            }
        }
    }
    if let Some(pid) = port_owner_pid(config.port) {
        return Err(AppError::InvalidRequest(format!(
            "포트 {}이(가) 이미 사용 중입니다 (PID {}). 기존 프로세스를 중지한 뒤 다시 시도하세요",
            config.port, pid
        )));
    }
    // LAYA_API_KEY는 선택 사항이다. Vault에 키가 있으면 Bearer 인증용으로 주입하고,
    // 없으면 주입하지 않은 채 localhost 전용 무인증 모드로 실행한다.
    let vault_locked = !vault_unlocked(vault);
    let secret = if vault_locked {
        None
    } else {
        vault::read_secret_string(vault, &def.secret_id)
            .ok()
            .map(|value| value.to_string())
    };
    if let Some(reason) = start_block_reason(&config, vault_locked, secret.is_some()) {
        return Err(AppError::InvalidRequest(reason));
    }

    let mut command = Command::new(&config.executable);
    command
        .args(&config.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !config.workdir.is_empty() {
        command.current_dir(&config.workdir);
    }
    for (key, value) in service_env(&def, &config, secret.as_deref()) {
        command.env(key, value);
    }
    command.creation_flags_no_window();

    state.push_log(
        &id,
        format!(
            "[api-desk] {} 시작 ({}): {} {}",
            def.label,
            if secret.is_some() {
                format!("{} 주입 · Bearer 인증", def.secret_env)
            } else {
                format!("{} 미설정 · localhost 전용 무인증", def.secret_env)
            },
            config.executable,
            config.args.join(" ")
        ),
    );

    let mut child = command
        .spawn()
        .map_err(|e| AppError::Io(format!("서비스 실행에 실패했습니다: {e}")))?;
    let pid = child.id();
    if let Some(stdout) = child.stdout.take() {
        spawn_log_reader(app.clone(), id.to_string(), stdout, secret.clone().into_iter().collect());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_log_reader(app.clone(), id.to_string(), stderr, secret.clone().into_iter().collect());
    }

    let started_unix = unix_now();
    let mut processes = state.processes.lock().unwrap();
    processes.insert(
        id.to_string(),
        ManagedProcess {
            child,
            pid,
            started_unix,
        },
    );
    drop(processes);

    let wait_until = Instant::now() + Duration::from_millis(START_PORT_WAIT_MS);
    while Instant::now() < wait_until {
        if tcp_alive(config.port) {
            break;
        }
        std::thread::sleep(Duration::from_millis(120));
    }

    let statuses = build_status(&app, &state, &vault);
    statuses
        .into_iter()
        .find(|status| status.id == id)
        .ok_or_else(|| AppError::Io("서비스 상태를 읽지 못했습니다".into()))
}

fn stop_service(
    app: &AppHandle,
    state: &LocalServicesState,
    vault: &VaultState,
    id: &str,
) -> Result<LocalServiceStatus, AppError> {
    let (_, config) = merged_config(app, id)?;
    let mut killed_pid: Option<u32> = None;
    {
        let mut processes = state.processes.lock().unwrap();
        if let Some(entry) = processes.remove(id) {
            killed_pid = Some(entry.pid);
            kill_tree(entry.pid);
        }
    }
    if killed_pid.is_none() {
        if let Some(pid) = port_owner_pid(config.port) {
            kill_tree(pid);
            killed_pid = Some(pid);
        }
    }
    if let Some(pid) = killed_pid {
        state.push_log(id, format!("[api-desk] 서비스 중지 (PID {pid})"));
        std::thread::sleep(Duration::from_millis(600));
    }
    let statuses = build_status(&app, &state, &vault);
    statuses
        .into_iter()
        .find(|status| status.id == id)
        .ok_or_else(|| AppError::Io("서비스 상태를 읽지 못했습니다".into()))
}

fn restart_service(
    app: &AppHandle,
    state: &LocalServicesState,
    vault: &VaultState,
    id: &str,
) -> Result<LocalServiceStatus, AppError> {
    let (_, config) = merged_config(app, id)?;
    {
        let mut processes = state.processes.lock().unwrap();
        if let Some(entry) = processes.remove(id) {
            kill_tree(entry.pid);
        }
    }
    if let Some(pid) = port_owner_pid(config.port) {
        kill_tree(pid);
    }
    std::thread::sleep(Duration::from_millis(700));
    start_service(app, state, vault, id)
}

#[tauri::command]
pub fn local_service_start(
    app: AppHandle,
    state: State<'_, LocalServicesState>,
    vault: State<'_, VaultState>,
    id: String,
) -> Result<LocalServiceStatus, AppError> {
    let status = start_service(&app, &state, &vault, &id)?;
    emit_status_changed(&app);
    Ok(status)
}

#[tauri::command]
pub fn local_service_stop(
    app: AppHandle,
    state: State<'_, LocalServicesState>,
    vault: State<'_, VaultState>,
    id: String,
) -> Result<LocalServiceStatus, AppError> {
    let status = stop_service(&app, &state, &vault, &id)?;
    emit_status_changed(&app);
    Ok(status)
}

#[tauri::command]
pub fn local_service_restart(
    app: AppHandle,
    state: State<'_, LocalServicesState>,
    vault: State<'_, VaultState>,
    id: String,
) -> Result<LocalServiceStatus, AppError> {
    let status = restart_service(&app, &state, &vault, &id)?;
    emit_status_changed(&app);
    Ok(status)
}

#[tauri::command]
pub async fn local_service_health(
    app: AppHandle,
    vault: State<'_, VaultState>,
    id: String,
) -> Result<Value, AppError> {
    let (def, config) = merged_config(&app, &id)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(4_000))
        .build()
        .map_err(|e| AppError::Network(e.to_string()))?;
    let url = format!("{}{}", config.endpoint, config.health_path);
    let secret = if vault_unlocked(&vault) {
        vault::read_secret_string(&vault, &def.secret_id)
            .ok()
            .map(|value| value.to_string())
    } else {
        None
    };
    let mut request = client.get(&url);
    if let Some(secret) = secret.as_deref() {
        request = request.header("Authorization", format!("Bearer {secret}"));
    }
    let started = Instant::now();
    match request.send().await {
        Ok(response) => {
            let status = response.status();
            let latency = started.elapsed().as_millis() as u64;
            let body = response.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(160).collect();
            Ok(json!({
                "ok": status.is_success(),
                "status": status.as_u16(),
                "latencyMs": latency,
                "url": url,
                "detail": if snippet.trim().is_empty() { Value::Null } else { json!(snippet) },
                "label": def.label,
            }))
        }
        Err(error) => Ok(json!({
            "ok": false,
            "status": 0,
            "latencyMs": started.elapsed().as_millis() as u64,
            "url": url,
            "detail": scrub_secrets(&error.to_string(), &[]),
            "label": def.label,
        })),
    }
}

#[tauri::command]
pub fn local_service_set_api_key(
    app: AppHandle,
    vault: State<'_, VaultState>,
    id: String,
    value: String,
) -> Result<(), AppError> {
    let def = definition(&id)
        .ok_or_else(|| AppError::InvalidRequest(format!("알 수 없는 로컬 서비스입니다: {id}")))?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidRequest("API 키가 비어 있습니다".into()));
    }
    vault::write_secret(&vault, &def.secret_id, trimmed)?;
    let _ = set_api_key_configured(&app, &def.id, true);
    emit_status_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn local_service_clear_api_key(
    app: AppHandle,
    vault: State<'_, VaultState>,
    id: String,
) -> Result<(), AppError> {
    let def = definition(&id)
        .ok_or_else(|| AppError::InvalidRequest(format!("알 수 없는 로컬 서비스입니다: {id}")))?;
    vault::delete_secret(&vault, &def.secret_id)?;
    let _ = set_api_key_configured(&app, &def.id, false);
    emit_status_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn local_service_autostart(
    app: AppHandle,
    state: State<'_, LocalServicesState>,
    vault: State<'_, VaultState>,
) -> Vec<String> {
    let mut started = Vec::new();
    let statuses = build_status(&app, &state, &vault);
    for status in statuses {
        if !status.auto_start || status.state != "stopped" || !status.executable_set {
            continue;
        }
        if start_service(&app, &state, &vault, &status.id).is_ok() {
            started.push(status.id);
        }
    }
    started
}

pub fn shutdown(app: &AppHandle) {
    let Some(state) = app.try_state::<LocalServicesState>() else {
        return;
    };
    let configs = read_configs(app);
    let mut processes = state.processes.lock().unwrap();
    let ids: Vec<String> = processes.keys().cloned().collect();
    for id in ids {
        let keep = configs
            .iter()
            .find(|config| config.id == id)
            .map(|config| config.keep_alive_on_exit)
            .unwrap_or(true);
        if keep {
            continue;
        }
        if let Some(entry) = processes.remove(&id) {
            kill_tree(entry.pid);
        }
    }
}

/// Windows 로그인 시 자동 실행 방식. 지금은 인터페이스만 분리해 두고 실제 등록은
/// 추후 확장한다(시작프로그램 폴더 / 작업 스케줄러 / 서비스 등록 등).
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub enum LoginStartupMethod {
    None,
    StartupFolder,
    ScheduledTask,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LoginStartupPlan {
    pub service_id: String,
    pub method: LoginStartupMethod,
    pub supported: bool,
    pub detail: String,
}

#[tauri::command]
pub fn local_service_login_startup_plan(id: String) -> Result<LoginStartupPlan, AppError> {
    let def = definition(&id)
        .ok_or_else(|| AppError::InvalidRequest(format!("알 수 없는 로컬 서비스입니다: {id}")))?;
    Ok(LoginStartupPlan {
        service_id: def.id.clone(),
        method: LoginStartupMethod::None,
        supported: def.supports_login_startup,
        detail: "Windows 로그인 시 자동 실행은 아직 구현되지 않았습니다 (인터페이스만 준비됨)".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_listening_port_owner() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(tcp_alive(port));
        let owner = port_owner_pid(port);
        assert_eq!(owner, Some(std::process::id()));
        drop(listener);
    }

    #[test]
    fn parses_port_owner_from_netstat() {
        let output = "\
  TCP    127.0.0.1:8000         0.0.0.0:0              LISTENING       4242
  TCP    127.0.0.1:8001         0.0.0.0:0              ESTABLISHED     1111
  TCP    [::]:8000              [::]:0                 LISTENING       9999
";
        assert_eq!(parse_port_owner(output, 8000), Some(4242));
        assert_eq!(parse_port_owner(output, 8001), None);
        assert_eq!(parse_port_owner(output, 8002), None);
    }

    #[test]
    fn scrubs_secret_values_from_logs() {
        let secret = "sk-laya-super-secret".to_string();
        let line = "Authorization: Bearer sk-laya-super-secret (loaded)";
        let cleaned = scrub_secrets(line, &[secret]);
        assert!(!cleaned.contains("sk-laya-super-secret"));
        assert!(cleaned.contains("***"));
    }

    #[test]
    fn builds_laya_environment() {
        let def = definition("laya").unwrap();
        let config = ServiceConfig {
            id: "laya".into(),
            device: "cuda".into(),
            ..Default::default()
        }
        .merged(&def);
        let env = service_env(&def, &config, Some("secret-key"));
        let map: HashMap<_, _> = env.into_iter().collect();
        assert_eq!(map.get("USE_TF").map(String::as_str), Some("0"));
        assert_eq!(map.get("LAYA_PRELOAD").map(String::as_str), Some("1"));
        assert_eq!(map.get("LAYA_DEVICE").map(String::as_str), Some("cuda"));
        assert_eq!(map.get("LAYA_API_KEY").map(String::as_str), Some("secret-key"));
    }

    #[test]
    fn blocks_start_only_when_configured_key_unavailable() {
        let def = definition("laya").unwrap();
        let plain = fresh_config("laya").merged(&def);
        let with_key = ServiceConfig {
            api_key_configured: true,
            ..fresh_config("laya")
        }
        .merged(&def);

        // 무인증 모드: Vault가 잠겨 있어도 실행 가능
        assert!(start_block_reason(&plain, true, false).is_none());
        assert!(start_block_reason(&plain, false, false).is_none());
        // 키를 저장해 둔 경우: 잠금 상태면 차단
        let reason = start_block_reason(&with_key, true, false).unwrap();
        assert!(reason.contains("Vault 잠금 해제 필요"), "reason: {reason}");
        // 잠금 해제됐지만 키를 찾지 못하면 안내
        let reason = start_block_reason(&with_key, false, false).unwrap();
        assert!(reason.contains("찾을 수 없습니다"), "reason: {reason}");
        // 정상: 키 존재
        assert!(start_block_reason(&with_key, false, true).is_none());
    }

    #[test]
    fn omits_api_key_when_absent() {
        let def = definition("laya").unwrap();
        let config = fresh_config("laya").merged(&def);
        let env = service_env(&def, &config, None);
        let map: HashMap<_, _> = env.into_iter().collect();
        assert!(!map.contains_key("LAYA_API_KEY"));
        assert_eq!(map.get("USE_TF").map(String::as_str), Some("0"));
        assert_eq!(map.get("LAYA_DEVICE").map(String::as_str), Some("cuda"));
    }

    #[test]
    fn masks_api_key_hint() {
        assert_eq!(api_key_hint("sk-1234567890abcd"), "••••abcd");
        assert_eq!(api_key_hint("abc"), "••••");
    }

    #[test]
    fn merges_defaults_into_config() {
        let def = definition("laya").unwrap();
        let config = ServiceConfig {
            executable: "  C:\\laya\\venv\\Scripts\\python.exe  ".into(),
            args: vec![" -m ".into(), "laya".into(), "".into()],
            ..fresh_config("laya")
        }
        .merged(&def);
        assert_eq!(config.port, 8000);
        assert_eq!(config.endpoint, "http://127.0.0.1:8000");
        assert_eq!(config.device, "cuda");
        assert_eq!(config.health_path, "/health");
        assert!(config.keep_alive_on_exit);
        assert_eq!(config.executable, "C:\\laya\\venv\\Scripts\\python.exe");
        assert_eq!(config.args, vec!["-m".to_string(), "laya".to_string()]);
    }

    #[test]
    fn derives_service_state() {
        assert_eq!(service_state_from(None, None, false).0, "stopped");
        assert_eq!(service_state_from(None, Some(1234), true).0, "running");
        assert_eq!(service_state_from(Some((10, unix_now(), None)), None, false).0, "starting");
        assert_eq!(service_state_from(Some((10, unix_now(), None)), None, true).0, "running");
        assert_eq!(service_state_from(Some((10, unix_now(), Some(0))), None, false).0, "stopped");
        assert_eq!(service_state_from(Some((10, unix_now(), Some(1))), None, false).0, "error");
    }
}
