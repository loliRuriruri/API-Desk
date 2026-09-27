//! TURZX 3.5" AI 대시보드 (표시 전용, OUTPUT ONLY).
//!
//! - 기존 GPU 모니터 캐시와 API Desk 스냅샷(DB)을 재사용한다. 새 폴링 시스템을 만들지 않는다.
//! - 외부 API를 호출하지 않는다(화면 갱신이 provider 호출을 유발하면 안 됨).
//! - 앱 로직을 변경하지 않는다(서비스 시작/중지, 계정 전환, Vault 접근 없음).
pub mod device;
pub mod gdi;
pub mod protocol;
pub mod renderer;

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serialport::SerialPort;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::Row;
use tauri::{AppHandle, Manager};

use crate::error::AppError;
use device::PortCandidate;
use protocol::{Orientation, PanelModel};
use renderer::TurzxDisplaySnapshot;

const SETTINGS_FILE: &str = "turzx-display.json";
const WORKER_TICK_MS: u64 = 400;
const SERIAL_TIMEOUT_MS: u64 = 500;
const BITMAP_COOLDOWN_MS: u64 = 50;
const TILE_PX: usize = 32;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TurzxSettings {
    pub enabled: bool,
    /// "auto" 또는 "COM4" 처럼 명시 포트.
    pub port: String,
    /// "portrait" | "landscape"
    pub orientation: String,
    /// 0 | 90 | 180 | 270
    pub rotation: u16,
    pub brightness: u8,
    pub refresh_secs: u64,
    /// "single" | "runtime" | "api" | "rotate"
    pub page_mode: String,
    pub page_rotation_secs: u64,
    pub auto_reconnect: bool,
    pub launch_with_app: bool,
    pub device_label: String,
}

impl Default for TurzxSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: "auto".into(),
            orientation: "portrait".into(),
            rotation: 0,
            brightness: 70,
            refresh_secs: 2,
            page_mode: "single".into(),
            page_rotation_secs: 8,
            auto_reconnect: true,
            launch_with_app: true,
            device_label: "TURZX 3.5\"".into(),
        }
    }
}

#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TurzxStatus {
    pub enabled: bool,
    pub connected: bool,
    pub port: Option<String>,
    pub device: Option<String>,
    pub model: Option<String>,
    pub resolution: String,
    pub orientation: String,
    pub page: String,
    pub last_error: Option<String>,
    pub last_frame_at: Option<String>,
    pub last_frame_ms: Option<u64>,
    pub last_bytes_sent: Option<usize>,
    pub last_update_kind: Option<String>,
    pub candidates: Vec<PortCandidate>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TurzxPreview {
    pub width: u16,
    pub height: u16,
    pub page: String,
    pub mode: String,
    pub rgb_base64: String,
}

pub struct TurzxState {
    settings: Mutex<TurzxSettings>,
    status: Mutex<TurzxStatus>,
    running: Arc<AtomicBool>,
    frame: Mutex<Option<(Vec<u8>, u16, u16, String)>>,
    preview: Mutex<Option<TurzxPreview>>,
}

impl Default for TurzxState {
    fn default() -> Self {
        Self {
            settings: Mutex::new(TurzxSettings::default()),
            status: Mutex::new(TurzxStatus::default()),
            running: Arc::new(AtomicBool::new(false)),
            frame: Mutex::new(None),
            preview: Mutex::new(None),
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<std::path::PathBuf, AppError> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|error| AppError::Io(error.to_string()))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(SETTINGS_FILE))
}

fn read_settings(app: &AppHandle) -> TurzxSettings {
    settings_path(app)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| {
            let trimmed = text.trim_start_matches('\u{feff}');
            serde_json::from_str(trimmed).ok()
        })
        .unwrap_or_default()
}

fn write_settings(app: &AppHandle, settings: &TurzxSettings) -> Result<(), AppError> {
    let path = settings_path(app)?;
    let text = serde_json::to_string_pretty(settings).map_err(|error| AppError::Io(error.to_string()))?;
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

// ---------------------------------------------------------------- snapshot

fn rows_from_db(app: &AppHandle) -> Vec<renderer::DisplayApiRow> {
    let Ok(dir) = app.path().app_config_dir() else {
        return Vec::new();
    };
    let db_path = dir.join("apidb.db");
    if !db_path.exists() {
        return Vec::new();
    }
    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .read_only(true);
    let runtime = tauri::async_runtime::block_on(async move {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .ok()?;
        let monitors = sqlx::query(
            "SELECT monitor, status, details_json, fetched_at FROM monitor_snapshots",
        )
        .fetch_all(&pool)
        .await
        .ok()?;
        let usage = sqlx::query(
            "SELECT u.adapter, u.summary, u.used, u.limit_total, u.remaining, u.currency, u.fetched_at, c.name AS credential_name
             FROM usage_snapshots u LEFT JOIN credentials c ON c.id = u.credential_id",
        )
        .fetch_all(&pool)
        .await
        .ok()?;
        Some((monitors, usage))
    });
    let Some((monitors, usage)) = runtime else {
        return Vec::new();
    };
    let now = crate::usage::epoch_to_rfc3339(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs() as i64)
            .unwrap_or(0),
    );
    let age = |fetched_at: Option<&str>| -> i64 {
        let now_secs = crate::monitors::parse_rfc3339_secs(&now).unwrap_or(0);
        let then = fetched_at
            .and_then(crate::monitors::parse_rfc3339_secs)
            .unwrap_or(now_secs);
        (now_secs - then).max(0)
    };

    let mut rows: Vec<renderer::DisplayApiRow> = Vec::new();
    for (index, row) in monitors.iter().enumerate() {
        let monitor: String = row.get("monitor");
        if monitor.starts_with("antigravity:") {
            continue; // 계정별 스냅샷은 요약 행에서 제외
        }
        let details: Option<String> = row.get("details_json");
        let fetched: Option<String> = row.get("fetched_at");
        let status: Option<String> = row.get("status");
        if let Some(mut normalized) = renderer::normalize_monitor_row(&monitor, details.as_deref()) {
            normalized.age_secs = age(fetched.as_deref());
            normalized.status = status.unwrap_or_else(|| "ok".into());
            rows.push(normalized);
        } else if index == 0 {
            // 알 수 없는 monitor 라도 무시한다(새 폴링 금지).
        }
    }
    for row in usage.iter() {
        let adapter: String = row.get("adapter");
        let label: Option<String> = row.try_get("credential_name").ok().flatten();
        let summary: String = row.try_get("summary").unwrap_or_default();
        let used: Option<f64> = row.try_get("used").ok().flatten();
        let limit: Option<f64> = row.try_get("limit_total").ok().flatten();
        let remaining: Option<f64> = row.try_get("remaining").ok().flatten();
        let currency: Option<String> = row.try_get("currency").ok().flatten();
        let fetched: Option<String> = row.try_get("fetched_at").ok().flatten();
        let mut normalized = renderer::normalize_usage_row(
            &adapter,
            label.as_deref().unwrap_or(&adapter),
            used,
            limit,
            remaining,
            currency.as_deref(),
        );
        normalized.age_secs = age(fetched.as_deref());
        if normalized.primary_remaining_percent.is_none() && normalized.amount_text.is_none() {
            normalized.amount_text = Some(protocol::truncate(&summary, 18)).filter(|text| !text.is_empty());
        }
        rows.push(normalized);
    }
    renderer::prioritize_api_rows(rows)
}

/// 시스템 RAM 사용량(전체/사용). Windows GlobalMemoryStatusEx.
#[cfg(windows)]
fn system_memory() -> (Option<u64>, Option<u64>) {
    use std::ffi::c_void;

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut c_void) -> i32;
    }
    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    let ok = unsafe { GlobalMemoryStatusEx(&mut status as *mut _ as *mut c_void) };
    if ok == 0 || status.total_phys == 0 {
        return (None, None);
    }
    (Some(status.total_phys.saturating_sub(status.avail_phys)), Some(status.total_phys))
}

#[cfg(not(windows))]
fn system_memory() -> (Option<u64>, Option<u64>) {
    (None, None)
}

pub(crate) fn build_snapshot(app: &AppHandle) -> TurzxDisplaySnapshot {
    let (ram_used, ram_total) = system_memory();
    let gpu_snapshot = crate::gpu_monitor::cached_snapshot(app, 2_000);
    let gpu = match gpu_snapshot.gpus.first() {
        Some(info) => renderer::DisplayGpu {
            name: info.name.clone(),
            utilization_percent: info.utilization_percent,
            vram_used_bytes: info.memory_used_bytes,
            vram_total_bytes: info.memory_total_bytes,
            temperature_c: info.temperature_c,
            power_watts: info.power_watts,
            ram_used_bytes: ram_used,
            ram_total_bytes: ram_total,
        },
        None => renderer::DisplayGpu::default(),
    };
    let workloads: Vec<renderer::DisplayWorkload> = gpu_snapshot
        .processes
        .iter()
        .filter(|process| !process.display_name.is_empty() && process.classification != "system")
        .map(|process| renderer::DisplayWorkload {
            service: process.display_name.clone(),
            model: if process.models.is_empty() {
                process.runtime.clone()
            } else {
                Some(process.models.join(" · "))
            },
            vram_bytes: process.used_vram_bytes,
            kind: process.service_kind.clone(),
            confidence: process.confidence.clone(),
            cpu_only: false,
            gpu_percent: process.gpu_percent,
            classification: process.classification.clone(),
        })
        .collect();
    TurzxDisplaySnapshot {
        timestamp: now_iso(),
        gpu,
        workloads,
        api_usage: rows_from_db(app),
    }
}

// ---------------------------------------------------------------- transport

fn effective_orientation(settings: &TurzxSettings) -> Orientation {
    let base = Orientation::parse(&settings.orientation);
    Orientation::from_rotation(settings.rotation, base)
}

fn open_port(port_name: &str) -> Result<Box<dyn SerialPort>, AppError> {
    // 참조 구현과 동일하게 RTS/CTS 하드웨어 흐름 제어로 연다
    // (대량 비트맵 전송에서 필수), DTR도 함께 올린다.
    let mut port = serialport::new(port_name, protocol::BAUD_RATE)
        .flow_control(serialport::FlowControl::Hardware)
        .timeout(Duration::from_millis(SERIAL_TIMEOUT_MS))
        .open()
        .map_err(|error| AppError::Io(format!("{port_name} 열기 실패: {error}")))?;
    let _ = port.write_data_terminal_ready(true);
    Ok(port)
}

fn handshake(port: &mut Box<dyn SerialPort>) -> Result<PanelModel, AppError> {
    // Rev A HELLO = 0x45 x6. 정식 Turing 3.5는 응답하지 않으며, 그 경우도 정상으로 본다.
    port.write_all(&protocol::hello_packet())
        .map_err(|error| AppError::Io(format!("HELLO 전송 실패: {error}")))?;
    let _ = port.flush();
    let mut response = [0u8; 6];
    let mut filled = 0usize;
    let deadline = Instant::now() + Duration::from_millis(500);
    while filled < response.len() && Instant::now() < deadline {
        match port.read(&mut response[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(ref error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    Ok(protocol::parse_hello(&response[..filled]))
}

fn configure(
    port: &mut Box<dyn SerialPort>,
    model: PanelModel,
    settings: &TurzxSettings,
) -> Result<Orientation, AppError> {
    if !model.is_supported() {
        return Err(AppError::Io(format!(
            "{} 패널은 아직 지원하지 않습니다 (320x480 3.5\"만 지원)",
            model.label()
        )));
    }
    let orientation = effective_orientation(settings);
    port.write_all(&protocol::screen_packet(true))
        .map_err(|error| AppError::Io(format!("화면 켜기 실패: {error}")))?;
    port.write_all(&protocol::orientation_packet(orientation))
        .map_err(|error| AppError::Io(format!("방향 설정 실패: {error}")))?;
    port.write_all(&protocol::brightness_packet(settings.brightness))
        .map_err(|error| AppError::Io(format!("밝기 설정 실패: {error}")))?;
    let _ = port.flush();
    Ok(orientation)
}

/// 영역(부분 프레임) 전송.
fn send_region(
    port: &mut Box<dyn SerialPort>,
    frame: &[u8],
    width: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
) -> Result<usize, AppError> {
    let region_width = x1 - x0 + 1;
    let region_height = y1 - y0 + 1;
    let mut rgb = Vec::with_capacity(region_width * region_height * 3);
    for y in y0..=y1 {
        let start = (y * width + x0) * 3;
        rgb.extend_from_slice(&frame[start..start + region_width * 3]);
    }
    let payload = protocol::rgb565_le(&rgb);
    let header = protocol::command_packet(
        protocol::CMD_DISPLAY_BITMAP,
        x0 as u16,
        y0 as u16,
        x1 as u16,
        y1 as u16,
    );
    port.write_all(&header)
        .map_err(|error| AppError::Io(format!("비트맵 헤더 전송 실패: {error}")))?;
    let chunk = protocol::chunk_size(width as u16).max(64);
    // 한 번의 WriteFile이 쓰기 타임아웃(500ms)을 넘지 않도록 1KB 이하로 나눠 보낸다.
    const WRITE_PIECE: usize = 1024;
    let mut sent = header.len();
    for piece in payload.chunks(chunk) {
        for slice in piece.chunks(WRITE_PIECE) {
            port.write_all(slice)
                .map_err(|error| AppError::Io(format!("프레임 전송 실패: {error}")))?;
        }
        let _ = port.flush();
        sent += piece.len();
    }
    std::thread::sleep(Duration::from_millis(BITMAP_COOLDOWN_MS));
    Ok(sent)
}

// ---------------------------------------------------------------- worker

fn set_status(app: &AppHandle, update: impl FnOnce(&mut TurzxStatus)) {
    if let Some(state) = app.try_state::<TurzxState>() {
        let mut status = state.status.lock().unwrap();
        update(&mut status);
    }
}


pub fn start_worker(app: &AppHandle) {
    let state = app.state::<TurzxState>();
    if state.running.swap(true, Ordering::SeqCst) {
        return;
    }
    // 앱 시작/외부 편집 시에도 파일 설정이 반영되도록 상태를 갱신한다.
    *state.settings.lock().unwrap() = read_settings(app);
    let handle = app.clone();
    std::thread::spawn(move || worker_loop(handle));
}

pub fn stop_worker(app: &AppHandle) {
    if let Some(state) = app.try_state::<TurzxState>() {
        state.running.store(false, Ordering::SeqCst);
        let mut status = state.status.lock().unwrap();
        status.connected = false;
        status.port = None;
    }
}

fn worker_loop(app: AppHandle) {
    let backoffs = [1u64, 2, 5, 10];
    let mut backoff_index = 0usize;
    let mut port: Option<Box<dyn SerialPort>> = None;
    let mut orientation = Orientation::Portrait;
    let mut applied_brightness: u8 = TurzxSettings::default().brightness;
    let mut previous_frame: Option<Vec<u8>> = None;
    let mut page_index = 0usize;
    let mut last_page_switch = Instant::now();

    loop {
        let state = match app.try_state::<TurzxState>() {
            Some(state) => state,
            None => break,
        };
        if !state.running.load(Ordering::Relaxed) {
            break;
        }
        let settings = state.settings.lock().unwrap().clone();
        if !settings.enabled {
            std::thread::sleep(Duration::from_millis(WORKER_TICK_MS));
            continue;
        }

        // 연결 보장
        if port.is_none() {
            let detected = device::detect(Some(&settings.port));
            let Some(candidate) = detected else {
                set_status(&app, |status| {
                    status.connected = false;
                    status.last_error = Some("TURZX 후보 포트를 찾을 수 없습니다".into());
                    status.candidates = device::list_candidates();
                });
                let wait = if settings.auto_reconnect {
                    backoffs[backoff_index.min(backoffs.len() - 1)]
                } else {
                    5
                };
                backoff_index = (backoff_index + 1).min(backoffs.len() - 1);
                std::thread::sleep(Duration::from_secs(wait));
                continue;
            };
            match open_port(&candidate.port).and_then(|mut opened| {
                let model = handshake(&mut opened)?;
                let orientation = configure(&mut opened, model, &settings)?;
                Ok((opened, model, orientation))
            }) {
                Ok((opened, model, configured_orientation)) => {
                    port = Some(opened);
                    orientation = configured_orientation;
                    applied_brightness = settings.brightness;
                    previous_frame = None;
                    backoff_index = 0;
                    let resolution = format!("{}x{}", orientation.size().0, orientation.size().1);
                    set_status(&app, |status| {
                        status.connected = true;
                        status.port = Some(candidate.port.clone());
                        status.device = Some(settings.device_label.clone());
                        status.model = Some(model.label().to_string());
                        status.resolution = resolution;
                        status.orientation = format!("{:?}", orientation).to_lowercase();
                        status.last_error = None;
                    });
                }
                Err(error) => {
                    set_status(&app, |status| {
                        status.connected = false;
                        status.last_error = Some(error.to_string());
                    });
                    let wait = if settings.auto_reconnect {
                        backoffs[backoff_index.min(backoffs.len() - 1)]
                    } else {
                        5
                    };
                    backoff_index = (backoff_index + 1).min(backoffs.len() - 1);
                    std::thread::sleep(Duration::from_secs(wait));
                    continue;
                }
            }
        }

        // 표시 설정(방향/밝기) 변경은 재연결 없이 즉시 반영한다.
        let next_orientation = effective_orientation(&settings);
        if port.is_some()
            && (next_orientation != orientation || settings.brightness != applied_brightness)
        {
            if let Some(active) = port.as_mut() {
                let _ = active.write_all(&protocol::orientation_packet(next_orientation));
                let _ = active.write_all(&protocol::brightness_packet(settings.brightness));
                let _ = active.flush();
                applied_brightness = settings.brightness;
                if next_orientation != orientation {
                    previous_frame = None;
                }
                orientation = next_orientation;
            }
        }

        // 페이지 선택
        let (width, height) = orientation.size();
        let page = match settings.page_mode.as_str() {
            "runtime" => "runtime",
            "api" => "api",
            "rotate" => {
                let interval = settings.page_rotation_secs.clamp(5, 60);
                if last_page_switch.elapsed().as_secs() >= interval {
                    page_index = 1 - page_index;
                    last_page_switch = Instant::now();
                }
                if page_index == 0 {
                    "runtime"
                } else {
                    "api"
                }
            }
            _ => "single",
        };

        // 렌더 + 전송
        let started = Instant::now();
        let snapshot = build_snapshot(&app);
        let frame = renderer::render(&snapshot, page, orientation);
        let width_px = width as usize;
        let height_px = height as usize;

        let outcome = (|| -> Result<(usize, &'static str), AppError> {
            let active = port.as_mut().ok_or_else(|| AppError::Io("포트 없음".into()))?;
            let rect = match previous_frame.as_deref() {
                // 첫 프레임(또는 재연결 직후)은 전체 전송
                None => Some((0u16, 0u16, width - 1, height - 1)),
                Some(previous) => protocol::dirty_rect(previous, &frame, width_px, height_px, TILE_PX),
            };
            match rect {
                None => Ok((0, "unchanged")),
                Some((x0, y0, x1, y1)) => {
                    let kind = if x0 == 0 && y0 == 0 && x1 as usize + 1 == width_px && y1 as usize + 1 == height_px {
                        "full"
                    } else {
                        "partial"
                    };
                    let sent = send_region(
                        active,
                        &frame,
                        width_px,
                        x0 as usize,
                        y0 as usize,
                        x1 as usize,
                        y1 as usize,
                    )?;
                    Ok((sent, kind))
                }
            }
        })();

        match outcome {
            Ok((sent, kind)) => {
                previous_frame = Some(frame.clone());
                let elapsed = started.elapsed().as_millis() as u64;
                if let Some(state) = app.try_state::<TurzxState>() {
                    *state.frame.lock().unwrap() =
                        Some((frame, width, height, page.to_string()));
                    *state.preview.lock().unwrap() = Some(TurzxPreview {
                        width,
                        height,
                        page: page.to_string(),
                        mode: "live".into(),
                        rgb_base64: base64_encode(
                            &state.frame.lock().unwrap().as_ref().map(|entry| entry.0.clone()).unwrap_or_default(),
                        ),
                    });
                }
                set_status(&app, |status| {
                    status.connected = true;
                    status.page = page.to_string();
                    status.last_frame_at = Some(now_iso());
                    status.last_frame_ms = Some(elapsed);
                    status.last_bytes_sent = Some(sent);
                    status.last_update_kind = Some(kind.to_string());
                });
            }
            Err(error) => {
                let message = error.to_string();
                port = None;
                previous_frame = None;
                set_status(&app, |status| {
                    status.connected = false;
                    status.last_error = Some(message);
                });
                if !settings.auto_reconnect {
                    std::thread::sleep(Duration::from_secs(5));
                }
                continue;
            }
        }

        let refresh = settings.refresh_secs.clamp(1, 30);
        let next_frame_at = Instant::now() + Duration::from_secs(refresh);
        let mut slept = 0u64;
        while Instant::now() < next_frame_at {
            if !state.running.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            slept += 100;
            if slept > refresh * 1_000 + 5_000 {
                break;
            }
        }
    }

    // 종료 시 포트 해제
    drop(port);
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn turzx_settings(app: AppHandle) -> TurzxSettings {
    read_settings(&app)
}

#[tauri::command]
pub fn turzx_save_settings(app: AppHandle, settings: TurzxSettings) -> Result<TurzxSettings, AppError> {
    let sanitized = TurzxSettings {
        enabled: settings.enabled,
        port: settings.port.trim().to_string(),
        orientation: match settings.orientation.as_str() {
            "landscape" => "landscape".into(),
            _ => "portrait".into(),
        },
        rotation: match settings.rotation {
            90 | 180 | 270 => settings.rotation,
            _ => 0,
        },
        brightness: settings.brightness.min(100),
        refresh_secs: settings.refresh_secs.clamp(1, 30),
        page_mode: match settings.page_mode.as_str() {
            "runtime" | "api" | "rotate" => settings.page_mode.clone(),
            _ => "single".into(),
        },
        page_rotation_secs: settings.page_rotation_secs.clamp(5, 60),
        auto_reconnect: settings.auto_reconnect,
        launch_with_app: settings.launch_with_app,
        device_label: if settings.device_label.trim().is_empty() {
            "TURZX 3.5\"".into()
        } else {
            settings.device_label.clone()
        },
    };
    write_settings(&app, &sanitized)?;
    if let Some(state) = app.try_state::<TurzxState>() {
        *state.settings.lock().unwrap() = sanitized.clone();
    }
    if sanitized.enabled {
        start_worker(&app);
    } else {
        stop_worker(&app);
    }
    Ok(sanitized)
}

#[tauri::command]
pub fn turzx_status(app: AppHandle) -> TurzxStatus {
    let settings = read_settings(&app);
    let mut status = app
        .try_state::<TurzxState>()
        .map(|state| state.status.lock().unwrap().clone())
        .unwrap_or_default();
    status.enabled = settings.enabled;
    status.candidates = device::list_candidates();
    if status.orientation.is_empty() {
        status.orientation = format!("{:?}", effective_orientation(&settings)).to_lowercase();
    }
    if status.resolution.is_empty() {
        let (width, height) = effective_orientation(&settings).size();
        status.resolution = format!("{width}x{height}");
    }
    status
}

#[tauri::command]
pub fn turzx_detect() -> Vec<PortCandidate> {
    device::list_candidates()
}

#[tauri::command]
pub fn turzx_test(app: AppHandle) -> TurzxStatus {
    let settings = read_settings(&app);
    let mut status = turzx_status(app.clone());
    // 워커가 이미 포트를 점유하며 연결 중이면 다시 열지 않고 현재 상태를 결과로 돌려준다.
    if status.connected {
        return status;
    }
    let Some(candidate) = device::detect(Some(&settings.port)) else {
        status.connected = false;
        status.last_error = Some("TURZX 후보 포트를 찾을 수 없습니다".into());
        return status;
    };
    match open_port(&candidate.port).and_then(|mut port| {
        let model = handshake(&mut port)?;
        let orientation = configure(&mut port, model, &settings)?;
        Ok((model, orientation))
    }) {
        Ok((model, orientation)) => {
            status.connected = true;
            status.port = Some(candidate.port);
            status.device = Some(settings.device_label.clone());
            status.model = Some(model.label().to_string());
            status.orientation = format!("{orientation:?}").to_lowercase();
            let (width, height) = orientation.size();
            status.resolution = format!("{width}x{height}");
            status.last_error = None;
        }
        Err(error) => {
            status.connected = false;
            status.last_error = Some(error.to_string());
        }
    }
    status
}

#[tauri::command]
pub fn turzx_connect(app: AppHandle) -> Result<TurzxSettings, AppError> {
    let mut settings = read_settings(&app);
    settings.enabled = true;
    turzx_save_settings(app, settings)
}

#[tauri::command]
pub fn turzx_disconnect(app: AppHandle) -> Result<TurzxSettings, AppError> {
    let mut settings = read_settings(&app);
    settings.enabled = false;
    turzx_save_settings(app, settings)
}

#[tauri::command]
pub fn turzx_preview(app: AppHandle, mode: Option<String>, page: Option<String>) -> TurzxPreview {
    let mode = mode.unwrap_or_else(|| "live".into());
    let settings = read_settings(&app);
    let orientation = effective_orientation(&settings);
    let page = page.unwrap_or_else(|| {
        match settings.page_mode.as_str() {
            "runtime" | "api" => settings.page_mode.clone(),
            _ => "single".into(),
        }
    });
    if mode == "sample" {
        let snapshot = renderer::sample_snapshot();
        let frame = renderer::render(&snapshot, &page, orientation);
        let (width, height) = orientation.size();
        return TurzxPreview {
            width,
            height,
            page,
            mode,
            rgb_base64: base64_encode(&frame),
        };
    }
    if let Some(state) = app.try_state::<TurzxState>() {
        let frame = state.frame.lock().unwrap();
        if let Some((rgb, width, height, last_page)) = frame.as_ref() {
            return TurzxPreview {
                width: *width,
                height: *height,
                page: last_page.clone(),
                mode: "live".into(),
                rgb_base64: base64_encode(rgb),
            };
        }
    }
    // 라이브 프레임이 없으면 현재 텔레메트리로 즉시 렌더한다(프리뷰는 하드웨어와 무관).
    let snapshot = build_snapshot(&app);
    let frame = renderer::render(&snapshot, &page, orientation);
    let (width, height) = orientation.size();
    TurzxPreview {
        width,
        height,
        page,
        mode: "live".into(),
        rgb_base64: base64_encode(&frame),
    }
}

#[tauri::command]
pub fn turzx_export_preview(app: AppHandle, path: Option<String>) -> Result<String, AppError> {
    let preview = turzx_preview(app, Some("sample".into()), None);
    use base64::Engine;
    let rgb = base64::engine::general_purpose::STANDARD
        .decode(preview.rgb_base64)
        .map_err(|error| AppError::Io(error.to_string()))?;
    let target = path
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("turzx-ai-dashboard-preview.png"));
    protocol::write_png(&target, preview.width as usize, preview.height as usize, &rgb)
        .map_err(|error| AppError::Io(error.to_string()))?;
    Ok(target.to_string_lossy().to_string())
}

pub fn shutdown(app: &AppHandle) {
    stop_worker(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_settings_ranges() {
        assert_eq!(TurzxSettings::default().refresh_secs, 2);
        let settings = TurzxSettings {
            rotation: 45,
            page_mode: "weird".into(),
            refresh_secs: 999,
            page_rotation_secs: 1,
            ..Default::default()
        };
        let clamped = TurzxSettings {
            rotation: match settings.rotation { 90 | 180 | 270 => settings.rotation, _ => 0 },
            refresh_secs: settings.refresh_secs.clamp(1, 30),
            page_rotation_secs: settings.page_rotation_secs.clamp(5, 60),
            ..settings
        };
        assert_eq!(clamped.rotation, 0);
        assert_eq!(clamped.refresh_secs, 30);
        assert_eq!(clamped.page_rotation_secs, 5);
    }

    #[test]
    fn snapshot_never_contains_secrets() {
        let snapshot = renderer::sample_snapshot();
        let text = serde_json::to_string(&snapshot).unwrap();
        assert!(!text.to_lowercase().contains("key"));
        assert!(!text.contains("Bearer"));
    }
}
