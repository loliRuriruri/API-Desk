//! GPU/VRAM 모니터.
//!
//! - NVML(동적 로드, nvml.dll) primary, nvidia-smi fallback
//! - GPU: 이름/총·사용·여유 VRAM/사용률/온도/전력/index
//! - 프로세스: PID/이름/VRAM(불가 시 N/A)/서비스 귀속/모델/출처/신뢰도
//! - API Desk LocalServiceManager의 managed PID/descendant/포트 소유자를 귀속 신호로 사용
//! - 프로세스 VRAM을 근거 없이 0으로 속이거나 모델별로 임의 배분하지 않는다.
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::error::AppError;
use crate::local_services::{self, LocalServicesState};

const MAIN_TTL_MS: u64 = 1_000;
const MINI_TTL_MS: u64 = 2_000;
const META_TTL_MS: u64 = 20_000;
const OLLAMA_TTL_MS: u64 = 5_000;
const HEALTH_TTL_MS: u64 = 5_000;

// ---------------------------------------------------------------- structures

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub index: u32,
    pub name: String,
    pub memory_total_bytes: Option<u64>,
    pub memory_used_bytes: Option<u64>,
    pub memory_free_bytes: Option<u64>,
    pub utilization_percent: Option<u32>,
    pub temperature_c: Option<u32>,
    pub power_watts: Option<f64>,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct GpuProcess {
    pub pid: u32,
    pub process_name: String,
    pub used_vram_bytes: Option<u64>,
    pub service: Option<String>,
    pub service_kind: String,
    pub models: Vec<String>,
    pub model_source: Option<String>,
    pub confidence: String,
    pub executable: Option<String>,
    pub is_service_root: bool,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct GpuSnapshot {
    pub available: bool,
    pub reason: Option<String>,
    pub source: String,
    pub fetched_at: String,
    pub gpus: Vec<GpuInfo>,
    pub processes: Vec<GpuProcess>,
    pub other_count: usize,
    pub detail: Option<String>,
}

// ---------------------------------------------------------------- NVML (dynamic)

#[cfg(windows)]
mod nvml {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    pub struct Memory {
        pub total: u64,
        pub free: u64,
        pub used: u64,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    pub struct Utilization {
        pub gpu: u32,
        pub memory: u32,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    pub struct ProcessInfo {
        pub pid: u32,
        pub used_gpu_memory: u64,
        pub gpu_instance_id: u32,
        pub compute_instance_id: u32,
    }

    const NVML_ERROR_INSUFFICIENT_SIZE: i32 = 6;
    pub const NVML_VALUE_NOT_AVAILABLE: u64 = u64::MAX;

    type InitFn = unsafe extern "system" fn() -> i32;
    type ShutdownFn = unsafe extern "system" fn() -> i32;
    type CountFn = unsafe extern "system" fn(*mut u32) -> i32;
    type HandleFn = unsafe extern "system" fn(u32, *mut *mut c_void) -> i32;
    type NameFn = unsafe extern "system" fn(*mut c_void, *mut i8, u32) -> i32;
    type MemoryFn = unsafe extern "system" fn(*mut c_void, *mut Memory) -> i32;
    type UtilFn = unsafe extern "system" fn(*mut c_void, *mut Utilization) -> i32;
    type TempFn = unsafe extern "system" fn(*mut c_void, u32, *mut u32) -> i32;
    type PowerFn = unsafe extern "system" fn(*mut c_void, *mut u32) -> i32;
    type ProcsFn = unsafe extern "system" fn(*mut c_void, *mut u32, *mut ProcessInfo) -> i32;

    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }

    pub struct Nvml {
        _module: *mut c_void,
        pub init: InitFn,
        pub shutdown: ShutdownFn,
        pub count: CountFn,
        pub handle: HandleFn,
        pub name: NameFn,
        pub memory: MemoryFn,
        pub utilization: UtilFn,
        pub temperature: TempFn,
        pub power: PowerFn,
        pub compute_procs: Option<ProcsFn>,
        pub graphics_procs: Option<ProcsFn>,
    }

    fn symbol(module: *mut c_void, name: &str) -> Option<*mut c_void> {
        let mut buffer = Vec::from(name.as_bytes());
        buffer.push(0);
        let address = unsafe { GetProcAddress(module, buffer.as_ptr()) };
        if address.is_null() {
            None
        } else {
            Some(address)
        }
    }

    pub fn load() -> Option<Nvml> {
        let candidates = ["nvml.dll", r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll"];
        let mut module = std::ptr::null_mut();
        for candidate in candidates {
            let wide: Vec<u16> = Path::new(candidate)
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let loaded = unsafe { LoadLibraryW(wide.as_ptr()) };
            if !loaded.is_null() {
                module = loaded;
                break;
            }
        }
        if module.is_null() {
            return None;
        }
        unsafe {
            Some(Nvml {
                _module: module,
                init: std::mem::transmute(symbol(module, "nvmlInit_v2")?),
                shutdown: std::mem::transmute(symbol(module, "nvmlShutdown")?),
                count: std::mem::transmute(symbol(module, "nvmlDeviceGetCount_v2")?),
                handle: std::mem::transmute(symbol(module, "nvmlDeviceGetHandleByIndex_v2")?),
                name: std::mem::transmute(symbol(module, "nvmlDeviceGetName")?),
                memory: std::mem::transmute(symbol(module, "nvmlDeviceGetMemoryInfo")?),
                utilization: std::mem::transmute(symbol(module, "nvmlDeviceGetUtilizationRates")?),
                temperature: std::mem::transmute(symbol(module, "nvmlDeviceGetTemperature")?),
                power: std::mem::transmute(symbol(module, "nvmlDeviceGetPowerUsage")?),
                compute_procs: symbol(module, "nvmlDeviceGetComputeRunningProcesses_v3")
                    .or_else(|| symbol(module, "nvmlDeviceGetComputeRunningProcesses_v2"))
                    .map(|address| std::mem::transmute(address)),
                graphics_procs: symbol(module, "nvmlDeviceGetGraphicsRunningProcesses_v3")
                    .or_else(|| symbol(module, "nvmlDeviceGetGraphicsRunningProcesses_v2"))
                    .map(|address| std::mem::transmute(address)),
            })
        }
    }

    pub fn running_processes(
        device: *mut c_void,
        function: ProcsFn,
    ) -> Vec<ProcessInfo> {
        let mut capacity: u32 = 128;
        loop {
            let mut buffer = vec![ProcessInfo::default(); capacity as usize];
            let mut count = capacity;
            let result = unsafe { function(device, &mut count, buffer.as_mut_ptr()) };
            if result == 0 {
                buffer.truncate(count as usize);
                return buffer;
            }
            if result == NVML_ERROR_INSUFFICIENT_SIZE && capacity < 4096 {
                capacity *= 2;
                continue;
            }
            return Vec::new();
        }
    }
}

#[cfg(not(windows))]
mod nvml {
    pub struct Nvml;
    pub fn load() -> Option<Nvml> {
        None
    }
}

// ---------------------------------------------------------------- nvidia-smi fallback

fn smi_query(args: &[&str]) -> Option<String> {
    let output = Command::new("nvidia-smi")
        .args(args)
        .creation_flags_hidden()
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn parse_optional_u64(text: &str) -> Option<u64> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("N/A") || trimmed.eq_ignore_ascii_case("[N/A]") {
        return None;
    }
    trimmed.parse::<u64>().ok()
}

pub fn parse_smi_gpu_line(line: &str) -> Option<GpuInfo> {
    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
    if parts.len() < 8 {
        return None;
    }
    let index = parts[0].parse::<u32>().ok()?;
    let mebibytes = 1024 * 1024;
    Some(GpuInfo {
        index,
        name: parts[1].to_string(),
        memory_total_bytes: parse_optional_u64(parts[2]).map(|value| value * mebibytes),
        memory_used_bytes: parse_optional_u64(parts[3]).map(|value| value * mebibytes),
        memory_free_bytes: parse_optional_u64(parts[4]).map(|value| value * mebibytes),
        utilization_percent: parse_optional_u64(parts[5]).map(|value| value as u32),
        temperature_c: parse_optional_u64(parts[6]).map(|value| value as u32),
        power_watts: parts[7].trim().parse::<f64>().ok(),
    })
}

pub fn parse_smi_process_line(line: &str) -> Option<(u32, String, Option<u64>)> {
    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
    if parts.len() < 3 {
        return None;
    }
    let pid = parts[0].parse::<u32>().ok()?;
    let name = parts[1].to_string();
    let memory = parse_optional_u64(parts[2]).map(|value| value * 1024 * 1024);
    Some((pid, name, memory))
}

fn smi_snapshot() -> Option<(Vec<GpuInfo>, Vec<(u32, String, Option<u64>)>)> {
    let gpu_output = smi_query(&[
        "--query-gpu=index,name,memory.total,memory.used,memory.free,utilization.gpu,temperature.gpu,power.draw",
        "--format=csv,noheader,nounits",
    ])?;
    let gpus: Vec<GpuInfo> = gpu_output.lines().filter_map(parse_smi_gpu_line).collect();
    if gpus.is_empty() {
        return None;
    }
    let mut processes = Vec::new();
    for query in ["--query-compute-apps=pid,process_name,used_memory", "--query-graphics-apps=pid,process_name,used_memory"] {
        if let Some(output) = smi_query(&[query, "--format=csv,noheader,nounits"]) {
            for line in output.lines() {
                if let Some(entry) = parse_smi_process_line(line) {
                    processes.push(entry);
                }
            }
        }
    }
    Some((gpus, processes))
}

// ---------------------------------------------------------------- process metadata

#[derive(Clone, Default)]
struct ProcMeta {
    name: String,
    parent: Option<u32>,
    cmdline: Option<String>,
}

#[derive(Default)]
struct MetaCache {
    fetched_at: Option<Instant>,
    refreshing: bool,
    map: HashMap<u32, ProcMeta>,
}

#[derive(Default)]
pub struct GpuMonitorState {
    cache: Mutex<Option<(GpuSnapshot, Instant)>>,
    meta: Mutex<MetaCache>,
    ollama: Mutex<Option<(Vec<String>, Instant)>>,
    health: Mutex<Option<(Vec<String>, Instant)>>,
}

fn exe_path(pid: u32) -> Option<String> {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        use std::os::windows::ffi::OsStringExt;

        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            fn QueryFullProcessImageNameW(
                handle: *mut c_void,
                flags: u32,
                buffer: *mut u16,
                size: *mut u32,
            ) -> i32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let mut buffer = vec![0u16; 1024];
            let mut size = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size);
            CloseHandle(handle);
            if ok == 0 {
                return None;
            }
            buffer.truncate(size as usize);
            return Some(
                std::ffi::OsString::from_wide(&buffer)
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}

fn base_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

fn cim_metadata() -> HashMap<u32, ProcMeta> {
    let script = r#"
$ProgressPreference = 'SilentlyContinue'
Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,CommandLine | ConvertTo-Json -Compress -Depth 3
"#;
    let Ok(output) = crate::monitors::run_powershell_hidden(script) else {
        return HashMap::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&output) else {
        return HashMap::new();
    };
    let entries: Vec<Value> = match value {
        Value::Array(items) => items,
        other => vec![other],
    };
    let mut map = HashMap::new();
    for entry in entries {
        let Some(pid) = entry.get("ProcessId").and_then(Value::as_u64) else {
            continue;
        };
        map.insert(
            pid as u32,
            ProcMeta {
                name: entry
                    .get("Name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                parent: entry
                    .get("ParentProcessId")
                    .and_then(Value::as_u64)
                    .map(|value| value as u32),
                cmdline: entry
                    .get("CommandLine")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
        );
    }
    map
}

fn metadata_snapshot(app: &AppHandle, state: &GpuMonitorState) -> HashMap<u32, ProcMeta> {
    {
        let cache = state.meta.lock().unwrap();
        let stale = cache
            .fetched_at
            .map(|at| at.elapsed().as_millis() as u64 > META_TTL_MS)
            .unwrap_or(true);
        if !stale {
            return cache.map.clone();
        }
    }
    let empty = {
        let cache = state.meta.lock().unwrap();
        cache.map.is_empty()
    };
    if empty {
        // 첫 호출은 동기 갱신(이후 TTL 동안 캐시 사용)
        let map = cim_metadata();
        let mut cache = state.meta.lock().unwrap();
        if !map.is_empty() {
            cache.map = map;
            cache.fetched_at = Some(Instant::now());
        }
        return cache.map.clone();
    }
    // 이후에는 stale-while-revalidate: 현재 캐시를 즉시 돌려주고 백그라운드에서 갱신한다.
    {
        let mut cache = state.meta.lock().unwrap();
        if !cache.refreshing {
            cache.refreshing = true;
            let handle = app.clone();
            std::thread::spawn(move || {
                let map = cim_metadata();
                let state = handle.state::<GpuMonitorState>();
                let mut cache = state.meta.lock().unwrap();
                if !map.is_empty() {
                    cache.map = map;
                    cache.fetched_at = Some(Instant::now());
                }
                cache.refreshing = false;
            });
        }
    }
    let cache = state.meta.lock().unwrap();
    cache.map.clone()
}

fn descendants_of(root: u32, map: &HashMap<u32, ProcMeta>) -> HashSet<u32> {
    let mut result = HashSet::new();
    let mut frontier = vec![root];
    while let Some(current) = frontier.pop() {
        for (pid, meta) in map {
            if meta.parent == Some(current) && result.insert(*pid) {
                frontier.push(*pid);
            }
        }
    }
    result
}

// ---------------------------------------------------------------- hints / sanitization

const SECRET_MARKERS: [&str; 6] = ["api-key", "apikey", "api_key", "token", "secret", "password"];

/// 모델/경로 힌트에서 자격증명으로 보이는 부분을 제거한다.
pub fn sanitize_hint(text: &str) -> String {
    let mut out = String::new();
    let mut skip_next = false;
    for token in text.split_whitespace() {
        let lower = token.to_lowercase();
        if skip_next {
            skip_next = false;
            continue;
        }
        if SECRET_MARKERS.iter().any(|marker| lower.contains(marker)) {
            if !token.contains('=') {
                skip_next = true;
            }
            continue;
        }
        if lower.starts_with("sk-")
            || lower.starts_with("ghp_")
            || lower.starts_with("bearer")
            || lower.starts_with("eyj")
            || token.len() > 200
        {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(token);
    }
    out.chars().take(160).collect()
}

fn model_from_cmdline(cmdline: &str) -> Option<String> {
    let tokens: Vec<&str> = cmdline.split_whitespace().collect();
    for (index, token) in tokens.iter().enumerate() {
        let clean = token.trim_matches('"');
        if clean == "--model" || clean == "--model-path" || clean == "--ckpt" {
            if let Some(value) = tokens.get(index + 1) {
                let name = base_name(value.trim_matches('"'));
                if !name.is_empty() {
                    return Some(sanitize_hint(&name));
                }
            }
            continue;
        }
        if clean == "-m" {
            // python -m <module> 은 모델 인자가 아니므로 제외하고,
            // 경로처럼 보이는 경우(예: llama.cpp -m model.gguf)만 모델로 인정한다.
            let previous = index
                .checked_sub(1)
                .and_then(|slot| tokens.get(slot))
                .map(|value| value.trim_matches('"').to_lowercase())
                .unwrap_or_default();
            if previous.ends_with("python") || previous.ends_with("python.exe") || previous == "py" {
                continue;
            }
            if let Some(value) = tokens.get(index + 1) {
                let path = value.trim_matches('"');
                let lower = path.to_lowercase();
                let looks_like_path = path.contains('\\')
                    || path.contains('/')
                    || lower.ends_with(".gguf")
                    || lower.ends_with(".bin")
                    || lower.ends_with(".safetensors");
                if looks_like_path {
                    let name = base_name(path);
                    if !name.is_empty() {
                        return Some(sanitize_hint(&name));
                    }
                }
            }
            continue;
        }
        if let Some(rest) = clean.strip_prefix("--model=") {
            let name = base_name(rest);
            if !name.is_empty() {
                return Some(sanitize_hint(&name));
            }
        }
    }
    None
}

#[derive(Clone, Copy, PartialEq)]
pub enum ToolKind {
    Ollama,
    LlamaCpp,
    Vllm,
    ComfyUi,
    GenericPython,
    Other,
}

pub fn classify_tool(process_name: &str, cmdline: Option<&str>) -> ToolKind {
    let name = process_name.to_lowercase();
    let command = cmdline.unwrap_or("").to_lowercase();
    if name.contains("ollama") || command.contains("ollama") {
        return ToolKind::Ollama;
    }
    if name.contains("llama-server") || name.contains("llama-cli") || command.contains("llama.cpp") {
        return ToolKind::LlamaCpp;
    }
    if name.contains("vllm") || command.contains("vllm") {
        return ToolKind::Vllm;
    }
    if name.contains("comfyui") || command.contains("comfyui") || command.contains("comfy") {
        return ToolKind::ComfyUi;
    }
    if name.contains("python") || name.contains("laya") || command.contains("laya") {
        return ToolKind::GenericPython;
    }
    ToolKind::Other
}

// ---------------------------------------------------------------- ollama / laya health

fn ollama_models() -> Vec<String> {
    let Some(output) = Command::new("ollama")
        .args(["ps"])
        .creation_flags_hidden()
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
    else {
        return Vec::new();
    };
    output
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .filter(|name| !name.is_empty())
        .collect()
}

fn http_get_json(host: &str, port: u16, path: &str, timeout_ms: u64) -> Option<Value> {
    let address = format!("{host}:{port}");
    let socket = address.parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&socket, Duration::from_millis(timeout_ms)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(timeout_ms)));
    let request = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    let body = response.split("\r\n\r\n").nth(1)?;
    serde_json::from_str::<Value>(body).ok()
}

// ---------------------------------------------------------------- snapshot build

fn snapshot_from_nvml() -> Option<(Vec<GpuInfo>, Vec<(u32, String, Option<u64>)>)> {
    #[cfg(windows)]
    {
        let nvml = nvml::load()?;
        if unsafe { (nvml.init)() } != 0 {
            return None;
        }
        let mut count: u32 = 0;
        if unsafe { (nvml.count)(&mut count) } != 0 || count == 0 {
            unsafe {
                let _ = (nvml.shutdown)();
            }
            return None;
        }
        let mut gpus = Vec::new();
        let mut processes: Vec<(u32, String, Option<u64>)> = Vec::new();
        for index in 0..count {
            let mut handle: *mut std::ffi::c_void = std::ptr::null_mut();
            if unsafe { (nvml.handle)(index, &mut handle) } != 0 {
                continue;
            }
            let mut name_buffer = vec![0i8; 96];
            let name = if unsafe { (nvml.name)(handle, name_buffer.as_mut_ptr(), 96) } == 0 {
                let bytes: Vec<u8> = name_buffer
                    .iter()
                    .take_while(|byte| **byte != 0)
                    .map(|byte| *byte as u8)
                    .collect();
                String::from_utf8_lossy(&bytes).to_string()
            } else {
                format!("GPU {index}")
            };
            let mut memory = nvml::Memory::default();
            let memory_ok = unsafe { (nvml.memory)(handle, &mut memory) } == 0;
            let mut utilization = nvml::Utilization::default();
            let utilization_ok = unsafe { (nvml.utilization)(handle, &mut utilization) } == 0;
            let mut temperature: u32 = 0;
            let temperature_ok = unsafe { (nvml.temperature)(handle, 0, &mut temperature) } == 0;
            let mut power: u32 = 0;
            let power_ok = unsafe { (nvml.power)(handle, &mut power) } == 0;
            gpus.push(GpuInfo {
                index,
                name,
                memory_total_bytes: memory_ok.then_some(memory.total),
                memory_used_bytes: memory_ok.then_some(memory.used),
                memory_free_bytes: memory_ok.then_some(memory.free),
                utilization_percent: utilization_ok.then_some(utilization.gpu),
                temperature_c: temperature_ok.then_some(temperature),
                power_watts: power_ok.then_some(power as f64 / 1000.0),
            });
            let mut seen: HashSet<u32> = HashSet::new();
            for function in [nvml.compute_procs, nvml.graphics_procs].into_iter().flatten() {
                for entry in nvml::running_processes(handle, function) {
                    if !seen.insert(entry.pid) {
                        continue;
                    }
                    let memory = if entry.used_gpu_memory == nvml::NVML_VALUE_NOT_AVAILABLE {
                        None
                    } else {
                        Some(entry.used_gpu_memory)
                    };
                    processes.push((entry.pid, String::new(), memory));
                }
            }
        }
        unsafe {
            let _ = (nvml.shutdown)();
        }
        if gpus.is_empty() {
            return None;
        }
        return Some((gpus, processes));
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn now_iso() -> String {
    crate::usage::epoch_to_rfc3339(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs() as i64)
            .unwrap_or(0),
    )
}

fn build_processes(
    app: &AppHandle,
    state: &GpuMonitorState,
    services: &LocalServicesState,
    raw: &[(u32, String, Option<u64>)],
) -> (Vec<GpuProcess>, usize) {
    let metadata = metadata_snapshot(app, state);

    // 서비스 귀속 신호: managed PID + descendant + 포트 소유자
    let mut owner: HashMap<u32, (String, String, bool)> = HashMap::new();
    for (id, pid) in services.managed_pids() {
        let label = local_services::definitions()
            .into_iter()
            .find(|def| def.id == id)
            .map(|def| def.label)
            .unwrap_or(id.clone());
        owner.insert(pid, (label.clone(), id.clone(), true));
        for child in descendants_of(pid, &metadata) {
            owner.entry(child).or_insert((label.clone(), id.clone(), false));
        }
    }
    for (id, label, port) in services.service_ports(app) {
        if let Some(pid) = local_services::port_owner(port) {
            owner.entry(pid).or_insert((label, id, false));
        }
    }

    let ollama = {
        let mut cache = state.ollama.lock().unwrap();
        let stale = cache
            .as_ref()
            .map(|(_, at)| at.elapsed().as_millis() as u64 > OLLAMA_TTL_MS)
            .unwrap_or(true);
        if stale {
            *cache = Some((ollama_models(), Instant::now()));
        }
        cache.as_ref().map(|(models, _)| models.clone()).unwrap_or_default()
    };

    let laya_models = {
        let mut cache = state.health.lock().unwrap();
        let stale = cache
            .as_ref()
            .map(|(_, at)| at.elapsed().as_millis() as u64 > HEALTH_TTL_MS)
            .unwrap_or(true);
        if stale {
            let models = http_get_json("127.0.0.1", 8000, "/health", 600)
                .and_then(|body| body.get("loaded").cloned())
                .and_then(|loaded| loaded.as_array().cloned())
                .map(|items| {
                    items
                        .into_iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default();
            *cache = Some((models, Instant::now()));
        }
        cache.as_ref().map(|(models, _)| models.clone()).unwrap_or_default()
    };

    let mut result = Vec::new();
    let mut other_count = 0usize;
    for (pid, fallback_name, vram) in raw {
        let meta = metadata.get(pid).cloned().unwrap_or_default();
        let process_name = if !fallback_name.is_empty() {
            base_name(fallback_name)
        } else if !meta.name.is_empty() {
            meta.name.clone()
        } else {
            exe_path(*pid)
                .map(|path| base_name(&path))
                .unwrap_or_else(|| format!("PID {pid}"))
        };
        let executable = exe_path(*pid).map(|path| base_name(&path));

        let mut process = GpuProcess {
            pid: *pid,
            process_name: process_name.clone(),
            used_vram_bytes: *vram,
            service: None,
            service_kind: "unknown".into(),
            models: Vec::new(),
            model_source: None,
            confidence: "low".into(),
            executable,
            is_service_root: false,
        };

        if let Some((label, id, is_root)) = owner.get(pid) {
            process.service = Some(label.clone());
            process.service_kind = "managed_service".into();
            process.is_service_root = *is_root;
            process.confidence = "high".into();
            if id == "laya" && !laya_models.is_empty() {
                process.models = laya_models.clone();
                process.model_source = Some("laya_health".into());
            }
            result.push(process);
            continue;
        }

        let cmdline = meta.cmdline.as_deref();
        match classify_tool(&process_name, cmdline) {
            ToolKind::Ollama => {
                process.service = Some("Ollama".into());
                process.service_kind = "ollama".into();
                process.confidence = "medium".into();
                if !ollama.is_empty() {
                    process.models = ollama.clone();
                    process.model_source = Some("ollama_ps".into());
                }
                result.push(process);
            }
            ToolKind::LlamaCpp => {
                process.service = Some("llama.cpp".into());
                process.service_kind = "llama_cpp".into();
                process.confidence = "medium".into();
                process.models = model_from_cmdline(cmdline.unwrap_or("")).into_iter().collect();
                process.model_source = Some("cmdline".into());
                result.push(process);
            }
            ToolKind::Vllm => {
                process.service = Some("vLLM".into());
                process.service_kind = "vllm".into();
                process.confidence = "medium".into();
                process.models = model_from_cmdline(cmdline.unwrap_or("")).into_iter().collect();
                process.model_source = Some("cmdline".into());
                result.push(process);
            }
            ToolKind::ComfyUi => {
                process.service = Some("ComfyUI".into());
                process.service_kind = "comfyui".into();
                process.confidence = "low".into();
                process.models = model_from_cmdline(cmdline.unwrap_or("")).into_iter().collect();
                process.model_source = Some("cmdline_estimated".into());
                result.push(process);
            }
            ToolKind::GenericPython => {
                process.service = Some("Unknown AI workload".into());
                process.service_kind = "unknown_ai".into();
                process.confidence = "low".into();
                process.model_source = cmdline.map(|line| sanitize_hint(line));
                result.push(process);
            }
            ToolKind::Other => {
                other_count += 1;
                if other_count <= 8 {
                    process.service = Some("Other".into());
                    process.service_kind = "other".into();
                    process.confidence = "low".into();
                    result.push(process);
                }
            }
        }
    }

    result.sort_by(|a, b| {
        b.used_vram_bytes
            .unwrap_or(0)
            .cmp(&a.used_vram_bytes.unwrap_or(0))
            .then_with(|| a.pid.cmp(&b.pid))
    });
    (result, other_count)
}

fn refresh_snapshot(app: &AppHandle) -> GpuSnapshot {
    let services = app.state::<LocalServicesState>();
    let monitor = app.state::<GpuMonitorState>();

    let nvml_data = snapshot_from_nvml();
    let (source, gpus, raw) = match nvml_data {
        Some((gpus, processes)) => ("nvml", gpus, processes),
        None => match smi_snapshot() {
            Some((gpus, processes)) => ("nvidia-smi", gpus, processes),
            None => {
                return GpuSnapshot {
                    available: false,
                    reason: Some("nvml_unavailable".into()),
                    source: "none".into(),
                    fetched_at: now_iso(),
                    detail: Some("NVIDIA GPU 정보를 사용할 수 없음".into()),
                    ..Default::default()
                }
            }
        },
    };

    let (processes, other_count) = build_processes(app, &monitor, &services, &raw);
    GpuSnapshot {
        available: true,
        reason: None,
        source: source.into(),
        fetched_at: now_iso(),
        gpus,
        processes,
        other_count,
        detail: None,
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub async fn gpu_snapshot(
    app: AppHandle,
    view: Option<String>,
    visible: Option<bool>,
) -> Result<GpuSnapshot, AppError> {
    let ttl = match view.as_deref() {
        Some("mini") => MINI_TTL_MS,
        _ => MAIN_TTL_MS,
    };
    let visible = visible.unwrap_or(true);
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<GpuMonitorState>();
        {
            let cache = state.cache.lock().unwrap();
            if let Some((snapshot, at)) = cache.as_ref() {
                let fresh = at.elapsed().as_millis() as u64 <= ttl;
                if fresh || !visible {
                    return snapshot.clone();
                }
            }
        }
        let snapshot = refresh_snapshot(&handle);
        let mut cache = state.cache.lock().unwrap();
        *cache = Some((snapshot.clone(), Instant::now()));
        snapshot
    })
    .await
    .map_err(|error| AppError::Io(format!("GPU 스냅샷 작업 실패: {error}")))
}

#[tauri::command]
pub async fn gpu_process_details(app: AppHandle) -> Result<Value, AppError> {
    let snapshot = gpu_snapshot(app, Some("main".into()), Some(true)).await?;
    Ok(json!({ "processes": snapshot.processes, "otherCount": snapshot.other_count }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_smi_gpu_line() {
        let info = parse_smi_gpu_line(
            "0, NVIDIA GeForce RTX 5090, 32607, 8298, 23890, 2, 47, 46.19",
        )
        .unwrap();
        assert_eq!(info.index, 0);
        assert_eq!(info.name, "NVIDIA GeForce RTX 5090");
        assert_eq!(info.memory_total_bytes, Some(32607 * 1024 * 1024));
        assert_eq!(info.utilization_percent, Some(2));
        assert_eq!(info.temperature_c, Some(47));
        assert!((info.power_watts.unwrap() - 46.19).abs() < 0.01);
    }

    #[test]
    fn parses_smi_process_line_with_na() {
        let (pid, name, memory) = parse_smi_process_line("1888, C:\\Windows\\dwm.exe, [N/A]").unwrap();
        assert_eq!(pid, 1888);
        assert!(name.ends_with("dwm.exe"));
        assert_eq!(memory, None);
        let (_, _, memory) = parse_smi_process_line("42, python.exe, 2048").unwrap();
        assert_eq!(memory, Some(2048 * 1024 * 1024));
    }

    #[test]
    fn parses_model_from_llama_cpp_cmdline() {
        let cmd = "llama-server.exe -m C:\\models\\qwen3.5-9b-instruct-Q4_K_M.gguf --port 8080";
        assert_eq!(
            model_from_cmdline(cmd).as_deref(),
            Some("qwen3.5-9b-instruct-Q4_K_M.gguf")
        );
        let vllm = "python -m vllm.entrypoints.openai.api_server --model meta-llama/Llama-3-8B";
        assert_eq!(model_from_cmdline(vllm).as_deref(), Some("Llama-3-8B"));
        assert_eq!(model_from_cmdline("python app.py"), None);
    }

    #[test]
    fn sanitizes_secret_like_arguments() {
        let hint = "python server.py --api-key sk-abcdef123456 --model qwen.gguf";
        let cleaned = sanitize_hint(hint);
        assert!(!cleaned.contains("sk-abcdef123456"));
        assert!(!cleaned.contains("--api-key"));
        assert!(cleaned.contains("qwen.gguf"));
    }

    #[test]
    fn classifies_tools() {
        assert!(matches!(classify_tool("ollama.exe", None), ToolKind::Ollama));
        assert!(matches!(
            classify_tool("llama-server.exe", Some("llama-server -m x.gguf")),
            ToolKind::LlamaCpp
        ));
        assert!(matches!(
            classify_tool("python.exe", Some("python -m vllm.entrypoints.api")),
            ToolKind::Vllm
        ));
        assert!(matches!(
            classify_tool("python.exe", Some("python ComfyUI\\main.py")),
            ToolKind::ComfyUi
        ));
        assert!(matches!(classify_tool("python.exe", Some("python -m laya.server")), ToolKind::GenericPython));
        assert!(matches!(classify_tool("dwm.exe", None), ToolKind::Other));
    }

    #[test]
    fn finds_descendant_tree() {
        let mut map = HashMap::new();
        map.insert(1, ProcMeta { name: "laya-serve.exe".into(), parent: Some(0), cmdline: None });
        map.insert(2, ProcMeta { name: "python.exe".into(), parent: Some(1), cmdline: None });
        map.insert(3, ProcMeta { name: "python.exe".into(), parent: Some(2), cmdline: None });
        map.insert(4, ProcMeta { name: "other.exe".into(), parent: Some(9), cmdline: None });
        let descendants = descendants_of(1, &map);
        assert!(descendants.contains(&2));
        assert!(descendants.contains(&3));
        assert!(!descendants.contains(&4));
    }

    #[test]
    fn snapshot_serializes_camel_case_with_na() {
        let snapshot = GpuSnapshot {
            available: true,
            source: "nvml".into(),
            gpus: vec![GpuInfo {
                index: 0,
                name: "NVIDIA GeForce RTX 5090".into(),
                memory_total_bytes: Some(1024),
                memory_used_bytes: None,
                ..Default::default()
            }],
            processes: vec![GpuProcess {
                pid: 42,
                process_name: "python.exe".into(),
                used_vram_bytes: None,
                service: Some("Laya".into()),
                service_kind: "managed_service".into(),
                models: vec!["multilingual".into()],
                model_source: Some("laya_health".into()),
                confidence: "high".into(),
                executable: Some("python.exe".into()),
                is_service_root: false,
            }],
            ..Default::default()
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["gpus"][0]["memoryUsedBytes"], Value::Null);
        assert_eq!(value["processes"][0]["usedVramBytes"], Value::Null);
        assert_eq!(value["processes"][0]["serviceKind"], "managed_service");
        assert_eq!(value["processes"][0]["models"][0], "multilingual");
        assert_eq!(value["available"], true);
    }

    #[test]
    fn unavailable_snapshot_is_explicit() {
        let snapshot = GpuSnapshot {
            available: false,
            reason: Some("nvml_unavailable".into()),
            source: "none".into(),
            detail: Some("NVIDIA GPU 정보를 사용할 수 없음".into()),
            ..Default::default()
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["available"], false);
        assert_eq!(value["reason"], "nvml_unavailable");
        assert_eq!(value["gpus"].as_array().unwrap().len(), 0);
    }
}

trait HiddenCommand {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}

impl HiddenCommand for Command {
    #[cfg(windows)]
    fn creation_flags_hidden(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(0x0800_0000)
    }

    #[cfg(not(windows))]
    fn creation_flags_hidden(&mut self) -> &mut Self {
        self
    }
}
