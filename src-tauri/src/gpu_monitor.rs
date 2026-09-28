//! GPU/VRAM 모니터 V2 — GPU 프로세스 귀속.
//!
//! 수집 파이프라인(NVML primary + Windows PDH 보조):
//!   NVML compute/graphics + PDH(엔진 사용률/전용·공유 메모리) → PID 병합 →
//!   프로세스 리졸버(부모/서비스/런타임/모델/분류) → GpuProcess 스냅샷
//!
//! 원칙:
//! - "Unknown AI workload" 같은 임의 AI 분류를 만들지 않는다(증거 기반 분류만).
//! - 프로세스 커맨드 라인은 내부 분류 전용이며 DTO/프런트엔드로 내보내지 않는다.
//! - 프로세스 VRAM은 출처 우선순위(PDH dedicated > NVML > N/A)로 하나만 표시하고 합산하지 않는다.
//! - 조회 실패/권한 오류는 치명적이지 않다(best-effort).
//!
//! 참조(행위 참고, 코드 미복사): XuehaiPan/nvitop(Apache-2.0), lablup/all-smi(Apache-2.0),
//! GuillaumeGomez/sysinfo(MIT), winsiderss/systeminformer(MIT), GameTechDev/PresentMon(MIT).
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
use crate::gpu_meta::{MetaCache, ProcIdentity, ProcMeta};
use crate::gpu_pdh::{self, PdhSnapshot};
use crate::local_services::{self, LocalServicesState};

const MAIN_TTL_MS: u64 = 1_000;
const MINI_TTL_MS: u64 = 2_000;
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

/// GPU 프로세스 DTO V2.
/// 기존 필드(service/serviceKind/models/...)는 호환을 위해 유지하고,
/// 식별/분류/엔진 정보를 추가로 제공한다. 커맨드 라인은 포함하지 않는다.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct GpuProcess {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub process_name: String,
    pub display_name: String,
    pub product_name: Option<String>,
    pub executable: Option<String>,

    pub classification: String,
    pub runtime: Option<String>,
    pub models: Vec<String>,
    pub model_source: Option<String>,

    pub gpu_percent: Option<f64>,
    pub dominant_engine: Option<String>,

    /// 표시용 프로세스 전용 VRAM(출처 우선순위 적용, 합산 금지).
    pub used_vram_bytes: Option<u64>,
    pub dedicated_vram_bytes: Option<u64>,
    pub shared_gpu_bytes: Option<u64>,
    pub vram_source: Option<String>,

    pub service: Option<String>,
    pub service_kind: String,
    pub managed: bool,
    pub is_service_root: bool,
    pub confidence: String,
    pub sources: Vec<String>,
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
    /// 표시 지표(GPU %, VRAM)가 없는 유휴 항목 수. UI의 "+N" 표시용.
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

    pub fn running_processes(device: *mut c_void, function: ProcsFn) -> Vec<ProcessInfo> {
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

/// (GPU 목록, (pid, compute 여부, NVML VRAM) 목록)
fn smi_snapshot() -> Option<(Vec<GpuInfo>, Vec<(u32, bool, Option<u64>)>)> {
    let gpu_output = smi_query(&[
        "--query-gpu=index,name,memory.total,memory.used,memory.free,utilization.gpu,temperature.gpu,power.draw",
        "--format=csv,noheader,nounits",
    ])?;
    let gpus: Vec<GpuInfo> = gpu_output.lines().filter_map(parse_smi_gpu_line).collect();
    if gpus.is_empty() {
        return None;
    }
    let mut processes = Vec::new();
    for (query, compute) in [
        ("--query-compute-apps=pid,process_name,used_memory", true),
        ("--query-graphics-apps=pid,process_name,used_memory", false),
    ] {
        if let Some(output) = smi_query(&[query, "--format=csv,noheader,nounits"]) {
            for line in output.lines() {
                if let Some((pid, _, memory)) = parse_smi_process_line(line) {
                    processes.push((pid, compute, memory));
                }
            }
        }
    }
    Some((gpus, processes))
}

// ---------------------------------------------------------------- process identity helpers

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

/// gpu_meta 모듈에서 사용하는 공개 래퍼.
pub(crate) fn exe_path_pub(pid: u32) -> Option<String> {
    exe_path(pid)
}

fn base_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

/// managed 서비스 루트의 자손 PID 집합.
pub(crate) fn descendants_of(root: u32, identities: &HashMap<u32, ProcIdentity>) -> HashSet<u32> {
    let mut result = HashSet::new();
    let mut frontier = vec![root];
    while let Some(current) = frontier.pop() {
        for (pid, identity) in identities {
            if identity.parent == Some(current) && result.insert(*pid) {
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
        if clean == "-m" || clean == "--model-file" {
            // python -m <module> 은 모델 인자가 아니므로 제외하고,
            // 경로처럼 보이는 경우(예: llama.cpp -m model.gguf)만 모델로 인정한다.
            let previous = index
                .checked_sub(1)
                .and_then(|slot| tokens.get(slot))
                .map(|value| value.trim_matches('"').to_lowercase())
                .unwrap_or_default();
            if clean == "-m"
                && (previous.ends_with("python")
                    || previous.ends_with("python.exe")
                    || previous == "py")
            {
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

#[allow(dead_code)] // 진단/테스트용 분류기
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ToolKind {
    Ollama,
    LlamaCpp,
    Vllm,
    ComfyUi,
    GenericPython,
    Other,
}

#[allow(dead_code)]
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
    if name.contains("comfyui") || command.contains("comfyui") {
        return ToolKind::ComfyUi;
    }
    if name.contains("python") || name.contains("laya") || command.contains("laya") {
        return ToolKind::GenericPython;
    }
    ToolKind::Other
}

// ---------------------------------------------------------------- evidence tables

const BROWSER_EXES: [&str; 8] = [
    "msedge.exe",
    "msedgewebview2.exe",
    "chrome.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "vivaldi.exe",
    "whale.exe",
];

const VIDEO_EXES: [&str; 6] = [
    "obs64.exe",
    "obs32.exe",
    "vlc.exe",
    "mpc-hc64.exe",
    "potplayermini64.exe",
    "potplayer64.exe",
];

const SYSTEM_EXES: [&str; 11] = [
    "dwm.exe",
    "csrss.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "fontdrvhost.exe",
    "explorer.exe",
    "system",
    "registry",
    "memory compression",
    "secure system",
];

const LAUNCHER_EXES: [&str; 13] = [
    "steam.exe",
    "steamwebhelper.exe",
    "epicgameslauncher.exe",
    "galaxyclient.exe",
    "ubisoftconnect.exe",
    "upc.exe",
    "riotclientservices.exe",
    "battle.net.exe",
    "gamingservices.exe",
    "gamingservicesnet.exe",
    "dmmgameplayer.exe",
    "gamesplayassist.exe",
    "gamesplayassist_x64.exe",
];

const PYTHON_HINTS: [&str; 5] = ["python.exe", "pythonw.exe", "python3.exe", "python3.11.exe", "py.exe"];

const CUDA_HINTS: [&str; 8] = [
    "torch",
    "cuda",
    "tensorflow",
    "jax",
    "diffusers",
    "safetensors",
    "onnx",
    "vllm",
];

fn is_python_like(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with("python") || PYTHON_HINTS.contains(&lower.as_str())
}

fn is_launcher(name: &str) -> bool {
    LAUNCHER_EXES.contains(&name.to_lowercase().as_str())
}

fn engine_runtime_suffix(engine: Option<&str>) -> &'static str {
    match engine {
        Some(value) if value.eq_ignore_ascii_case("3D") => "3D",
        Some(value) if value.eq_ignore_ascii_case("Compute") => "Compute",
        Some(value) if value.eq_ignore_ascii_case("Copy") => "Copy",
        Some(value) if value.eq_ignore_ascii_case("VideoDecode") => "Decode",
        Some(value) if value.eq_ignore_ascii_case("VideoEncode") => "Encode",
        Some(value) if value.eq_ignore_ascii_case("VideoProcessing") => "Video",
        _ => "Graphics",
    }
}

// ---------------------------------------------------------------- merger

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RawGpuProcess {
    pub pid: u32,
    pub sources: Vec<&'static str>,
    pub nvml_bytes: Option<u64>,
    pub engines: Vec<(String, f64)>,
    pub pdh_dedicated: Option<u64>,
    pub pdh_shared: Option<u64>,
}

impl RawGpuProcess {
    /// 헤드라인 GPU % = 가장 바쁜 엔진(합산 금지, 0..100 클램프).
    pub fn headline(&self) -> Option<(String, f64)> {
        self.engines
            .iter()
            .max_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| gpu_pdh::engine_rank(&b.0).cmp(&gpu_pdh::engine_rank(&a.0)))
            })
            .map(|(engine, percent)| (engine.clone(), percent.clamp(0.0, 100.0)))
    }
}

/// NVML + PDH 결과를 PID 기준으로 병합한다(어댑터+PID; 중복 엔트리 금지).
pub(crate) fn merge_raw_processes(
    nvml: &[(u32, bool, Option<u64>)],
    pdh: &PdhSnapshot,
) -> Vec<RawGpuProcess> {
    let mut map: HashMap<u32, RawGpuProcess> = HashMap::new();
    for (pid, compute, bytes) in nvml {
        if *pid == 0 {
            continue;
        }
        let entry = map.entry(*pid).or_default();
        entry.pid = *pid;
        let source = if *compute { "nvmlCompute" } else { "nvmlGraphics" };
        if !entry.sources.contains(&source) {
            entry.sources.push(source);
        }
        entry.nvml_bytes = entry.nvml_bytes.max(*bytes);
    }
    for (pid, process) in &pdh.processes {
        if *pid == 0 {
            continue;
        }
        let entry = map.entry(*pid).or_default();
        entry.pid = *pid;
        if !entry.sources.contains(&"windowsPdh") {
            entry.sources.push("windowsPdh");
        }
        entry.engines = process.engines.clone();
        entry.pdh_dedicated = process.dedicated_bytes;
        entry.pdh_shared = process.shared_bytes;
    }
    let mut list: Vec<RawGpuProcess> = map.into_values().collect();
    list.sort_by_key(|process| process.pid);
    list
}

/// 표시 VRAM 출처 우선순위: PDH dedicated > NVML > N/A (합산 금지).
pub(crate) fn choose_vram(raw: &RawGpuProcess) -> (Option<u64>, Option<&'static str>) {
    match (raw.pdh_dedicated, raw.nvml_bytes) {
        (Some(bytes), _) if bytes > 0 => (Some(bytes), Some("windowsPdh")),
        (_, Some(bytes)) if bytes > 0 => (Some(bytes), Some("nvml")),
        (Some(0), None) => (Some(0), Some("windowsPdh")),
        _ => (None, None),
    }
}

// ---------------------------------------------------------------- resolver

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Resolution {
    pub display_name: String,
    pub classification: &'static str,
    pub runtime: Option<String>,
    pub models: Vec<String>,
    pub model_source: Option<String>,
    pub confidence: &'static str,
    pub managed: bool,
    pub service: Option<String>,
    pub service_kind: String,
    pub is_service_root: bool,
}

pub(crate) struct ResolveContext<'a> {
    pub owners: &'a HashMap<u32, (String, String, bool)>,
    pub ollama_models: &'a [String],
    pub laya_models: &'a [String],
    /// 살아 있는 런처 프로세스가 커맨드 라인에서 참조하는 실행 파일 이름(소문자).
    /// (예: GamesPlayAssist --startup-exe-file-path E:\Umamusume\umamusume.exe → game)
    pub launcher_exes: &'a HashSet<String>,
}

static EMPTY_LAUNCHERS: std::sync::OnceLock<HashSet<String>> = std::sync::OnceLock::new();

impl<'a> ResolveContext<'a> {
    pub fn new(
        owners: &'a HashMap<u32, (String, String, bool)>,
        ollama_models: &'a [String],
        laya_models: &'a [String],
    ) -> Self {
        Self {
            owners,
            ollama_models,
            laya_models,
            launcher_exes: EMPTY_LAUNCHERS.get_or_init(HashSet::new),
        }
    }
}

/// 증거 기반 분류. 순서는 결정적이며, 근거 없는 AI 판정을 하지 않는다.
pub(crate) fn resolve_process(
    meta: &ProcMeta,
    raw: &RawGpuProcess,
    ctx: &ResolveContext<'_>,
) -> Resolution {
    let exe = meta.base_name();
    let lower = exe.to_lowercase();
    let cmdline = meta.command_line.as_deref().unwrap_or("");
    let cmdline_lower = cmdline.to_lowercase();
    let engine = raw.headline().map(|(engine, _)| engine);
    let engine_suffix = engine_runtime_suffix(engine.as_deref());

    // 1) API Desk managed 서비스(최우선 근거)
    if let Some((label, id, is_root)) = ctx.owners.get(&raw.pid) {
        let mut models = Vec::new();
        let mut model_source = None;
        if id == "laya" && !ctx.laya_models.is_empty() {
            models = ctx.laya_models.to_vec();
            model_source = Some("laya_health".into());
        }
        let confidence = if !models.is_empty() { "exact" } else { "high" };
        return Resolution {
            display_name: label.clone(),
            classification: "ai",
            runtime: Some(label.clone()),
            models,
            model_source,
            confidence,
            managed: true,
            service: Some(label.clone()),
            service_kind: "managed_service".into(),
            is_service_root: *is_root,
        };
    }

    // 2) Ollama
    let ollama_cmdline = cmdline_lower.contains("ollama.exe")
        || cmdline_lower.contains("ollama app")
        || cmdline_lower.contains("ollama serve")
        || cmdline_lower.contains("\\ollama\\");
    if lower.contains("ollama") || ollama_cmdline {
        let models = ctx.ollama_models.to_vec();
        return Resolution {
            display_name: "Ollama".into(),
            classification: "ai",
            runtime: Some("Ollama".into()),
            models: models.clone(),
            model_source: (!models.is_empty()).then(|| "ollama_ps".into()),
            confidence: if models.is_empty() { "high" } else { "exact" },
            managed: false,
            service: Some("Ollama".into()),
            service_kind: "ollama".into(),
            is_service_root: false,
        };
    }

    // 3) Laya(비관리 프로세스로 발견된 경우)
    let laya_cmdline =
        cmdline_lower.contains("laya-serve") || cmdline_lower.contains("\\laya\\");
    if lower.contains("laya") || laya_cmdline {
        let models = ctx.laya_models.to_vec();
        return Resolution {
            display_name: "Laya".into(),
            classification: "ai",
            runtime: Some("Laya".into()),
            models: models.clone(),
            model_source: (!models.is_empty()).then(|| "laya_health".into()),
            confidence: "medium",
            managed: false,
            service: Some("Laya".into()),
            service_kind: "laya_app".into(),
            is_service_root: false,
        };
    }

    // 4) llama.cpp
    if lower.contains("llama-server") || lower.contains("llama-cli") || cmdline_lower.contains("llama.cpp") {
        let models: Vec<String> = model_from_cmdline(cmdline).into_iter().collect();
        return Resolution {
            display_name: "llama.cpp".into(),
            classification: "ai",
            runtime: Some("llama.cpp".into()),
            model_source: (!models.is_empty()).then(|| "cmdline".into()),
            models,
            confidence: "medium",
            managed: false,
            service: Some("llama.cpp".into()),
            service_kind: "llama_cpp".into(),
            is_service_root: false,
        };
    }

    // 5) vLLM
    if lower.contains("vllm") || cmdline_lower.contains("vllm") {
        let models: Vec<String> = model_from_cmdline(cmdline).into_iter().collect();
        return Resolution {
            display_name: "vLLM".into(),
            classification: "ai",
            runtime: Some("vLLM".into()),
            model_source: (!models.is_empty()).then(|| "cmdline".into()),
            models,
            confidence: "medium",
            managed: false,
            service: Some("vLLM".into()),
            service_kind: "vllm".into(),
            is_service_root: false,
        };
    }

    // 6) ComfyUI (체크포인트가 확실하지 않으면 모델은 비워 둔다)
    if lower.contains("comfyui") || cmdline_lower.contains("comfyui") {
        let models: Vec<String> = model_from_cmdline(cmdline).into_iter().collect();
        return Resolution {
            display_name: "ComfyUI".into(),
            classification: "ai",
            runtime: Some("ComfyUI".into()),
            model_source: (!models.is_empty()).then(|| "cmdline".into()),
            models,
            confidence: "medium",
            managed: false,
            service: Some("ComfyUI".into()),
            service_kind: "comfyui".into(),
            is_service_root: false,
        };
    }

    // 7) Python + CUDA/PyTorch (런타임은 알지만 애플리케이션/모델은 모름)
    if is_python_like(&exe) {
        let cuda_hint = CUDA_HINTS.iter().any(|hint| cmdline_lower.contains(hint));
        let compute_evidence = raw.sources.contains(&"nvmlCompute")
            || engine.as_deref().map(|value| value.eq_ignore_ascii_case("Compute")).unwrap_or(false);
        if cuda_hint || compute_evidence {
            let runtime = if cmdline_lower.contains("torch") {
                "PyTorch / CUDA"
            } else {
                "CUDA / Compute"
            };
            return Resolution {
                display_name: exe.clone(),
                classification: "ai",
                runtime: Some(runtime.into()),
                models: Vec::new(),
                model_source: None,
                confidence: if cuda_hint { "medium" } else { "low" },
                managed: false,
                service: None,
                service_kind: "python_cuda".into(),
                is_service_root: false,
            };
        }
    }

    // 8) 브라우저
    if BROWSER_EXES.contains(&lower.as_str()) {
        return Resolution {
            display_name: exe.clone(),
            classification: "browser",
            runtime: Some(format!("Browser / {engine_suffix}")),
            models: Vec::new(),
            model_source: None,
            confidence: "high",
            managed: false,
            service: None,
            service_kind: "browser".into(),
            is_service_root: false,
        };
    }

    // 9) 영상 앱
    if VIDEO_EXES.contains(&lower.as_str()) {
        return Resolution {
            display_name: exe.clone(),
            classification: "video",
            runtime: Some(format!("Video / {engine_suffix}")),
            models: Vec::new(),
            model_source: None,
            confidence: "high",
            managed: false,
            service: None,
            service_kind: "video".into(),
            is_service_root: false,
        };
    }

    // 10) 게임 런처 자식/감독 대상 → 게임
    if meta.ancestor_names.iter().any(|name| is_launcher(name))
        || ctx.launcher_exes.contains(&lower)
    {
        return Resolution {
            display_name: meta.display_name(),
            classification: "game",
            runtime: Some(format!("Game / {engine_suffix}")),
            models: Vec::new(),
            model_source: None,
            confidence: "high",
            managed: false,
            service: None,
            service_kind: "game".into(),
            is_service_root: false,
        };
    }

    // 11) 시스템 프로세스
    if SYSTEM_EXES.contains(&lower.as_str()) {
        let runtime = if lower == "dwm.exe" { "Windows Desktop" } else { "System" };
        return Resolution {
            display_name: exe.clone(),
            classification: "system",
            runtime: Some(runtime.into()),
            models: Vec::new(),
            model_source: None,
            confidence: "high",
            managed: false,
            service: None,
            service_kind: "system".into(),
            is_service_root: false,
        };
    }

    // 12) 증거 없는 폴백: 실행 파일 신원은 항상 우선한다.
    if exe.is_empty() {
        return Resolution {
            display_name: format!("PID {}", raw.pid),
            classification: "unknown",
            runtime: Some("Unknown GPU process".into()),
            models: Vec::new(),
            model_source: None,
            confidence: "low",
            managed: false,
            service: None,
            service_kind: "unknown".into(),
            is_service_root: false,
        };
    }
    let (classification, runtime) = if engine
        .as_deref()
        .map(|value| value.eq_ignore_ascii_case("3D"))
        .unwrap_or(false)
    {
        ("graphics", format!("Graphics / {engine_suffix}"))
    } else if engine
        .as_deref()
        .map(|value| value.eq_ignore_ascii_case("Compute"))
        .unwrap_or(false)
    {
        ("unknown", "GPU Compute".to_string())
    } else {
        ("unknown", "GPU application".to_string())
    };
    Resolution {
        display_name: if meta.version.file_description.is_some()
            || meta.version.product_name.is_some()
        {
            meta.display_name()
        } else {
            exe
        },
        classification,
        runtime: Some(runtime),
        models: Vec::new(),
        model_source: None,
        confidence: "low",
        managed: false,
        service: None,
        service_kind: "application".into(),
        is_service_root: false,
    }
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

fn snapshot_from_nvml() -> Option<(Vec<GpuInfo>, Vec<(u32, bool, Option<u64>)>)> {
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
        let mut processes: Vec<(u32, bool, Option<u64>)> = Vec::new();
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
            // compute/graphics 양쪽을 모두 모으고, 같은 PID는 병합 플래그로 남긴다.
            let mut seen: HashMap<u32, usize> = HashMap::new();
            for (function, compute) in [
                (nvml.compute_procs, true),
                (nvml.graphics_procs, false),
            ] {
                let Some(function) = function else {
                    continue;
                };
                for entry in nvml::running_processes(handle, function) {
                    let memory = if entry.used_gpu_memory == nvml::NVML_VALUE_NOT_AVAILABLE {
                        None
                    } else {
                        Some(entry.used_gpu_memory)
                    };
                    if let Some(index) = seen.get(&entry.pid).copied() {
                        let slot: &mut (u32, bool, Option<u64>) = &mut processes[index];
                        // 이미 compute로 등록된 PID가 graphics에도 있으면 VRAM은 큰 쪽 유지
                        slot.2 = slot.2.max(memory);
                        if !compute {
                            continue;
                        }
                        slot.1 = true;
                        continue;
                    }
                    seen.insert(entry.pid, processes.len());
                    processes.push((entry.pid, compute, memory));
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

/// managed 서비스 귀속 맵(pid → (라벨, id, 루트 여부)).
fn managed_owners(
    app: &AppHandle,
    services: &LocalServicesState,
    identities: &HashMap<u32, ProcIdentity>,
) -> HashMap<u32, (String, String, bool)> {
    let mut owner: HashMap<u32, (String, String, bool)> = HashMap::new();
    for (id, pid) in services.managed_pids() {
        let label = local_services::definitions()
            .into_iter()
            .find(|def| def.id == id)
            .map(|def| def.label)
            .unwrap_or(id.clone());
        owner.insert(pid, (label.clone(), id.clone(), true));
        for child in descendants_of(pid, identities) {
            owner.entry(child).or_insert((label.clone(), id.clone(), false));
        }
    }
    for (id, label, port) in services.service_ports(app) {
        if let Some(pid) = local_services::port_owner(port) {
            owner.entry(pid).or_insert((label, id, false));
        }
    }
    owner
}

/// 커맨드 라인에서 참조된 실행 파일 이름(소문자)을 뽑는다.
pub(crate) fn exe_names_in_cmdline(cmdline: &str) -> Vec<String> {
    cmdline
        .split_whitespace()
        .map(|token| base_name(token.trim_matches('"')).to_lowercase())
        .filter(|candidate| candidate.ends_with(".exe") && !candidate.contains('\\'))
        .collect()
}

/// 살아 있는 런처 프로세스들의 커맨드 라인에서 참조된 실행 파일 이름을 모은다.
/// (런처가 게임 프로세스를 직접 부모로 두지 않아도 감독 관계를 포착)
pub(crate) fn launcher_referenced_exes(
    identities: &HashMap<u32, ProcIdentity>,
    cache: &MetaCache,
) -> HashSet<String> {
    let mut set = HashSet::new();
    for (pid, identity) in identities {
        if !is_launcher(&identity.name) {
            continue;
        }
        let Some(cmdline) = cache.command_line(*pid) else {
            continue;
        };
        set.extend(exe_names_in_cmdline(&cmdline));
    }
    set
}

fn build_processes(
    app: &AppHandle,
    state: &GpuMonitorState,
    services: &LocalServicesState,
    raw_processes: &[RawGpuProcess],
) -> (Vec<GpuProcess>, usize) {
    let identities = state.meta.identities();
    let owners = managed_owners(app, services, &identities);

    let ollama = {
        let mut cache = state.ollama.lock().unwrap();
        let stale = cache
            .as_ref()
            .map(|(_, at)| at.elapsed().as_millis() as u64 > OLLAMA_TTL_MS)
            .unwrap_or(true);
        if stale {
            *cache = Some((ollama_models(), Instant::now()));
        }
        cache
            .as_ref()
            .map(|(models, _)| models.clone())
            .unwrap_or_default()
    };

    let laya_port = services
        .service_ports(app)
        .into_iter()
        .find(|(id, _, _)| id == "laya")
        .map(|(_, _, port)| port)
        .unwrap_or(8000);
    let laya_models = {
        let mut cache = state.health.lock().unwrap();
        let stale = cache
            .as_ref()
            .map(|(_, at)| at.elapsed().as_millis() as u64 > HEALTH_TTL_MS)
            .unwrap_or(true);
        if stale {
            let models = http_get_json("127.0.0.1", laya_port, "/health", 600)
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
        cache
            .as_ref()
            .map(|(models, _)| models.clone())
            .unwrap_or_default()
    };

    let launcher_exes = launcher_referenced_exes(&identities, &state.meta);
    let mut context = ResolveContext::new(&owners, &ollama, &laya_models);
    context.launcher_exes = &launcher_exes;

    let mut result = Vec::new();
    for raw in raw_processes {
        let meta = state.meta.resolve(raw.pid, &identities);
        let resolution = resolve_process(&meta, raw, &context);
        let (used_vram, vram_source) = choose_vram(raw);
        let headline = raw.headline();
        let (gpu_percent, dominant_engine) = match headline {
            Some((engine, percent)) => (Some(percent), Some(engine)),
            None => (None, None),
        };
        result.push(GpuProcess {
            pid: raw.pid,
            parent_pid: meta.parent,
            process_name: meta.base_name(),
            display_name: resolution.display_name,
            product_name: meta.version.product_name.clone(),
            executable: meta.exe_path.as_deref().map(base_name),
            classification: resolution.classification.to_string(),
            runtime: resolution.runtime,
            models: resolution.models,
            model_source: resolution.model_source,
            gpu_percent,
            dominant_engine,
            used_vram_bytes: used_vram,
            dedicated_vram_bytes: used_vram,
            shared_gpu_bytes: raw.pdh_shared,
            vram_source: vram_source.map(str::to_string),
            service: resolution.service,
            service_kind: resolution.service_kind,
            managed: resolution.managed,
            is_service_root: resolution.is_service_root,
            confidence: resolution.confidence.to_string(),
            sources: raw.sources.iter().map(|value| value.to_string()).collect(),
        });
    }

    // 정렬: GPU % 내림차순 → VRAM 내림차순 → PID.
    result.sort_by(|a, b| {
        b.gpu_percent
            .unwrap_or(-1.0)
            .partial_cmp(&a.gpu_percent.unwrap_or(-1.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.used_vram_bytes
                    .unwrap_or(0)
                    .cmp(&a.used_vram_bytes.unwrap_or(0))
            })
            .then_with(|| a.pid.cmp(&b.pid))
    });

    let other_count = result
        .iter()
        .filter(|process| process.gpu_percent.is_none() && process.used_vram_bytes.is_none())
        .count();
    (result, other_count)
}

/// 다른 백엔드 모듈(TURZX 등)이 동일 캐시를 재사용할 수 있게 노출한다.
/// (별도 NVML/nvidia-smi/PDH 폴링을 만들지 않기 위함)
pub(crate) fn cached_snapshot(app: &AppHandle, ttl_ms: u64) -> GpuSnapshot {
    let state = app.state::<GpuMonitorState>();
    {
        let cache = state.cache.lock().unwrap();
        if let Some((snapshot, at)) = cache.as_ref() {
            if at.elapsed().as_millis() as u64 <= ttl_ms {
                return snapshot.clone();
            }
        }
    }
    let snapshot = refresh_snapshot(app);
    let mut cache = state.cache.lock().unwrap();
    *cache = Some((snapshot.clone(), Instant::now()));
    snapshot
}

fn refresh_snapshot(app: &AppHandle) -> GpuSnapshot {
    let services = app.state::<LocalServicesState>();
    let monitor = app.state::<GpuMonitorState>();

    let nvml_data = snapshot_from_nvml();
    let (source, gpus, nvml_processes) = match nvml_data {
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

    let pdh = gpu_pdh::collect();
    let merged = merge_raw_processes(&nvml_processes, &pdh);
    let (processes, other_count) = build_processes(app, &monitor, &services, &merged);
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

/// 백엔드 상주 GPU 샘플러 상태(프런트엔드와 무관).
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct GpuRuntimeStatus {
    /// starting | ready | degraded | error
    pub state: String,
    pub last_sample_at: Option<String>,
    pub last_error: Option<String>,
    pub nvml_ready: bool,
    pub pdh_ready: bool,
    pub samples: u64,
}

pub struct GpuMonitorState {
    cache: Mutex<Option<(GpuSnapshot, Instant)>>,
    meta: MetaCache,
    ollama: Mutex<Option<(Vec<String>, Instant)>>,
    health: Mutex<Option<(Vec<String>, Instant)>>,
    runtime: Mutex<GpuRuntimeStatus>,
    sampler_running: std::sync::atomic::AtomicBool,
}

impl Default for GpuMonitorState {
    fn default() -> Self {
        Self {
            cache: Mutex::new(None),
            meta: MetaCache::default(),
            ollama: Mutex::new(None),
            health: Mutex::new(None),
            runtime: Mutex::new(GpuRuntimeStatus {
                state: "starting".into(),
                ..Default::default()
            }),
            sampler_running: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

/// 연속 실패 횟수 → 런타임 상태(순수, 테스트 가능).
pub fn runtime_state_name(consecutive_failures: u32) -> &'static str {
    match consecutive_failures {
        0 => "ready",
        1 | 2 => "degraded",
        _ => "error",
    }
}

fn set_runtime(app: &AppHandle, update: impl FnOnce(&mut GpuRuntimeStatus)) {
    if let Some(state) = app.try_state::<GpuMonitorState>() {
        if let Ok(mut status) = state.runtime.lock() {
            update(&mut status);
        }
    }
}

pub fn runtime_status(app: &AppHandle) -> GpuRuntimeStatus {
    app.try_state::<GpuMonitorState>()
        .and_then(|state| state.runtime.lock().ok().map(|status| status.clone()))
        .unwrap_or_default()
}

/// 백엔드 시작 시 GPU 샘플러를 소유/가동한다(창·프런트엔드 불필요).
pub fn start_sampler(app: &AppHandle) {
    let state = app.state::<GpuMonitorState>();
    if state
        .sampler_running
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    set_runtime(app, |status| {
        status.state = "starting".into();
        status.last_error = None;
    });
    crate::resident::log(app, "gpu", "sampler starting");
    let handle = app.clone();
    std::thread::spawn(move || sampler_loop(handle));
}

fn sampler_loop(app: AppHandle) {
    let mut consecutive_failures: u32 = 0;
    let mut first_pass = true;
    loop {
        let stop = app
            .try_state::<GpuMonitorState>()
            .map(|state| !state.sampler_running.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(true);
        if stop {
            break;
        }
        // cached_snapshot(app, 0) → 항상 새로 수집해 캐시에 게시한다.
        let snapshot = cached_snapshot(&app, 0);
        let now = now_iso();
        if snapshot.available {
            consecutive_failures = 0;
            let nvml_ready = snapshot.source == "nvml";
            let pdh_ready = gpu_pdh::last_rates_ready();
            set_runtime(&app, |status| {
                status.state = "ready".into();
                status.last_sample_at = Some(now.clone());
                status.last_error = None;
                status.nvml_ready = nvml_ready;
                status.pdh_ready = pdh_ready;
                status.samples = status.samples.saturating_add(1);
            });
            if first_pass {
                crate::resident::log(
                    &app,
                    "gpu",
                    &format!(
                        "nvml={} pdh_rates={} snapshot ready ({}개 프로세스)",
                        nvml_ready,
                        pdh_ready,
                        snapshot.processes.len()
                    ),
                );
            }
        } else {
            consecutive_failures += 1;
            let detail = snapshot
                .detail
                .clone()
                .or_else(|| snapshot.reason.clone())
                .unwrap_or_else(|| "GPU 정보를 사용할 수 없습니다".into());
            let state_name = runtime_state_name(consecutive_failures);
            set_runtime(&app, |status| {
                status.state = state_name.into();
                status.last_error = Some(detail.clone());
                status.nvml_ready = false;
                status.pdh_ready = gpu_pdh::last_rates_ready();
            });
            if first_pass || consecutive_failures == 3 {
                crate::resident::log(&app, "gpu", &format!("샘플 실패: {detail}"));
            }
        }
        first_pass = false;
        // 첫 샘플 직후에는 rate 카운터 확보를 위해 짧게, 이후 1초 주기.
        let sleep_ms = if consecutive_failures >= 3 { 5_000 } else { 1_000 };
        std::thread::sleep(Duration::from_millis(sleep_ms));
    }
}

#[tauri::command]
pub fn gpu_runtime_status(app: AppHandle) -> GpuRuntimeStatus {
    runtime_status(&app)
}

#[tauri::command]
pub fn gpu_restart_sampler(app: AppHandle) -> GpuRuntimeStatus {
    if let Some(state) = app.try_state::<GpuMonitorState>() {
        state
            .sampler_running
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
    std::thread::sleep(Duration::from_millis(120));
    start_sampler(&app);
    runtime_status(&app)
}

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
        let sampler_active = state
            .sampler_running
            .load(std::sync::atomic::Ordering::Relaxed);
        {
            let cache = state.cache.lock().unwrap();
            if let Some((snapshot, at)) = cache.as_ref() {
                let fresh = at.elapsed().as_millis() as u64 <= ttl;
                // 백엔드 샘플러가 돌고 있으면 캐시는 항상 최신(1초 주기)이다.
                if fresh || !visible || sampler_active {
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
    use crate::gpu_meta::VersionInfo;

    fn raw(pid: u32, sources: &[&'static str]) -> RawGpuProcess {
        RawGpuProcess {
            pid,
            sources: sources.to_vec(),
            ..Default::default()
        }
    }

    fn meta_with(
        name: &str,
        cmdline: Option<&str>,
        parent: Option<u32>,
        description: Option<&str>,
    ) -> ProcMeta {
        ProcMeta {
            name: name.to_string(),
            command_line: cmdline.map(str::to_string),
            ancestor_names: parent.map(|_| vec!["steamwebhelper.exe".to_string()]).unwrap_or_default(),
            parent,
            version: VersionInfo {
                file_description: description.map(str::to_string),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn empty_context() -> (HashMap<u32, (String, String, bool)>, Vec<String>, Vec<String>) {
        (HashMap::new(), Vec::new(), Vec::new())
    }

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
        let sanitized = sanitize_hint(hint);
        assert!(!sanitized.contains("sk-"));
        assert!(!sanitized.to_lowercase().contains("api-key"));
        assert!(sanitized.contains("qwen.gguf"));
    }

    #[test]
    fn classifies_tools() {
        assert_eq!(classify_tool("ollama.exe", None), ToolKind::Ollama);
        assert_eq!(classify_tool("llama-server.exe", None), ToolKind::LlamaCpp);
        assert_eq!(classify_tool("python.exe", Some("vllm serve m")), ToolKind::Vllm);
        assert_eq!(classify_tool("python.exe", Some("ComfyUI main.py")), ToolKind::ComfyUi);
        assert_eq!(classify_tool("python.exe", None), ToolKind::GenericPython);
        assert_eq!(classify_tool("game.exe", None), ToolKind::Other);
    }

    #[test]
    fn finds_descendant_tree() {
        let mut identities = HashMap::new();
        identities.insert(10, ProcIdentity { name: "laya-serve.exe".into(), parent: Some(1) });
        identities.insert(11, ProcIdentity { name: "python.exe".into(), parent: Some(10) });
        identities.insert(12, ProcIdentity { name: "worker.exe".into(), parent: Some(11) });
        identities.insert(13, ProcIdentity { name: "other.exe".into(), parent: Some(1) });
        let tree = descendants_of(10, &identities);
        assert!(tree.contains(&11));
        assert!(tree.contains(&12));
        assert!(!tree.contains(&13));
    }

    // §21 테스트: 병합/중복/출처
    #[test]
    fn merges_nvml_compute_and_graphics_same_pid_once() {
        let nvml = vec![(100u32, true, Some(1024u64)), (100, false, None)];
        let pdh = PdhSnapshot::default();
        let merged = merge_raw_processes(&nvml, &pdh);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].sources, vec!["nvmlCompute", "nvmlGraphics"]);
        assert_eq!(merged[0].nvml_bytes, Some(1024));
    }

    #[test]
    fn keeps_pdh_only_graphics_pid() {
        let mut pdh = PdhSnapshot {
            rates_ready: true,
            processes: HashMap::new(),
        };
        pdh.processes.insert(
            55,
            gpu_pdh::PdhProcess {
                engines: vec![("3D".into(), 44.0)],
                dedicated_bytes: Some(512),
                shared_bytes: Some(64),
                luid: 1,
            },
        );
        let merged = merge_raw_processes(&[], &pdh);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].sources, vec!["windowsPdh"]);
        assert_eq!(merged[0].headline(), Some(("3D".to_string(), 44.0)));
    }

    #[test]
    fn gpu_percent_is_max_engine_and_clamped() {
        let process = RawGpuProcess {
            pid: 1,
            sources: vec!["windowsPdh"],
            engines: vec![("3D".into(), 72.0), ("Copy".into(), 4.0), ("Compute".into(), 12.0)],
            ..Default::default()
        };
        assert_eq!(process.headline(), Some(("3D".to_string(), 72.0)));
        let over = RawGpuProcess {
            pid: 2,
            sources: vec!["windowsPdh"],
            engines: vec![("Compute".into(), 140.0)],
            ..Default::default()
        };
        assert_eq!(over.headline(), Some(("Compute".to_string(), 100.0)));
    }

    #[test]
    fn vram_prefers_pdh_and_never_sums() {
        let both = RawGpuProcess {
            pid: 1,
            pdh_dedicated: Some(300),
            nvml_bytes: Some(100),
            ..Default::default()
        };
        assert_eq!(choose_vram(&both), (Some(300), Some("windowsPdh")));
        let nvml_only = RawGpuProcess {
            pid: 2,
            nvml_bytes: Some(100),
            ..Default::default()
        };
        assert_eq!(choose_vram(&nvml_only), (Some(100), Some("nvml")));
        let zero_pdh = RawGpuProcess {
            pid: 3,
            pdh_dedicated: Some(0),
            nvml_bytes: Some(4096),
            ..Default::default()
        };
        assert_eq!(choose_vram(&zero_pdh), (Some(4096), Some("nvml")));
        let none = RawGpuProcess { pid: 4, ..Default::default() };
        assert_eq!(choose_vram(&none), (None, None));
    }

    // §21: 리졸버
    #[test]
    fn resolves_managed_service_descendant() {
        let (mut owners, _ollama, laya) = empty_context();
        owners.insert(200, ("Laya".into(), "laya".into(), false));
        let context = ResolveContext::new(&owners, &[], &laya);
        let meta = meta_with("python.exe", None, Some(199), None);
        let resolution = resolve_process(&meta, &raw(200, &["nvmlCompute"]), &context);
        assert_eq!(resolution.display_name, "Laya");
        assert_eq!(resolution.classification, "ai");
        assert_eq!(resolution.confidence, "high");
        assert!(resolution.managed);
    }

    #[test]
    fn resolves_managed_laya_with_health_models_as_exact() {
        let (mut owners, _ollama, _laya) = empty_context();
        owners.insert(300, ("Laya".into(), "laya".into(), true));
        let laya = vec!["english".to_string(), "multilingual".to_string()];
        let context = ResolveContext::new(&owners, &[], &laya);
        let meta = meta_with("laya-serve.exe", None, None, None);
        let resolution = resolve_process(&meta, &raw(300, &[]), &context);
        assert_eq!(resolution.confidence, "exact");
        assert_eq!(resolution.models, laya);
        assert_eq!(resolution.model_source.as_deref(), Some("laya_health"));
    }

    #[test]
    fn maps_ollama_process_to_loaded_model() {
        let (owners, _o, laya) = empty_context();
        let ollama = vec!["qwen3.5:9b".to_string()];
        let context = ResolveContext::new(&owners, &ollama, &laya);
        let meta = meta_with("ollama.exe", Some("ollama serve"), None, None);
        let resolution = resolve_process(&meta, &raw(10, &["nvmlCompute"]), &context);
        assert_eq!(resolution.display_name, "Ollama");
        assert_eq!(resolution.models, vec!["qwen3.5:9b".to_string()]);
        assert_eq!(resolution.confidence, "exact");
    }

    #[test]
    fn detects_comfyui_without_guessing_model() {
        let (owners, _o, laya) = empty_context();
        let context = ResolveContext::new(&owners, &[], &laya);
        let meta = meta_with(
            "python.exe",
            Some("python.exe C:\\ComfyUI\\main.py --listen"),
            None,
            None,
        );
        let resolution = resolve_process(&meta, &raw(20, &["nvmlCompute"]), &context);
        assert_eq!(resolution.display_name, "ComfyUI");
        assert!(resolution.models.is_empty(), "체크포인트 근거 없으면 모델 비움");
        assert_eq!(resolution.runtime.as_deref(), Some("ComfyUI"));
    }

    #[test]
    fn generic_python_cuda_is_not_unknown_ai() {
        let (owners, _o, laya) = empty_context();
        let context = ResolveContext::new(&owners, &[], &laya);
        let meta = meta_with("python.exe", Some("python train.py --epochs 3"), None, None);
        let mut raw_process = raw(30, &["nvmlCompute"]);
        raw_process.engines = vec![("Compute".into(), 55.0)];
        let resolution = resolve_process(&meta, &raw_process, &context);
        assert_eq!(resolution.classification, "ai");
        assert_eq!(resolution.runtime.as_deref(), Some("CUDA / Compute"));
        assert!(resolution.models.is_empty());
        assert_ne!(resolution.display_name, "Unknown AI workload");
        assert_eq!(resolution.confidence, "low");
    }

    #[test]
    fn pytorch_hint_raises_confidence() {
        let (owners, _o, laya) = empty_context();
        let context = ResolveContext::new(&owners, &[], &laya);
        let meta = meta_with(
            "python.exe",
            Some("python sd.py --use-torch --safetensors-checkpoint"),
            None,
            None,
        );
        let resolution = resolve_process(&meta, &raw(31, &["nvmlCompute"]), &context);
        assert_eq!(resolution.runtime.as_deref(), Some("PyTorch / CUDA"));
        assert_eq!(resolution.confidence, "medium");
    }

    #[test]
    fn classifies_browser_with_engine() {
        let (owners, _o, laya) = empty_context();
        let context = ResolveContext::new(&owners, &[], &laya);
        let meta = meta_with("msedge.exe", None, None, None);
        let mut raw_process = raw(40, &["windowsPdh"]);
        raw_process.engines = vec![("3D".into(), 12.0), ("VideoDecode".into(), 3.0)];
        let resolution = resolve_process(&meta, &raw_process, &context);
        assert_eq!(resolution.classification, "browser");
        assert_eq!(resolution.runtime.as_deref(), Some("Browser / 3D"));
    }

    #[test]
    fn classifies_unknown_3d_app_as_graphics_not_ai() {
        let (owners, _o, laya) = empty_context();
        let context = ResolveContext::new(&owners, &[], &laya);
        let meta = meta_with("eldenring.exe", None, None, None);
        let mut raw_process = raw(50, &["nvmlGraphics"]);
        raw_process.engines = vec![("3D".into(), 78.0)];
        let resolution = resolve_process(&meta, &raw_process, &context);
        assert_eq!(resolution.classification, "graphics");
        assert_eq!(resolution.display_name, "eldenring.exe");
        assert_eq!(resolution.runtime.as_deref(), Some("Graphics / 3D"));
        assert_ne!(resolution.classification, "ai");
    }

    #[test]
    fn classifies_launcher_game_with_product_name() {
        let (mut owners, _o, laya) = empty_context();
        owners.insert(60, ("steam.exe".into(), "steam".into(), false));
        let context = ResolveContext::new(&owners, &[], &laya);
        let mut meta = meta_with(
            "granblue.exe",
            None,
            Some(60),
            Some("Granblue Fantasy: Relink"),
        );
        meta.ancestor_names = vec!["steam.exe".into()];
        let mut raw_process = raw(61, &["windowsPdh"]);
        raw_process.engines = vec![("3D".into(), 71.0)];
        let resolution = resolve_process(&meta, &raw_process, &context);
        assert_eq!(resolution.classification, "game");
        assert_eq!(resolution.display_name, "Granblue Fantasy: Relink");
        assert_eq!(resolution.runtime.as_deref(), Some("Game / 3D"));
    }

    #[test]
    fn non_launcher_parent_is_not_a_game() {
        let (owners, _o, laya) = empty_context();
        let context = ResolveContext::new(&owners, &[], &laya);
        let mut meta = meta_with("eldenring.exe", None, Some(4), None);
        meta.ancestor_names = vec!["explorer.exe".into()];
        let mut raw_process = raw(51, &["nlvmGraphics"]);
        raw_process.engines = vec![("3D".into(), 60.0)];
        let resolution = resolve_process(&meta, &raw_process, &context);
        assert_eq!(resolution.classification, "graphics");
    }

    #[test]
    fn extracts_launcher_referenced_exe_names() {
        let cmdline = "\"C:\\Program Files\\DMMGamePlayer\\resources\\GamesPlayAssist.exe\" --window-name umamusume --startup-exe-file-path \"E:\\Umamusume\\umamusume.exe\" --product-id 123";
        let names = exe_names_in_cmdline(cmdline);
        assert!(names.contains(&"gamesplayassist.exe".to_string()));
        assert!(names.contains(&"umamusume.exe".to_string()));
        let resolution = {
            let owners = HashMap::new();
            let mut launcher_exes = HashSet::new();
            launcher_exes.insert("umamusume.exe".to_string());
            let context = ResolveContext {
                owners: &owners,
                ollama_models: &[],
                laya_models: &[],
                launcher_exes: &launcher_exes,
            };
            let meta = meta_with("umamusume.exe", None, None, None);
            let mut raw_process = raw(52, &["windowsPdh"]);
            raw_process.engines = vec![("3D".into(), 55.0)];
            resolve_process(&meta, &raw_process, &context)
        };
        assert_eq!(resolution.classification, "game");
        assert_eq!(resolution.runtime.as_deref(), Some("Game / 3D"));
    }

    #[test]
    fn classifies_dwm_as_system() {
        let (owners, _o, laya) = empty_context();
        let context = ResolveContext::new(&owners, &[], &laya);
        let meta = meta_with("dwm.exe", None, None, None);
        let resolution = resolve_process(&meta, &raw(70, &["windowsPdh"]), &context);
        assert_eq!(resolution.classification, "system");
        assert_eq!(resolution.runtime.as_deref(), Some("Windows Desktop"));
    }

    #[test]
    fn inaccessible_metadata_still_reports_pid() {
        let (owners, _o, laya) = empty_context();
        let context = ResolveContext::new(&owners, &[], &laya);
        // 이름/경로/커맨드 라인을 모두 못 읽은 경우(권한 제한)
        let meta = ProcMeta::default();
        let resolution = resolve_process(&meta, &raw(999, &["windowsPdh"]), &context);
        assert_eq!(resolution.display_name, "PID 999");
        assert_eq!(resolution.runtime.as_deref(), Some("Unknown GPU process"));
        assert_eq!(resolution.classification, "unknown");
    }

    #[test]
    fn exited_or_unknown_pid_is_skipped_from_merger() {
        let nvml = vec![(0u32, true, Some(1u64))];
        let merged = merge_raw_processes(&nvml, &PdhSnapshot::default());
        assert!(merged.is_empty(), "PID 0은 GPU 프로세스가 아니다");
    }

    #[test]
    fn dto_serialization_hides_command_line() {
        let process = GpuProcess {
            pid: 1,
            process_name: "python.exe".into(),
            display_name: "python.exe".into(),
            runtime: Some("PyTorch / CUDA".into()),
            sources: vec!["nvmlCompute".into(), "windowsPdh".into()],
            ..Default::default()
        };
        let text = serde_json::to_string(&process).unwrap();
        assert!(!text.contains("commandLine"));
        assert!(!text.contains("command_line"));
        assert!(!text.contains("--api-key"));
        // 내부 전용 필드가 실수로 추가되지 않도록 키 집합을 고정한다.
        for forbidden in ["cmd", "arguments", "environment"] {
            assert!(!text.to_lowercase().contains(forbidden));
        }
    }

    #[test]
    fn snapshot_serializes_camel_case_with_na() {
        let snapshot = GpuSnapshot {
            available: true,
            source: "nvml".into(),
            gpus: vec![GpuInfo {
                index: 0,
                name: "NVIDIA GeForce RTX 5090".into(),
                memory_total_bytes: Some(34_000),
                memory_used_bytes: Some(1_000),
                ..Default::default()
            }],
            processes: vec![GpuProcess {
                pid: 42,
                process_name: "python.exe".into(),
                display_name: "python.exe".into(),
                classification: "ai".into(),
                runtime: Some("CUDA / Compute".into()),
                used_vram_bytes: None,
                ..Default::default()
            }],
            other_count: 0,
            ..Default::default()
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["available"], true);
        assert_eq!(value["gpus"][0]["memoryUsedBytes"], 1000);
        assert_eq!(value["processes"][0]["usedVramBytes"], Value::Null);
        assert_eq!(value["processes"][0]["classification"], "ai");
        assert_eq!(value["processes"][0]["runtime"], "CUDA / Compute");
        assert!(value["processes"][0].get("commandLine").is_none());
    }

    #[test]
    fn derives_runtime_state_from_failures() {
        assert_eq!(runtime_state_name(0), "ready");
        assert_eq!(runtime_state_name(1), "degraded");
        assert_eq!(runtime_state_name(2), "degraded");
        assert_eq!(runtime_state_name(3), "error");
        assert_eq!(runtime_state_name(9), "error");
    }

    #[test]
    fn unavailable_snapshot_is_explicit() {
        let snapshot = GpuSnapshot {
            available: false,
            reason: Some("nvml_unavailable".into()),
            source: "none".into(),
            detail: Some("GPU 없음".into()),
            ..Default::default()
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["available"], false);
        assert_eq!(value["reason"], "nvml_unavailable");
        assert!(value["processes"].as_array().unwrap().is_empty());
    }

    #[test]
    fn sorts_by_gpu_percent_then_vram() {
        let mut processes = vec![
            (1u32, Some(10.0f64), Some(9000u64)),
            (2, Some(80.0), Some(100)),
            (3, Some(80.0), Some(5000)),
            (4, None, Some(100_000)),
        ];
        processes.sort_by(|a, b| {
            b.1.unwrap_or(-1.0)
                .partial_cmp(&a.1.unwrap_or(-1.0))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.2.unwrap_or(0).cmp(&a.2.unwrap_or(0)))
                .then_with(|| a.0.cmp(&b.0))
        });
        let order: Vec<u32> = processes.into_iter().map(|entry| entry.0).collect();
        assert_eq!(order, vec![3, 2, 1, 4], "GPU% → VRAM → PID 순서");
    }
}

#[cfg(windows)]
trait HiddenCommand {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}

#[cfg(windows)]
impl HiddenCommand for Command {
    fn creation_flags_hidden(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(0x0800_0000)
    }
}

#[cfg(not(windows))]
trait HiddenCommand {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}

#[cfg(not(windows))]
impl HiddenCommand for Command {
    fn creation_flags_hidden(&mut self) -> &mut Self {
        self
    }
}
