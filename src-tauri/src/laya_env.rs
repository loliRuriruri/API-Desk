//! Laya 환경 매니저: 버전 확인, 환경 진단, GPU 후보 환경 생성/검증/활성화/롤백.
//!
//! 안전 원칙:
//! - 기존 production 환경(C:\Laya\.venv)은 절대 변경하지 않는다(롤백용 보존).
//! - 모든 설치는 "새 후보 환경"에서만 수행하고, 검증 통과 후에만 활성화한다.
//! - 규칙은 셸 문자열이 아니라 `Command::new(executable).args([...])`로만 실행한다.
//! - 로그/에러에 시크릿(토큰/키)을 남기지 않는다.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::AppError;
use crate::local_services;

pub const DIAG_SENTINEL: &str = "__LAYA_DIAG__";
const VERSION_TTL_SECS: i64 = 24 * 60 * 60;
const ENVS_DIR: &str = "envs";
const WRAPPER_FILE: &str = "laya_health.py";
const ENV_STATE_FILE: &str = "laya-env.json";
const MANAGER_STATE_FILE: &str = "laya-env-manager.json";

/// PyTorch CUDA 휠 인덱스 후보(호환성 레이어). 앞에서부터 시도한다.
const TORCH_CUDA_INDEXES: [(&str, &str); 3] = [
    ("cu130", "https://download.pytorch.org/whl/cu130"),
    ("cu128", "https://download.pytorch.org/whl/cu128"),
    ("cu126", "https://download.pytorch.org/whl/cu126"),
];

const DEFAULT_MODELS: [&str; 3] = ["english", "multilingual", "typed-decisions"];

// ---------------------------------------------------------------- DTO

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LayaVersionCheck {
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub update_available: bool,
    pub source: String,
    pub error: Option<String>,
    pub checked_at: Option<String>,
    pub from_cache: bool,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LayaEnvironmentStatus {
    pub python_path: String,
    pub python_version: Option<String>,
    pub laya_version: Option<String>,
    pub latest_laya_version: Option<String>,
    pub update_available: bool,
    pub version_check_error: Option<String>,
    pub torch_version: Option<String>,
    pub torch_cuda_version: Option<String>,
    pub cuda_available: bool,
    pub gpu_name: Option<String>,
    pub compute_capability: Option<String>,
    pub tilelang_installed: bool,
    pub tilelang_version: Option<String>,
    pub fast_path_supported: bool,
    pub fast_path_active: Option<bool>,
    pub effective_device: Option<String>,
    pub backend: Option<String>,
    pub environment_kind: String,
    pub environment_version: Option<String>,
    pub service_running: bool,
    pub service_pid: Option<u32>,
    pub health: Option<Value>,
    pub problems: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LayaEnvEntry {
    pub path: String,
    pub version: Option<String>,
    pub kind: String,
    pub state: String,
    pub created_at: Option<String>,
    pub metrics: Option<Value>,
    pub active: bool,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct EnvStateFile {
    state: String,
    version: Option<String>,
    kind: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    #[serde(default)]
    torch_cuda_version: Option<String>,
    #[serde(default)]
    gpu_name: Option<String>,
    #[serde(default)]
    tilelang: Option<bool>,
    #[serde(default)]
    fast_path: Option<bool>,
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    metrics: Option<Value>,
    #[serde(default)]
    previous_env: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct ManagerStateFile {
    active_env: Option<String>,
    previous_env: Option<String>,
    #[serde(default)]
    version_cache: HashMap<String, i64>,
    #[serde(default)]
    latest_version: Option<String>,
    #[serde(default)]
    notified_version: Option<String>,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LayaBenchmark {
    pub python_path: String,
    pub device: Option<String>,
    pub backend: Option<String>,
    pub laya_version: Option<String>,
    pub torch_version: Option<String>,
    pub cold_load_ms: Option<u64>,
    pub warm_p50_ms: Option<f64>,
    pub batch_p50_ms: Option<f64>,
    pub iterations: u32,
    pub loaded_models: Vec<String>,
    pub vram_used_bytes: Option<u64>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct LayaEnvState {
    cancel: AtomicBool,
    running: AtomicBool,
    last_error: Mutex<Option<String>>,
}

impl LayaEnvState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------- 진단 스크립트

const DIAGNOSTIC_SCRIPT: &str = r#"
import json, sys, importlib.metadata as md
out = {"pythonVersion": sys.version.split()[0]}
try:
    out["layaVersion"] = md.version("laya")
except Exception:
    out["layaVersion"] = None
try:
    import torch
    out["torchVersion"] = torch.__version__
    out["torchCudaVersion"] = torch.version.cuda
    cuda = bool(torch.cuda.is_available())
    out["cudaAvailable"] = cuda
    if cuda:
        out["gpuName"] = torch.cuda.get_device_name(0)
        cap = torch.cuda.get_device_capability(0)
        out["computeCapability"] = "%d.%d" % (cap[0], cap[1])
    out["effectiveDevice"] = "cuda" if cuda else "cpu"
except Exception as exc:
    out["cudaAvailable"] = False
    out["torchVersion"] = "error:%s" % type(exc).__name__
try:
    import tilelang
    out["tilelangInstalled"] = True
    out["tilelangVersion"] = getattr(tilelang, "__version__", None)
except Exception:
    out["tilelangInstalled"] = False
try:
    import laya.fast  # noqa: F401
    out["fastImportable"] = True
except Exception:
    out["fastImportable"] = False
print("__LAYA_DIAG__" + json.dumps(out))
"#;

/// 후보 환경 검증: CUDA가 실제로 사용 가능해야 한다(아니면 비0 종료).
const CUDA_VERIFY_SCRIPT: &str = r#"
import json, sys
import torch
info = {
    "torch": torch.__version__,
    "cudaVersion": torch.version.cuda,
    "cudaAvailable": bool(torch.cuda.is_available()),
}
if info["cudaAvailable"]:
    info["gpu"] = torch.cuda.get_device_name(0)
    cap = torch.cuda.get_device_capability(0)
    info["capability"] = "%d.%d" % (cap[0], cap[1])
print("__LAYA_DIAG__" + json.dumps(info))
if not info["cudaAvailable"]:
    sys.exit(2)
"#;

/// 스모크: 실제 체크포인트 로드 + 추론 + fast-path 상태 확인.
const SMOKE_SCRIPT: &str = r#"
import os
os.environ.setdefault("NVCC_APPEND_FLAGS", "-allow-unsupported-compiler")
import json, os, sys, time
from laya.router import Router
names = [m for m in (os.environ.get("LAYA_SMOKE_MODELS", "english,multilingual,typed-decisions")).split(",") if m]
device = os.environ.get("LAYA_DEVICE") or "cuda"
fast = (os.environ.get("LAYA_FAST", "1").lower() not in ("0", "false", "no"))
import laya.agent as agent_mod
Base = agent_mod.Agent
class SmokeAgent(Base):
    def __init__(self, *args, **kwargs):
        kwargs.setdefault("fast", fast)
        try:
            super().__init__(*args, **kwargs)
        except Exception:
            kwargs["fast"] = False
            super().__init__(*args, **kwargs)
agent_mod.Agent = SmokeAgent
t0 = time.time()
router = Router(device=device, max_loaded=4)
router.preload(names)
load_ms = int((time.time() - t0) * 1000)
question = {"intent": {"type": "choice", "instructions": "Pick the action.",
                       "criteria": {"cancel": "cancel the subscription", "keep": "keep the subscription"}}}
t0 = time.time()
result = router.predict("I want to cancel my subscription.", question, model=names[0])
warm_ms = int((time.time() - t0) * 1000)
agents = {name: router.load(name) for name in names}
devices = sorted({getattr(a.device, "type", "unknown") for a in agents.values()})
fast_active = any(getattr(a, "_fast", None) is not None for a in agents.values())
out = {"loaded": router.loaded, "loadMs": load_ms, "warmMs": warm_ms,
       "devices": devices, "fastPath": fast_active,
       "answerKeys": sorted(list(result.get("answers", {}).keys()))}
print("__LAYA_DIAG__" + json.dumps(out))
"#;

/// 벤치마크: 콜드 로드 + 웜 단일/배치 p50 (측정값만 출력).
const BENCH_SCRIPT: &str = r#"
import json, os, statistics, sys, time
os.environ.setdefault("NVCC_APPEND_FLAGS", "-allow-unsupported-compiler")
os.environ.setdefault("LAYA_DEVICE", "cuda")
from laya.router import Router
fast = (os.environ.get("LAYA_FAST", "1").lower() not in ("0", "false", "no"))
import laya.agent as agent_mod
Base = agent_mod.Agent
class BenchAgent(Base):
    def __init__(self, *args, **kwargs):
        kwargs.setdefault("fast", fast)
        try:
            super().__init__(*args, **kwargs)
        except Exception:
            kwargs["fast"] = False
            super().__init__(*args, **kwargs)
agent_mod.Agent = BenchAgent
iterations = int(os.environ.get("LAYA_BENCH_ITERS", "5"))
names = [m for m in (os.environ.get("LAYA_BENCH_MODELS", "english")).split(",") if m]
t0 = time.time()
router = Router(device=os.environ.get("LAYA_DEVICE") or None, max_loaded=4)
router.preload(names)
cold_ms = int((time.time() - t0) * 1000)
question = {"intent": {"type": "choice", "instructions": "Pick the action.",
                       "criteria": {"cancel": "cancel the subscription", "keep": "keep the subscription"}}}
state = "I want to cancel my subscription."
warm = []
for _ in range(iterations):
    t0 = time.time()
    router.predict(state, question, model=names[0])
    warm.append((time.time() - t0) * 1000)
batch = []
batch_requests = [{"state": state, "questions": question, "model": names[0]} for _ in range(3)]
for _ in range(max(2, iterations // 2)):
    t0 = time.time()
    router.predict_batch(batch_requests)
    batch.append((time.time() - t0) * 1000)
agents = {name: router.load(name) for name in names}
devices = sorted({getattr(a.device, "type", "unknown") for a in agents.values()})
fast_active = any(getattr(a, "_fast", None) is not None for a in agents.values())
vram = None
try:
    import torch
    if torch.cuda.is_available():
        vram = int(torch.cuda.memory_allocated(0))
except Exception:
    vram = None
import laya
out = {"device": ",".join(devices), "fastPath": fast_active,
       "layaVersion": getattr(laya, "__version__", None),
       "coldLoadMs": cold_ms, "warmP50Ms": statistics.median(warm) if warm else None,
       "batchP50Ms": statistics.median(batch) if batch else None,
       "iterations": iterations, "loaded": router.loaded, "vramAllocatedBytes": vram}
print("__LAYA_DIAG__" + json.dumps(out))
"#;

/// API Desk 전용 Laya 서버 래퍼: /health가 "실제 유효 상태"를 보고한다.
const WRAPPER_SCRIPT: &str = r#"# API Desk Laya health-contract wrapper.
# laya-serve 대신 실행되어 /health에 실제 effective device/backend/fallback 상태를 노출한다.
import json
import os
import threading
import time

STATE = {"ready": False, "reason": None, "warmup": None, "cpuFallbackCount": 0}


import os as _os
_os.environ.setdefault("NVCC_APPEND_FLAGS", "-allow-unsupported-compiler")


def _env_flag(name, default=True):
    raw = os.environ.get(name)
    if raw is None:
        return default
    return raw.strip().lower() not in ("0", "false", "no", "off")


def _build_router():
    import laya.agent as agent_mod
    from laya.router import Router

    want_fast = _env_flag("LAYA_FAST", True)
    base_agent = agent_mod.Agent

    class HealthAgent(base_agent):
        def __init__(self, *args, **kwargs):
            kwargs.setdefault("fast", want_fast)
            try:
                super().__init__(*args, **kwargs)
            except Exception:
                kwargs["fast"] = False
                super().__init__(*args, **kwargs)

    agent_mod.Agent = HealthAgent
    device = os.environ.get("LAYA_DEVICE") or None
    models = [m.strip() for m in os.environ.get("LAYA_MODELS", "").split(",") if m.strip()] or None
    router = Router(device=device, max_loaded=4)
    if _env_flag("LAYA_PRELOAD", True):
        router.preload(models)
    return router


def _contract(router):
    loaded = list(getattr(router, "loaded", []) or [])
    devices = set()
    fast_active = False
    for name in loaded:
        try:
            agent = router.load(name)
        except Exception:
            continue
        devices.add(str(getattr(getattr(agent, "device", None), "type", "unknown")))
        if getattr(agent, "_fast", None) is not None:
            fast_active = True
    effective = "cuda" if "cuda" in devices else ("cpu" if devices else None)
    gpu = None
    if effective == "cuda":
        try:
            import torch
            gpu = torch.cuda.get_device_name(0)
        except Exception:
            gpu = None
    backend = "tilelang" if (fast_active and effective == "cuda") else ("torch" if effective else "cpu")
    requested = os.environ.get("LAYA_DEVICE") or "auto"
    version = None
    try:
        import importlib.metadata as md
        version = md.version("laya")
    except Exception:
        version = None
    reason = STATE["reason"]
    if STATE["ready"] and reason is None and backend == "cpu" and requested in ("cuda", "auto"):
        reason = "cuda unavailable"
    return {
        "status": "ok",
        "ready": bool(STATE["ready"]),
        "laya_version": version,
        "device_requested": requested,
        "device_effective": effective or "unknown",
        "gpu": gpu,
        "backend": backend,
        "fast_path": bool(fast_active),
        "loaded": loaded,
        "cpu_fallback_count": int(STATE["cpuFallbackCount"]),
        "fallback_reason": reason,
        "warmup": STATE["warmup"],
    }


def _warmup(router):
    try:
        question = {"intent": {"type": "choice", "instructions": "Pick the action.",
                               "criteria": {"cancel": "cancel the subscription",
                                            "keep": "keep the subscription"}}}
        total = 0.0
        for name in list(getattr(router, "loaded", []) or []):
            started = time.time()
            router.predict("I want to cancel my subscription.", question, model=name)
            total += (time.time() - started) * 1000.0
        STATE["warmup"] = {"totalMs": int(total), "models": len(getattr(router, "loaded", []) or [])}
        # fallback 집계: cuda 요청인데 실제 cpu로 떨어진 에이전트 수
        requested = os.environ.get("LAYA_DEVICE") or "auto"
        fallbacks = 0
        for name in list(getattr(router, "loaded", []) or []):
            try:
                agent = router.load(name)
                if requested == "cuda" and str(getattr(getattr(agent, "device", None), "type", "")) == "cpu":
                    fallbacks += 1
                    STATE["reason"] = STATE["reason"] or "cuda placement unavailable for %s" % name
            except Exception:
                fallbacks += 1
        STATE["cpuFallbackCount"] = fallbacks
        if STATE["reason"] is None and fallbacks == 0:
            # fast path가 아니어도 stock CUDA면 사유를 남긴다
            fast_any = any(
                getattr(router.load(name), "_fast", None) is not None
                for name in (getattr(router, "loaded", []) or [])
            )
            if requested == "cuda" and not fast_any:
                STATE["reason"] = "tilelang fast path unavailable; using stock CUDA"
        STATE["ready"] = True
    except Exception as exc:  # warm-up 실패는 서비스를 죽이지 않는다
        STATE["warmup"] = {"error": type(exc).__name__}
        STATE["reason"] = STATE["reason"] or ("warmup failed: %s" % type(exc).__name__)
        STATE["ready"] = True


def main():
    router = _build_router()
    from laya.serve import create_app
    app = create_app(router=router)
    app.router.routes = [r for r in app.router.routes if getattr(r, "path", None) != "/health"]

    @app.get("/health")
    def health():
        return _contract(router)

    threading.Thread(target=_warmup, args=(router,), daemon=True).start()
    import uvicorn
    uvicorn.run(
        app,
        host=os.environ.get("LAYA_HOST", "0.0.0.0"),
        port=int(os.environ.get("LAYA_PORT", "8000")),
        log_level=os.environ.get("LAYA_LOG_LEVEL", "info"),
    )


if __name__ == "__main__":
    main()
"#;

// ---------------------------------------------------------------- 유틸

pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut skip_next = false;
    for token in text.split_whitespace() {
        let lower = token.to_lowercase();
        if skip_next {
            skip_next = false;
            out.push_str("[redacted]");
            out.push(' ');
            continue;
        }
        if lower == "bearer" || lower == "authorization:" || lower == "token" || lower == "token=" {
            skip_next = true;
            out.push_str("[redacted]");
        } else if lower.starts_with("hf_")
            || lower.starts_with("sk-")
            || lower.starts_with("ghp_")
            || lower.starts_with("bearer")
            || lower.starts_with("authorization:")
        {
            out.push_str("[redacted]");
        } else {
            out.push_str(token);
        }
        out.push(' ');
    }
    out.trim_end().chars().take(400).collect()
}

fn run_hidden(program: &str, args: &[&str]) -> Result<std::process::Output, AppError> {
    Command::new(program)
        .args(args)
        .creation_flags_hidden()
        .stdin(Stdio::null())
        .output()
        .map_err(|error| AppError::Io(format!("{program} 실행 실패: {}", sanitize(&error.to_string()))))
}

fn parse_sentinel(stdout: &str) -> Option<Value> {
    stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(DIAG_SENTINEL))
        .and_then(|payload| serde_json::from_str::<Value>(payload).ok())
}

// ---------------------------------------------------------------- 버전 관리

/// "0.3.21" 형태 비교. 숫자가 아니면 문자열 비교로 폴백.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |text: &str| -> Vec<i64> {
        text.split(['.', '-', '+'])
            .map(|part| {
                part.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
            })
            .map(|digits| digits.parse::<i64>().unwrap_or(0))
            .collect()
    };
    let (left, right) = (parse(a), parse(b));
    for index in 0..left.len().max(right.len()) {
        let l = left.get(index).copied().unwrap_or(0);
        let r = right.get(index).copied().unwrap_or(0);
        if l != r {
            return l.cmp(&r);
        }
    }
    a.cmp(b)
}

/// PyPI JSON에서 stable 최신 버전을 추출한다.
pub fn parse_pypi_latest(body: &Value) -> Option<String> {
    let version = body
        .get("info")
        .and_then(|info| info.get("version"))
        .and_then(Value::as_str)?;
    if version.trim().is_empty() {
        return None;
    }
    // pre-release(rc/dev/b)는 stable 채널에서 제외한다.
    let lower = version.to_lowercase();
    let prerelease = lower.contains("rc") || lower.contains("dev") || lower.contains('b');
    if prerelease {
        return None;
    }
    Some(version.to_string())
}

pub fn version_check(installed: Option<&str>, latest: Option<&str>) -> (bool, Option<String>) {
    match (installed, latest) {
        (Some(current), Some(remote)) => {
            let order = compare_versions(current, remote);
            (order == std::cmp::Ordering::Less, None)
        }
        (Some(_), None) => (false, Some("최신 버전을 확인할 수 없습니다".into())),
        (None, _) => (false, None),
    }
}

fn manager_state_path(app: &AppHandle) -> Result<PathBuf, AppError> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|error| AppError::Io(error.to_string()))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(MANAGER_STATE_FILE))
}

fn read_manager_state(app: &AppHandle) -> ManagerStateFile {
    manager_state_path(app)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(text.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

fn write_manager_state(app: &AppHandle, state: &ManagerStateFile) -> Result<(), AppError> {
    let path = manager_state_path(app)?;
    let text = serde_json::to_string_pretty(state).map_err(|error| AppError::Io(error.to_string()))?;
    std::fs::write(path, text)?;
    Ok(())
}

fn now_iso() -> String {
    crate::usage::epoch_to_rfc3339(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs() as i64)
            .unwrap_or(0),
    )
}

async fn fetch_latest_laya() -> Result<String, AppError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("API-Desk/0.1 (local)")
        .build()
        .map_err(|error| AppError::Network(error.to_string()))?;
    let body = client
        .get("https://pypi.org/pypi/laya/json")
        .send()
        .await
        .map_err(|error| AppError::Network(sanitize(&error.to_string())))?
        .json::<Value>()
        .await
        .map_err(|error| AppError::Network(sanitize(&error.to_string())))?;
    parse_pypi_latest(&body).ok_or_else(|| AppError::Network("PyPI 메타데이터가 올바르지 않습니다".into()))
}

/// 캐시(TTL 24h) 또는 강제 새로고침으로 최신 stable 버전을 확인한다.
pub async fn check_latest_version(app: AppHandle, force: bool) -> LayaVersionCheck {
    let mut state = read_manager_state(&app);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0);
    let cached_at = state.version_cache.get("laya").copied();
    let fresh = cached_at.map(|at| now - at < VERSION_TTL_SECS).unwrap_or(false);
    if !force && fresh {
        if let Some(latest) = state.latest_version.clone() {
            return LayaVersionCheck {
                installed: None,
                update_available: false,
                latest: Some(latest),
                source: "pypi".into(),
                error: None,
                checked_at: cached_at.map(|at| crate::usage::epoch_to_rfc3339(at)),
                from_cache: true,
            };
        }
    }
    match fetch_latest_laya().await {
        Ok(latest) => {
            state.latest_version = Some(latest.clone());
            state.version_cache.insert("laya".into(), now);
            let _ = write_manager_state(&app, &state);
            LayaVersionCheck {
                installed: None,
                latest: Some(latest),
                update_available: false,
                source: "pypi".into(),
                error: None,
                checked_at: Some(crate::usage::epoch_to_rfc3339(now)),
                from_cache: false,
            }
        }
        Err(error) => LayaVersionCheck {
            installed: None,
            latest: state.latest_version.clone(),
            update_available: false,
            source: "pypi".into(),
            error: Some(error.to_string()),
            checked_at: cached_at.map(|at| crate::usage::epoch_to_rfc3339(at)),
            from_cache: true,
        },
    }
}

// ---------------------------------------------------------------- 환경 경로

fn envs_root(app: &AppHandle) -> Result<PathBuf, AppError> {
    let config = active_service_config(app)?;
    let executable = PathBuf::from(&config.executable);
    // 환경 폴더: <root>\.venv  또는  <root>\envs\<env> — 어느 쪽이든 envs 루트는 <root>\envs.
    let env_dir = executable
        .parent()
        .and_then(|scripts| scripts.parent())
        .map(Path::to_path_buf);
    let is_envs_dir = |path: &Path| {
        path.file_name()
            .map(|name| name == ENVS_DIR)
            .unwrap_or(false)
    };
    let root = match env_dir {
        Some(dir) if is_envs_dir(&dir) => dir.parent().map(Path::to_path_buf),
        Some(dir) => {
            let parent_is_envs = dir.parent().map(|parent| is_envs_dir(parent)).unwrap_or(false);
            if parent_is_envs {
                dir.parent().and_then(|parent| parent.parent()).map(Path::to_path_buf)
            } else {
                dir.parent().map(Path::to_path_buf)
            }
        }
        None => None,
    }
    .unwrap_or_else(|| PathBuf::from(r"C:\Laya"));
    let envs = root.join(ENVS_DIR);
    std::fs::create_dir_all(&envs)?;
    Ok(envs)
}

fn wrapper_path(app: &AppHandle) -> Result<PathBuf, AppError> {
    let envs = envs_root(app)?;
    let path = envs.join(WRAPPER_FILE);
    std::fs::write(&path, WRAPPER_SCRIPT)?;
    Ok(path)
}

pub fn env_python(env_dir: &Path) -> PathBuf {
    env_dir.join("Scripts").join("python.exe")
}

fn active_service_config(app: &AppHandle) -> Result<local_services::ServiceConfig, AppError> {
    local_services::service_config(app, "laya")
}

fn read_env_state(env_dir: &Path) -> Option<EnvStateFile> {
    std::fs::read_to_string(env_dir.join(ENV_STATE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(text.trim_start_matches('\u{feff}')).ok())
}

fn write_env_state(env_dir: &Path, state: &EnvStateFile) -> Result<(), AppError> {
    std::fs::create_dir_all(env_dir)?;
    let text = serde_json::to_string_pretty(state).map_err(|error| AppError::Io(error.to_string()))?;
    std::fs::write(env_dir.join(ENV_STATE_FILE), text)?;
    Ok(())
}

/// 등록된 환경 목록(불완전 후보 포함 — UI에서 상태를 보여주기 위해).
pub fn list_envs(app: &AppHandle) -> Vec<LayaEnvEntry> {
    let Ok(root) = envs_root(app) else {
        return Vec::new();
    };
    let manager = read_manager_state(app);
    let mut entries = Vec::new();
    if let Ok(read) = std::fs::read_dir(&root) {
        for item in read.flatten() {
            let dir = item.path();
            if !dir.is_dir() {
                continue;
            }
            if !env_python(&dir).exists() {
                continue;
            }
            let state = read_env_state(&dir).unwrap_or_default();
            entries.push(LayaEnvEntry {
                path: dir.to_string_lossy().to_string(),
                version: state.version,
                kind: if state.kind.is_empty() { "gpu".into() } else { state.kind },
                state: if state.state.is_empty() { "incomplete".into() } else { state.state },
                created_at: state.created_at,
                metrics: state.metrics,
                active: manager
                    .active_env
                    .as_deref()
                    .map(|active| active.eq_ignore_ascii_case(&dir.to_string_lossy()))
                    .unwrap_or(false),
            });
        }
    }
    entries
}

/// 기존 venv의 pyvenv.cfg에서 base Python을 찾아 후보 venv 생성에 사용한다.
pub fn base_python(current_python: &Path) -> Result<PathBuf, AppError> {
    let cfg = current_python
        .parent()
        .and_then(|scripts| scripts.parent())
        .map(|env_dir| env_dir.join("pyvenv.cfg"));
    if let Some(cfg) = cfg {
        if let Ok(text) = std::fs::read_to_string(&cfg) {
            for line in text.lines() {
                let lower = line.to_lowercase();
                if let Some(rest) = lower.strip_prefix("home") {
                    if let Some(value) = rest.split('=').nth(1) {
                        let candidate = PathBuf::from(line.split('=').nth(1).unwrap_or("").trim());
                        if candidate.join("python.exe").exists() {
                            return Ok(candidate.join("python.exe"));
                        }
                        let _ = value;
                    }
                }
            }
        }
    }
    // 폴백: PATH의 python
    let output = run_hidden("python", &["-c", "import sys;print(sys.executable)"])?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return Err(AppError::Io("Python 실행 파일을 찾을 수 없습니다".into()));
    }
    Ok(PathBuf::from(text))
}

// ---------------------------------------------------------------- 진단 실행

pub fn parse_diagnostic(value: &Value) -> (LayaEnvironmentStatus, Value) {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|item| !item.is_empty())
    };
    let cuda_available = value
        .get("cudaAvailable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tilelang_installed = value
        .get("tilelangInstalled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let fast_importable = value
        .get("fastImportable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let effective_device = value
        .get("effectiveDevice")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| Some(if cuda_available { "cuda" } else { "cpu" }.to_string()));
    let mut status = LayaEnvironmentStatus {
        python_version: text("pythonVersion"),
        laya_version: text("layaVersion"),
        torch_version: text("torchVersion"),
        torch_cuda_version: text("torchCudaVersion"),
        cuda_available,
        gpu_name: text("gpuName"),
        compute_capability: text("computeCapability"),
        tilelang_installed,
        tilelang_version: text("tilelangVersion"),
        fast_path_supported: cuda_available && tilelang_installed && fast_importable,
        effective_device,
        ..Default::default()
    };
    let mut problems = Vec::new();
    if status.torch_version.is_none() {
        problems.push("PyTorch를 확인할 수 없습니다".into());
    } else if !cuda_available {
        problems.push("CUDA를 사용할 수 없습니다 (CPU 모드)".into());
    }
    if !tilelang_installed {
        problems.push("TileLang이 설치되어 있지 않습니다".into());
    }
    if let Some(torch) = status.torch_version.as_deref() {
        if torch.contains("+cpu") {
            problems.push("PyTorch가 CPU 전용 빌드입니다".into());
        }
    }
    status.problems = problems;
    (status, value.clone())
}

/// 선택한 Laya Python으로 단일 진단을 실행한다(셸 미사용, 구조화 JSON).
pub fn diagnose(executable: &Path) -> Result<LayaEnvironmentStatus, AppError> {
    if !executable.exists() {
        return Err(AppError::Io(format!(
            "Python 실행 파일이 없습니다: {}",
            executable.to_string_lossy()
        )));
    }
    let output = run_hidden(
        &executable.to_string_lossy(),
        &["-c", DIAGNOSTIC_SCRIPT],
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(value) = parse_sentinel(&stdout) else {
        let stderr = sanitize(&String::from_utf8_lossy(&output.stderr));
        return Err(AppError::Io(format!(
            "진단 출력을 해석할 수 없습니다: {stderr}"
        )));
    };
    let (mut status, _) = parse_diagnostic(&value);
    status.python_path = executable.to_string_lossy().to_string();
    Ok(status)
}

// ---------------------------------------------------------------- 환경 상태 조회

#[tauri::command]
pub async fn laya_env_status(app: AppHandle) -> Result<LayaEnvironmentStatus, AppError> {
    let config = active_service_config(&app)?;
    let executable = PathBuf::from(&config.executable);
    let mut status = match diagnose(&executable) {
        Ok(status) => status,
        Err(error) => {
            let mut status = LayaEnvironmentStatus {
                python_path: config.executable.clone(),
                ..Default::default()
            };
            status.problems.push(sanitize(&error.to_string()));
            status
        }
    };
    let manager = read_manager_state(&app);
    let version = check_latest_version(app.clone(), false).await;
    status.latest_laya_version = version.latest.clone();
    let (update_available, version_error) = version_check(
        status.laya_version.as_deref(),
        version.latest.as_deref(),
    );
    status.update_available = update_available;
    status.version_check_error = version.error.clone().or(version_error);
    status.environment_kind = manager
        .active_env
        .as_deref()
        .map(|_| "managed".to_string())
        .unwrap_or_else(|| "legacy".into());
    status.environment_version = status.laya_version.clone();
    // 서비스 상태 + /health
    let (service_running, service_pid) = local_services::service_runtime(&app, "laya");
    status.service_running = service_running;
    status.service_pid = service_pid;
    if service_running {
        let port = if config.port == 0 { 8000 } else { config.port };
        let health_path = if config.health_path.is_empty() {
            "/health".into()
        } else {
            config.health_path.clone()
        };
        if let Some(body) = http_get_json("127.0.0.1", port, &health_path, 900) {
            let effective = body
                .get("device_effective")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(effective) = effective {
                status.effective_device = Some(effective.clone());
            }
            if let Some(backend) = body.get("backend").and_then(Value::as_str) {
                status.backend = Some(backend.to_string());
            }
            if let Some(fast) = body.get("fast_path").and_then(Value::as_bool) {
                status.fast_path_active = Some(fast);
            }
            if let Some(gpu) = body.get("gpu").and_then(Value::as_str) {
                status.gpu_name = Some(gpu.to_string());
            }
            if body.get("device_effective").and_then(Value::as_str) == Some("cpu")
                && status.problems.iter().all(|item| !item.contains("CPU"))
            {
                status.problems.push("서비스가 CPU로 실행 중입니다".into());
            }
            status.health = Some(body);
        }
    }
    Ok(status)
}

#[tauri::command]
pub async fn laya_env_check_updates(app: AppHandle, force: bool) -> Result<LayaVersionCheck, AppError> {
    let mut check = check_latest_version(app.clone(), force).await;
    let config = active_service_config(&app)?;
    let installed = diagnose(&PathBuf::from(&config.executable))
        .ok()
        .and_then(|status| status.laya_version);
    let (update_available, error) = version_check(installed.as_deref(), check.latest.as_deref());
    check.installed = installed;
    check.update_available = update_available;
    if check.error.is_none() {
        check.error = error;
    }
    Ok(check)
}

fn http_get_json(host: &str, port: u16, path: &str, timeout_ms: u64) -> Option<Value> {
    use std::io::{Read, Write};
    let address = format!("{host}:{port}");
    let socket = address.parse().ok()?;
    let mut stream = std::net::TcpStream::connect_timeout(&socket, Duration::from_millis(timeout_ms)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(timeout_ms)));
    let request = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    let body = response.split("\r\n\r\n").nth(1)?;
    serde_json::from_str::<Value>(body).ok()
}

// ---------------------------------------------------------------- 후보 생성 계획

#[derive(Clone, Debug, PartialEq)]
pub struct PlanStep {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
}

/// 후보 환경 생성 계획(순수 함수, 테스트 가능). 실제 실행은 `run_plan`.
pub fn build_create_plan(
    base_python: &Path,
    env_dir: &Path,
    version: &str,
    cuda_index: (&str, &str),
    wrapper: &Path,
) -> Vec<PlanStep> {
    let py = env_python(env_dir);
    let py_text = py.to_string_lossy().to_string();
    let dir_text = env_dir.to_string_lossy().to_string();
    vec![
        PlanStep {
            label: "Creating environment".into(),
            program: base_python.to_string_lossy().to_string(),
            args: vec!["-m".into(), "venv".into(), dir_text],
        },
        PlanStep {
            label: "Upgrading pip tooling".into(),
            program: py_text.clone(),
            args: vec![
                "-m".into(),
                "pip".into(),
                "install".into(),
                "--upgrade".into(),
                "pip".into(),
                "wheel".into(),
            ],
        },
        PlanStep {
            label: format!("Installing PyTorch ({})", cuda_index.0),
            program: py_text.clone(),
            args: vec![
                "-m".into(),
                "pip".into(),
                "install".into(),
                "torch".into(),
                "--index-url".into(),
                cuda_index.1.into(),
            ],
        },
        PlanStep {
            label: "Checking CUDA".into(),
            program: py_text.clone(),
            args: vec!["-c".into(), CUDA_VERIFY_SCRIPT.into()],
        },
        PlanStep {
            label: format!("Installing Laya {version} (serve, fast)"),
            program: py_text.clone(),
            args: vec![
                "-m".into(),
                "pip".into(),
                "install".into(),
                format!("laya[serve,fast]=={version}"),
            ],
        },
        PlanStep {
            label: "Running diagnostics".into(),
            program: py_text.clone(),
            args: vec!["-c".into(), DIAGNOSTIC_SCRIPT.into()],
        },
        PlanStep {
            label: "Loading model and running smoke test".into(),
            program: py_text.clone(),
            args: vec!["-c".into(), SMOKE_SCRIPT.into()],
        },
        PlanStep {
            label: "Testing HTTP server".into(),
            program: py_text,
            args: vec![wrapper.to_string_lossy().to_string()],
        },
    ]
}

fn emit_progress(app: &AppHandle, step: &str, label: &str, phase: &str, detail: Option<String>) {
    let _ = app.emit(
        "laya-env-progress",
        serde_json::json!({
            "step": step,
            "label": label,
            "phase": phase,
            "detail": detail,
            "at": now_iso(),
        }),
    );
}

fn interrupted(app: &AppHandle) -> bool {
    app.try_state::<LayaEnvState>()
        .map(|state| state.cancel.load(Ordering::Relaxed))
        .unwrap_or(false)
}

/// 계획 실행: 단계별 진행 이벤트 + 취소 지원. 실패 시 Err(단계 라벨, 사유).
fn run_plan(
    app: &AppHandle,
    steps: &[PlanStep],
    env_vars: &[(String, String)],
) -> Result<Vec<Value>, AppError> {
    let mut outputs = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        if interrupted(app) {
            return Err(AppError::InvalidRequest("사용자가 취소했습니다".into()));
        }
        emit_progress(app, &format!("{index}"), &step.label, "running", None);
        // HTTP 서버 테스트 단계는 별도 처리(프로세스 spawn + health 폴링)
        if step.label == "Testing HTTP server" {
            let probe = probe_candidate_server(app, step, env_vars);
            match probe {
                Ok(value) => {
                    emit_progress(app, &format!("{index}"), &step.label, "done", Some(sanitize(&value.to_string())));
                    outputs.push(value);
                }
                Err(error) => {
                    emit_progress(app, &format!("{index}"), &step.label, "failed", Some(sanitize(&error.to_string())));
                    return Err(error);
                }
            }
            continue;
        }
        let mut command = Command::new(&step.program);
        command.args(&step.args).creation_flags_hidden().stdin(Stdio::null());
        for (key, value) in env_vars {
            command.env(key, value);
        }
        let output = match command.output() {
            Ok(output) => output,
            Err(error) => {
                let message = sanitize(&error.to_string());
                emit_progress(app, &format!("{index}"), &step.label, "failed", Some(message.clone()));
                return Err(AppError::Io(format!("{}: {message}", step.label)));
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            let tail = sanitize(stderr.lines().rev().take(6).collect::<Vec<_>>().join(" ").as_str());
            emit_progress(app, &format!("{index}"), &step.label, "failed", Some(tail.clone()));
            return Err(AppError::Io(format!("{} 실패: {tail}", step.label)));
        }
        let value = parse_sentinel(&stdout).unwrap_or(Value::Null);
        emit_progress(
            app,
            &format!("{index}"),
            &step.label,
            "done",
            Some(sanitize(&stdout.lines().last().unwrap_or("").to_string())),
        );
        outputs.push(value);
    }
    Ok(outputs)
}

/// 후보 서버를 임시 포트에 띄워 /health와 추론 1건을 확인하고 종료한다.
fn probe_candidate_server(
    app: &AppHandle,
    step: &PlanStep,
    env_vars: &[(String, String)],
) -> Result<Value, AppError> {
    let port = 8123u16;
    let mut command = Command::new(&step.program);
    command
        .args(&step.args)
        .creation_flags_hidden()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in env_vars {
        command.env(key, value);
    }
    command.env("LAYA_PORT", port.to_string());
    command.env("LAYA_DEVICE", "cuda");
    let mut child = command
        .spawn()
        .map_err(|error| AppError::Io(format!("후보 서버 실행 실패: {}", sanitize(&error.to_string()))))?;
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut health = Value::Null;
    while Instant::now() < deadline {
        if interrupted(app) {
            let _ = child.kill();
            return Err(AppError::InvalidRequest("사용자가 취소했습니다".into()));
        }
        if let Some(body) = http_get_json("127.0.0.1", port, "/health", 800) {
            if body.get("ready").and_then(Value::as_bool).unwrap_or(false) {
                health = body;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(700));
    }
    if health.is_null() {
        let _ = child.kill();
        return Err(AppError::Io("후보 서버가 제한 시간 안에 준비되지 않았습니다".into()));
    }
    // health 확인 후, 서버를 살려 둔 채 추론을 검증한다(첫 추론은 커널 컴파일로 느릴 수 있음).
    let mut inference_ok = false;
    for attempt in 0..3 {
        if http_post_systemone(port, None) {
            inference_ok = true;
            break;
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_secs(3));
        }
    }
    let _ = child.kill();
    Ok(serde_json::json!({
        "health": health,
        "inferenceOk": inference_ok,
    }))
}

fn http_post_systemone(port: u16, api_key: Option<&str>) -> bool {
    use std::io::{Read, Write};
    let address = format!("127.0.0.1:{port}");
    let Ok(socket) = address.parse() else { return false };
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&socket, Duration::from_millis(800)) else {
        return false;
    };
    let body = serde_json::json!({
        "state": "I want to cancel my subscription.",
        "questions": {
            "intent": {"type": "choice", "instructions": "Pick the action.",
                       "criteria": {"cancel": "cancel the subscription", "keep": "keep the subscription"}}
        }
    })
    .to_string();
    let auth = api_key
        .filter(|key| !key.is_empty())
        .map(|key| format!("Authorization: Bearer {key}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST /v1/systemone HTTP/1.0\r\nHost: 127.0.0.1\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.starts_with("HTTP/1.0 200") || response.starts_with("HTTP/1.1 200")
}

fn cuda_env_vars(version: &str) -> Vec<(String, String)> {
    vec![
        ("LAYA_DEVICE".into(), "cuda".into()),
        ("LAYA_PRELOAD".into(), "1".into()),
        ("LAYA_FAST".into(), "1".into()),
        ("LAYA_SMOKE_MODELS".into(), DEFAULT_MODELS.join(",")),
        ("LAYA_BENCH_MODELS".into(), "english".into()),
        ("LAYA_VERSION_TARGET".into(), version.to_string()),
    ]
}

// ---------------------------------------------------------------- 후보 생성 커맨드

#[tauri::command]
pub async fn laya_env_create_gpu(app: AppHandle, target_version: Option<String>) -> Result<(), AppError> {
    let state = app.state::<LayaEnvState>();
    if state.running.swap(true, Ordering::SeqCst) {
        return Err(AppError::InvalidRequest("이미 환경 작업이 진행 중입니다".into()));
    }
    state.cancel.store(false, Ordering::SeqCst);
    let handle = app.clone();
    std::thread::spawn(move || {
        let result = create_gpu_candidate(&handle, target_version);
        if let Some(state) = handle.try_state::<LayaEnvState>() {
            state.running.store(false, Ordering::SeqCst);
            if let Err(error) = &result {
                if let Ok(mut slot) = state.last_error.lock() {
                    *slot = Some(sanitize(&error.to_string()));
                }
            }
        }
        let _ = handle.emit(
            "laya-env-progress",
            serde_json::json!({
                "step": "final",
                "label": if result.is_ok() { "Ready." } else { "Failed" },
                "phase": if result.is_ok() { "done" } else { "failed" },
                "detail": result.err().map(|error| sanitize(&error.to_string())),
                "at": now_iso(),
            }),
        );
    });
    Ok(())
}

fn create_gpu_candidate(app: &AppHandle, target_version: Option<String>) -> Result<(), AppError> {
    let config = active_service_config(app)?;
    let current_python = PathBuf::from(&config.executable);
    let base = base_python(&current_python)?;
    let envs = envs_root(app)?;
    let version = match target_version {
        Some(version) if !version.trim().is_empty() => version,
        _ => {
            let check = tauri::async_runtime::block_on(check_latest_version(app.clone(), true));
            check
                .latest
                .ok_or_else(|| AppError::Network(check.error.unwrap_or_else(|| "최신 버전 확인 실패".into())))?
        }
    };
    let env_dir = envs.join(format!("laya-{version}-gpu"));
    std::fs::create_dir_all(&env_dir)?;
    write_env_state(
        &env_dir,
        &EnvStateFile {
            state: "incomplete".into(),
            version: Some(version.clone()),
            kind: "gpu".into(),
            created_at: Some(now_iso()),
            updated_at: Some(now_iso()),
            ..Default::default()
        },
    )?;
    let wrapper = wrapper_path(app)?;
    let mut last_error: Option<AppError> = None;
    for index in TORCH_CUDA_INDEXES {
        emit_progress(app, "plan", &format!("Trying PyTorch {}", index.0), "running", None);
        let plan = build_create_plan(&base, &env_dir, &version, index, &wrapper);
        match run_plan(app, &plan, &cuda_env_vars(&version)) {
            Ok(outputs) => {
                let diag = outputs.get(5).cloned().unwrap_or(Value::Null);
                let smoke = outputs.get(6).cloned().unwrap_or(Value::Null);
                let server = outputs.get(7).cloned().unwrap_or(Value::Null);
                let fast = smoke.get("fastPath").and_then(Value::as_bool).unwrap_or(false);
                let devices = smoke.get("devices").cloned().unwrap_or(Value::Null);
                let tilelang = diag.get("tilelangInstalled").and_then(Value::as_bool).unwrap_or(false);
                write_env_state(
                    &env_dir,
                    &EnvStateFile {
                        state: "ready".into(),
                        version: Some(version.clone()),
                        kind: "gpu".into(),
                        created_at: read_env_state(&env_dir).and_then(|state| state.created_at).or_else(|| Some(now_iso())),
                        updated_at: Some(now_iso()),
                        torch_cuda_version: diag.get("torchCudaVersion").and_then(Value::as_str).map(str::to_string),
                        gpu_name: diag.get("gpuName").and_then(Value::as_str).map(str::to_string),
                        tilelang: Some(tilelang),
                        fast_path: Some(fast),
                        backend: Some(if fast { "tilelang".into() } else { "torch".into() }),
                        metrics: Some(serde_json::json!({
                            "smoke": smoke,
                            "server": server,
                            "devices": devices,
                            "cudaIndex": index.0,
                        })),
                        previous_env: None,
                    },
                )?;
                return Ok(());
            }
            Err(error) => {
                emit_progress(app, "plan", &format!("PyTorch {} 실패", index.0), "failed", Some(sanitize(&error.to_string())));
                last_error = Some(error);
            }
        }
    }
    // 실패: 불완전 후보로 남긴다(자동 활성화 금지, 다음 실행에서 무시).
    let mut state = read_env_state(&env_dir).unwrap_or_default();
    state.state = "incomplete".into();
    state.updated_at = Some(now_iso());
    let _ = write_env_state(&env_dir, &state);
    Err(last_error.unwrap_or_else(|| AppError::Io("GPU 환경 생성에 실패했습니다".into())))
}

#[tauri::command]
pub fn laya_env_cancel(app: AppHandle) -> Result<(), AppError> {
    if let Some(state) = app.try_state::<LayaEnvState>() {
        state.cancel.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[tauri::command]
pub fn laya_env_busy(app: AppHandle) -> bool {
    app.try_state::<LayaEnvState>()
        .map(|state| state.is_running())
        .unwrap_or(false)
}

#[tauri::command]
pub fn laya_env_list(app: AppHandle) -> Vec<LayaEnvEntry> {
    list_envs(&app)
}

// ---------------------------------------------------------------- 활성화 / 롤백

fn save_laya_config(app: &AppHandle, executable: &str, args: Vec<String>, env_dir: &str, version: Option<&str>, kind: &str) -> Result<(), AppError> {
    let mut config = active_service_config(app)?;
    config.executable = executable.to_string();
    config.args = args;
    if !env_dir.is_empty() {
        config.workdir = env_dir.to_string();
    }
    local_services::save_service_config(app, config, version, kind)
}

/// 후보 활성화: 서비스 중지 → 포트 해제 → 설정 원자 교체 → 시작 → /health 대기 → 스모크.
/// 실패하면 이전 환경 설정으로 자동 복구한다.
#[tauri::command]
pub async fn laya_env_activate(app: AppHandle, env_path: String) -> Result<String, AppError> {
    let env_dir = PathBuf::from(&env_path);
    if !env_python(&env_dir).exists() {
        return Err(AppError::InvalidRequest("후보 환경의 Python을 찾을 수 없습니다".into()));
    }
    let env_state = read_env_state(&env_dir).unwrap_or_default();
    if env_state.state != "ready" {
        return Err(AppError::InvalidRequest(
            "검증을 통과한 후보 환경만 활성화할 수 있습니다".into(),
        ));
    }
    let manager = read_manager_state(&app);
    let previous = active_service_config(&app)?;
    let previous_snapshot = serde_json::to_value(&previous).unwrap_or(Value::Null);
    let wrapper = wrapper_path(&app)?;
    let version = env_state.version.clone();

    // 1) 서비스 중지 + 포트 해제
    emit_progress(&app, "activate", "Stopping current Laya", "running", None);
    local_services::stop_service_for_activation(&app, "laya")?;
    // 2) 설정 교체
    save_laya_config(
        &app,
        &env_python(&env_dir).to_string_lossy(),
        vec![wrapper.to_string_lossy().to_string()],
        &env_dir.to_string_lossy(),
        version.as_deref(),
        "gpu",
    )?;
    // 3) 시작 + health + 스모크
    emit_progress(&app, "activate", "Starting candidate Laya", "running", None);
    let start_result = local_services::start_service_for_activation(&app, "laya");
    let activation_ok = start_result.is_ok() && wait_for_health(&app, Duration::from_secs(240));
    if activation_ok {
        let mut state = read_env_state(&env_dir).unwrap_or_default();
        state.state = "active".into();
        state.previous_env = manager.active_env.clone();
        state.updated_at = Some(now_iso());
        let _ = write_env_state(&env_dir, &state);
        let mut manager = read_manager_state(&app);
        manager.previous_env = manager.active_env.clone();
        manager.active_env = Some(env_dir.to_string_lossy().to_string());
        let _ = write_manager_state(&app, &manager);
        emit_progress(&app, "activate", "Activated", "done", None);
        return Ok(env_dir.to_string_lossy().to_string());
    }

    // 4) 자동 롤백
    emit_progress(&app, "activate", "Activation failed — rolling back", "failed", None);
    let rollback_result = rollback_to(&app, &previous_snapshot);
    match rollback_result {
        Ok(()) => Err(AppError::Io("업데이트 실패 — 기존 환경으로 자동 복구했습니다".into())),
        Err(error) => Err(AppError::Io(format!(
            "업데이트 실패 — 자동 복구도 실패했습니다: {}",
            sanitize(&error.to_string())
        ))),
    }
}

fn wait_for_health(app: &AppHandle, timeout: Duration) -> bool {
    let config = active_service_config(app).unwrap_or_default();
    let port = if config.port == 0 { 8000 } else { config.port };
    // 서비스에는 Vault의 LAYA_API_KEY가 주입되므로 추론 검증 시 동일 키를 사용한다(로그 금지).
    let api_key = local_services::secret_for_probe(app, "laya");
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(body) = http_get_json("127.0.0.1", port, "/health", 900) {
            if body.get("ready").and_then(Value::as_bool).unwrap_or(false) {
                return http_post_systemone(port, api_key.as_deref());
            }
        }
        std::thread::sleep(Duration::from_millis(900));
    }
    false
}

fn rollback_to(app: &AppHandle, snapshot: &Value) -> Result<(), AppError> {
    let config: local_services::ServiceConfig = serde_json::from_value(snapshot.clone())
        .map_err(|error| AppError::Io(format!("이전 서비스 설정을 복원할 수 없습니다: {error}")))?;
    local_services::stop_service_for_activation(app, "laya")?;
    local_services::restore_service_config(app, config)?;
    local_services::start_service_for_activation(app, "laya")?;
    if !wait_for_health(app, Duration::from_secs(180)) {
        return Err(AppError::Io("복구 후 Laya가 준비되지 않았습니다".into()));
    }
    Ok(())
}

#[tauri::command]
pub async fn laya_env_rollback(app: AppHandle) -> Result<String, AppError> {
    let manager = read_manager_state(&app);
    let Some(previous) = manager.previous_env.clone() else {
        return Err(AppError::InvalidRequest("이전 환경이 없습니다".into()));
    };
    let env_dir = PathBuf::from(&previous);
    if !env_python(&env_dir).exists() {
        return Err(AppError::InvalidRequest("이전 환경을 찾을 수 없습니다".into()));
    }
    let state = read_env_state(&env_dir).unwrap_or_default();
    let wrapper = wrapper_path(&app)?;
    let current = active_service_config(&app)?;
    local_services::stop_service_for_activation(&app, "laya")?;
    save_laya_config(
        &app,
        &env_python(&env_dir).to_string_lossy(),
        vec![wrapper.to_string_lossy().to_string()],
        &env_dir.to_string_lossy(),
        state.version.as_deref(),
        if state.kind.is_empty() { "gpu" } else { &state.kind },
    )?;
    local_services::start_service_for_activation(&app, "laya")?;
    if !wait_for_health(&app, Duration::from_secs(180)) {
        let _ = local_services::stop_service_for_activation(&app, "laya");
        let snapshot = serde_json::to_value(&current).unwrap_or(Value::Null);
        let _ = rollback_to(&app, &snapshot);
        return Err(AppError::Io("이전 버전 복구에 실패했습니다".into()));
    }
    let mut manager = read_manager_state(&app);
    manager.active_env = Some(previous);
    manager.previous_env = None;
    let _ = write_manager_state(&app, &manager);
    Ok("이전 버전으로 되돌렸습니다".into())
}

// ---------------------------------------------------------------- 벤치마크

#[tauri::command]
pub async fn laya_env_benchmark(app: AppHandle, env_path: Option<String>, iterations: Option<u32>) -> Result<LayaBenchmark, AppError> {
    let executable = match env_path {
        Some(path) if !path.trim().is_empty() => env_python(Path::new(&path)),
        _ => PathBuf::from(active_service_config(&app)?.executable),
    };
    if !executable.exists() {
        return Err(AppError::Io("Python 실행 파일이 없습니다".into()));
    }
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || run_benchmark(&handle, &executable, iterations.unwrap_or(5)))
        .await
        .map_err(|error| AppError::Io(format!("벤치마크 실패: {error}")))?
}

fn run_benchmark(app: &AppHandle, executable: &Path, iterations: u32) -> Result<LayaBenchmark, AppError> {
    let request = env_path_request(app);
    let _ = request;
    let mut command = Command::new(executable);
    command
        .args(["-c", BENCH_SCRIPT])
        .creation_flags_hidden()
        .stdin(Stdio::null())
        .env("LAYA_DEVICE", "cuda")
        .env("LAYA_FAST", "1")
        .env("LAYA_BENCH_ITERS", iterations.to_string())
        .env("LAYA_BENCH_MODELS", DEFAULT_MODELS.join(","));
    let output = command
        .output()
        .map_err(|error| AppError::Io(format!("벤치마크 실행 실패: {}", sanitize(&error.to_string()))))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(value) = parse_sentinel(&stdout) else {
        let stderr = sanitize(&String::from_utf8_lossy(&output.stderr));
        return Ok(LayaBenchmark {
            python_path: executable.to_string_lossy().to_string(),
            error: Some(format!("벤치마크 출력을 해석할 수 없습니다: {stderr}")),
            ..Default::default()
        });
    };
    let diag = diagnose(executable).ok();
    Ok(LayaBenchmark {
        python_path: executable.to_string_lossy().to_string(),
        device: value.get("device").and_then(Value::as_str).map(str::to_string),
        backend: value
            .get("fastPath")
            .and_then(Value::as_bool)
            .map(|fast| if fast { "tilelang".to_string() } else { "torch".to_string() }),
        laya_version: value.get("layaVersion").and_then(Value::as_str).map(str::to_string),
        torch_version: diag.as_ref().and_then(|status| status.torch_version.clone()),
        cold_load_ms: value.get("coldLoadMs").and_then(Value::as_u64),
        warm_p50_ms: value.get("warmP50Ms").and_then(Value::as_f64),
        batch_p50_ms: value.get("batchP50Ms").and_then(Value::as_f64),
        iterations,
        loaded_models: value
            .get("loaded")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        vram_used_bytes: value.get("vramAllocatedBytes").and_then(Value::as_u64),
        error: None,
    })
}

fn env_path_request(_app: &AppHandle) {}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_semantically() {
        assert_eq!(compare_versions("0.3.18", "0.3.21"), std::cmp::Ordering::Less);
        assert_eq!(compare_versions("0.3.21", "0.3.21"), std::cmp::Ordering::Equal);
        assert_eq!(compare_versions("0.4.0", "0.3.21"), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions("1.0", "0.9.9"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn parses_pypi_latest_and_excludes_prerelease() {
        let stable = serde_json::json!({"info": {"version": "0.3.21"}});
        assert_eq!(parse_pypi_latest(&stable).as_deref(), Some("0.3.21"));
        let rc = serde_json::json!({"info": {"version": "0.3.22rc1"}});
        assert_eq!(parse_pypi_latest(&rc), None, "pre-release 제외");
        let empty = serde_json::json!({"info": {}});
        assert_eq!(parse_pypi_latest(&empty), None);
    }

    #[test]
    fn version_check_matrix() {
        assert_eq!(version_check(Some("0.3.18"), Some("0.3.21")), (true, None));
        assert_eq!(version_check(Some("0.3.21"), Some("0.3.21")), (false, None));
        let (flag, error) = version_check(Some("0.3.21"), None);
        assert!(!flag);
        assert!(error.is_some(), "오프라인 상태는 오류로 보고");
    }

    #[test]
    fn diagnostic_parses_cpu_torch() {
        let value = serde_json::json!({
            "pythonVersion": "3.13.12",
            "layaVersion": "0.3.18",
            "torchVersion": "2.14.0+cpu",
            "torchCudaVersion": null,
            "cudaAvailable": false,
            "tilelangInstalled": false,
            "fastImportable": false,
            "effectiveDevice": "cpu"
        });
        let (status, _) = parse_diagnostic(&value);
        assert!(!status.cuda_available);
        assert_eq!(status.effective_device.as_deref(), Some("cpu"));
        assert!(!status.fast_path_supported);
        assert!(status.problems.iter().any(|item| item.contains("CPU")));
    }

    #[test]
    fn diagnostic_parses_cuda_torch_with_fast_path() {
        let value = serde_json::json!({
            "pythonVersion": "3.13.12",
            "layaVersion": "0.3.21",
            "torchVersion": "2.14.0+cu130",
            "torchCudaVersion": "13.0",
            "cudaAvailable": true,
            "gpuName": "NVIDIA GeForce RTX 5090",
            "computeCapability": "12.0",
            "tilelangInstalled": true,
            "tilelangVersion": "0.1.20",
            "fastImportable": true,
            "effectiveDevice": "cuda"
        });
        let (status, _) = parse_diagnostic(&value);
        assert!(status.cuda_available);
        assert_eq!(status.gpu_name.as_deref(), Some("NVIDIA GeForce RTX 5090"));
        assert_eq!(status.compute_capability.as_deref(), Some("12.0"));
        assert!(status.fast_path_supported);
        assert!(status.problems.is_empty());
    }

    #[test]
    fn diagnostic_cuda_stock_fallback_is_not_cpu() {
        let value = serde_json::json!({
            "torchVersion": "2.14.0+cu130",
            "torchCudaVersion": "13.0",
            "cudaAvailable": true,
            "gpuName": "NVIDIA GeForce RTX 5090",
            "tilelangInstalled": false,
            "fastImportable": false,
            "effectiveDevice": "cuda"
        });
        let (status, _) = parse_diagnostic(&value);
        assert!(status.cuda_available);
        assert!(!status.fast_path_supported, "타일랭 없으면 fast 미지원");
        assert_eq!(status.effective_device.as_deref(), Some("cuda"), "CUDA stock은 유지");
        assert!(status.problems.iter().any(|item| item.contains("TileLang")));
    }

    #[test]
    fn diagnostic_malformed_output_fails() {
        let (status, _) = parse_diagnostic(&Value::Null);
        assert!(status.torch_version.is_none());
        assert!(!status.cuda_available);
        assert!(parse_sentinel("no sentinel here").is_none());
    }

    #[test]
    fn sanitize_redacts_tokens() {
        let text = sanitize("token hf_abc123 and sk-live999 and Bearer xyz");
        assert!(!text.contains("hf_abc123"));
        assert!(!text.contains("sk-live999"));
        assert!(!text.contains("xyz"));
    }

    #[test]
    fn plan_uses_explicit_executables_and_no_shell() {
        let plan = build_create_plan(
            Path::new(r"C:\Python313\python.exe"),
            Path::new(r"C:\Laya\envs\laya-0.3.21-gpu"),
            "0.3.21",
            ("cu130", "https://download.pytorch.org/whl/cu130"),
            Path::new(r"C:\Laya\envs\laya_health.py"),
        );
        assert_eq!(plan.len(), 8);
        assert_eq!(plan[0].program, r"C:\Python313\python.exe");
        assert_eq!(plan[0].args, vec!["-m", "venv", r"C:\Laya\envs\laya-0.3.21-gpu"]);
        assert!(plan[2].args.contains(&"torch".to_string()));
        assert!(plan[2].args.contains(&"https://download.pytorch.org/whl/cu130".to_string()));
        assert!(plan[4].args.iter().any(|arg| arg == "laya[serve,fast]==0.3.21"));
        for step in &plan {
            assert!(!step.program.to_lowercase().contains("cmd"), "셸 미사용");
        }
    }

    #[test]
    fn incomplete_candidate_state_is_not_ready() {
        let dir = std::env::temp_dir().join("laya-env-test-incomplete");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // python.exe 없음 → 목록에 안 나오고 상태 파일도 incomplete
        write_env_state(
            &dir,
            &EnvStateFile {
                state: "incomplete".into(),
                version: Some("0.3.21".into()),
                kind: "gpu".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let state = read_env_state(&dir).unwrap();
        assert_eq!(state.state, "incomplete");
        assert!(!env_python(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
