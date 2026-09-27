//! TURZX 표시용 스냅샷 DTO + 대시보드 렌더러.
//!
//! - 원격 API를 호출하지 않는다(기존 GPU 캐시/DB 스냅샷 소비 전용).
//! - 비밀 값(키/토큰/명령줄/환경변수)은 절대 포함하지 않는다.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::gdi::Canvas;
use super::protocol::{truncate, Orientation};

pub const COLOR_BG: (u8, u8, u8) = (12, 17, 23);
pub const COLOR_TEXT: (u8, u8, u8) = (233, 240, 246);
pub const COLOR_MUTED: (u8, u8, u8) = (146, 163, 180);
pub const COLOR_FAINT: (u8, u8, u8) = (92, 108, 124);
pub const COLOR_OK: (u8, u8, u8) = (74, 222, 128);
pub const COLOR_WARN: (u8, u8, u8) = (245, 190, 80);
pub const COLOR_DANGER: (u8, u8, u8) = (235, 96, 118);
pub const COLOR_ACCENT: (u8, u8, u8) = (0, 229, 255);
pub const COLOR_BAR_BG: (u8, u8, u8) = (40, 52, 66);
pub const COLOR_SEPARATOR: (u8, u8, u8) = (44, 58, 74);

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
pub const WORKLOAD_LIMIT: usize = 3;
pub const API_LIMIT: usize = 4;

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DisplayGpu {
    pub name: String,
    pub utilization_percent: Option<u32>,
    pub vram_used_bytes: Option<u64>,
    pub vram_total_bytes: Option<u64>,
    pub temperature_c: Option<u32>,
    pub power_watts: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DisplayWorkload {
    pub service: String,
    pub model: Option<String>,
    pub vram_bytes: Option<u64>,
    pub kind: String,
    pub confidence: String,
    pub cpu_only: bool,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DisplayApiRow {
    pub id: String,
    pub label: String,
    /// 남은 비율(%). 사용(%)이 아니라 남은 기준으로만 채운다.
    pub primary_remaining_percent: Option<f64>,
    pub primary_label: String,
    pub secondary_remaining_percent: Option<f64>,
    pub secondary_label: String,
    /// 금액 표기용(있을 때만): "$6.2 / $10"
    pub amount_text: Option<String>,
    pub status: String,
    pub age_secs: i64,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TurzxDisplaySnapshot {
    pub timestamp: String,
    pub gpu: DisplayGpu,
    pub workloads: Vec<DisplayWorkload>,
    pub api_usage: Vec<DisplayApiRow>,
}

// ---------------------------------------------------------------- formatting helpers

pub fn format_gib(bytes: Option<u64>) -> String {
    match bytes {
        Some(value) => format!("{:.1}", value as f64 / GIB),
        None => "N/A".to_string(),
    }
}

pub fn vram_percent(gpu: &DisplayGpu) -> Option<f64> {
    match (gpu.vram_used_bytes, gpu.vram_total_bytes) {
        (Some(used), Some(total)) if total > 0 => Some(used as f64 / total as f64 * 100.0),
        _ => None,
    }
}

/// 남은 비율 색상: <10% 위험, <20% 경고.
pub fn remaining_color(percent: Option<f64>) -> (u8, u8, u8) {
    match percent {
        None => COLOR_FAINT,
        Some(value) if value < 10.0 => COLOR_DANGER,
        Some(value) if value < 20.0 => COLOR_WARN,
        Some(_) => COLOR_TEXT,
    }
}

/// VRAM 사용률 색상: >90% 위험, >75% 경고.
pub fn vram_color(percent: Option<f64>) -> (u8, u8, u8) {
    match percent {
        None => COLOR_FAINT,
        Some(value) if value > 90.0 => COLOR_DANGER,
        Some(value) if value > 75.0 => COLOR_WARN,
        Some(_) => COLOR_ACCENT,
    }
}

pub fn percent_text(percent: Option<f64>) -> String {
    match percent {
        Some(value) => format!("{}%", value.round() as i64),
        None => "N/A".to_string(),
    }
}

/// 금액 문자열("$6.2 / $10")을 만든다. 통화/숫자가 없으면 None.
pub fn amount_text(used: Option<f64>, limit: Option<f64>, currency: Option<&str>) -> Option<String> {
    let symbol = match currency.unwrap_or("").to_uppercase().as_str() {
        "USD" => "$",
        "KRW" => "₩",
        "" => "",
        _ => "",
    };
    match (used, limit) {
        (Some(used), Some(limit)) if limit > 0.0 => {
            Some(format!("{symbol}{used:.1} / {symbol}{limit:.1}"))
        }
        _ => None,
    }
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

fn remaining_from_used(used: Option<f64>) -> Option<f64> {
    used.map(|value| (100.0 - value).clamp(0.0, 100.0))
}

/// 저장된 monitor_snapshots(details_json)를 표시용 행으로 정규화한다.
pub fn normalize_monitor_row(monitor: &str, details_json: Option<&str>) -> Option<DisplayApiRow> {
    let details: Value = details_json.and_then(|text| serde_json::from_str(text).ok())?;
    let mut row = DisplayApiRow {
        id: monitor.to_string(),
        label: monitor_label(monitor),
        primary_label: String::new(),
        secondary_label: String::new(),
        status: "ok".into(),
        ..Default::default()
    };
    match monitor {
        "codex" => {
            row.primary_label = "Wk".into();
            row.secondary_label = "5h".into();
            row.primary_remaining_percent =
                remaining_from_used(number(details.get("primaryUsedPercent")));
            row.secondary_remaining_percent =
                remaining_from_used(number(details.get("secondaryUsedPercent")));
        }
        "grok" => {
            row.primary_label = "Mo".into();
            let used = number(details.get("usedPercent"));
            let limit = number(details.get("monthlyLimit"));
            row.primary_remaining_percent = remaining_from_used(used);
            if limit.is_none() {
                row.amount_text = None;
            }
        }
        "grok-build" => {
            row.primary_label = "Wk".into();
            row.primary_remaining_percent = number(details.get("remainingPercent"));
        }
        "antigravity" => {
            row.primary_label = "Gem".into();
            row.secondary_label = "Cl".into();
            row.primary_remaining_percent = number(details.get("geminiRemainingPercent"));
            row.secondary_remaining_percent = number(details.get("claudeRemainingPercent"));
        }
        "opencode" => {
            // 롤링/주간/월간 중 가장 빠듯한(남은 값이 가장 작은) 두 창을 보여준다.
            let mut windows: Vec<(String, Option<f64>)> = Vec::new();
            for (key, label) in [("rolling", "Ro"), ("weekly", "Wk"), ("monthly", "Mo")] {
                let used = details.get(key).and_then(|entry| entry.get("percent"));
                windows.push((label.to_string(), remaining_from_used(number(used))));
            }
            windows.retain(|(_, remaining)| remaining.is_some());
            windows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            if let Some((label, remaining)) = windows.first() {
                row.primary_label = label.clone();
                row.primary_remaining_percent = *remaining;
            }
            if let Some((label, remaining)) = windows.get(1) {
                row.secondary_label = label.clone();
                row.secondary_remaining_percent = *remaining;
            }
        }
        _ => return None,
    }
    Some(row)
}

pub fn monitor_label(monitor: &str) -> String {
    match monitor {
        "codex" => "Codex".into(),
        "grok" => "Grok".into(),
        "grok-build" => "Grok Build".into(),
        "antigravity" => "Antigravity".into(),
        "opencode" => "OpenCode".into(),
        other => other.to_string(),
    }
}

/// UsageSnapshot(키 기반 어댑터) 행 정규화.
pub fn normalize_usage_row(
    adapter: &str,
    label: &str,
    used: Option<f64>,
    limit: Option<f64>,
    remaining: Option<f64>,
    currency: Option<&str>,
) -> DisplayApiRow {
    let remaining_percent = match (remaining, limit) {
        (Some(remaining), Some(limit)) if limit > 0.0 => {
            Some((remaining / limit * 100.0).clamp(0.0, 100.0))
        }
        _ => None,
    };
    DisplayApiRow {
        id: adapter.to_string(),
        label: label.to_string(),
        primary_remaining_percent: remaining_percent,
        primary_label: String::new(),
        secondary_remaining_percent: None,
        secondary_label: String::new(),
        amount_text: amount_text(used, limit, currency),
        status: "ok".into(),
        age_secs: 0,
    }
}

/// 표시 우선순위: 남은 비율이 낮은 것 → 최신(age 작은 것) 순. 최대 API_LIMIT개.
pub fn prioritize_api_rows(mut rows: Vec<DisplayApiRow>) -> Vec<DisplayApiRow> {
    rows.sort_by(|a, b| {
        let a_remaining = a.primary_remaining_percent.unwrap_or(f64::INFINITY);
        let b_remaining = b.primary_remaining_percent.unwrap_or(f64::INFINITY);
        a_remaining
            .partial_cmp(&b_remaining)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.age_secs.cmp(&b.age_secs))
    });
    rows.truncate(API_LIMIT);
    rows
}

/// 워크로드 정렬: 관리형 서비스 → VRAM 큰 순 → unknown. 최대 WORKLOAD_LIMIT.
pub fn prioritize_workloads(mut rows: Vec<DisplayWorkload>) -> (Vec<DisplayWorkload>, usize) {
    rows.sort_by(|a, b| {
        let rank = |kind: &str| match kind {
            "managed_service" => 0,
            "ollama" | "llama_cpp" | "vllm" | "comfyui" => 1,
            _ => 2,
        };
        rank(&a.kind)
            .cmp(&rank(&b.kind))
            .then_with(|| b.vram_bytes.unwrap_or(0).cmp(&a.vram_bytes.unwrap_or(0)))
    });
    let overflow = rows.len().saturating_sub(WORKLOAD_LIMIT);
    rows.truncate(WORKLOAD_LIMIT);
    (rows, overflow)
}

// ---------------------------------------------------------------- rendering

fn local_hhmm() -> String {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct SystemTime {
            year: u16,
            month: u16,
            day_of_week: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            milliseconds: u16,
        }
        extern "system" {
            fn GetLocalTime(time: *mut SystemTime);
        }
        let mut time = SystemTime {
            year: 0,
            month: 0,
            day_of_week: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            milliseconds: 0,
        };
        unsafe { GetLocalTime(&mut time) };
        format!("{:02}:{:02}", time.hour, time.minute)
    }
    #[cfg(not(windows))]
    {
        "00:00".to_string()
    }
}

pub fn render(snapshot: &TurzxDisplaySnapshot, page: &str, orientation: Orientation) -> Vec<u8> {
    let (width, height) = orientation.size();
    let mut canvas = Canvas::new(width, height);
    canvas.rect(0, 0, width as i32, height as i32, COLOR_BG);

    let mut y_pos = 10;
    let show_gpu = page != "api";
    let show_api = page != "runtime";

    if show_gpu {
        y_pos = render_gpu_section(&mut canvas, snapshot, y_pos, width as i32);
    }
    if show_api {
        render_api_section(&mut canvas, snapshot, y_pos, width as i32, height as i32);
    }

    // footer
    let footer_y = height as i32 - 22;
    canvas.line(10, footer_y - 4, width as i32 - 10, footer_y - 4, COLOR_SEPARATOR);
    canvas.text(&local_hhmm(), 12, footer_y, 14, COLOR_MUTED, false);
    let (status_text, status_color) = if snapshot.gpu.name.is_empty() {
        ("N/A", COLOR_FAINT)
    } else {
        ("CONNECTED", COLOR_OK)
    };
    canvas.text_right(status_text, width as i32 - 12, footer_y, 13, status_color, false);
    canvas.circle(width as i32 - 96, footer_y + 8, 4, status_color);

    canvas.finish().to_vec()
}

fn render_gpu_section(canvas: &mut Canvas, snapshot: &TurzxDisplaySnapshot, mut y_pos: i32, width: i32) -> i32 {
    let gpu = &snapshot.gpu;
    let name = if gpu.name.is_empty() {
        "GPU 없음".to_string()
    } else {
        gpu.name.replace("NVIDIA GeForce ", "")
    };
    canvas.text(&truncate(&name, 22), 12, y_pos, 20, COLOR_TEXT, true);
    y_pos += 26;

    // GPU util · temp · power 한 줄
    let util_text = format!(
        "GPU {}",
        gpu.utilization_percent
            .map(|value| format!("{value}%"))
            .unwrap_or_else(|| "N/A".into())
    );
    canvas.text(&util_text, 12, y_pos, 18, COLOR_ACCENT, true);
    let mut right = width - 12;
    if let Some(power) = gpu.power_watts {
        let text = format!("{power:.0}W");
        canvas.text_right(&text, right, y_pos, 15, COLOR_MUTED, false);
        right -= canvas.text_width(&text, 15, false) + 12;
    }
    if let Some(temp) = gpu.temperature_c {
        let text = format!("{temp}°C");
        canvas.text_right(&text, right, y_pos, 15, COLOR_MUTED, false);
    }
    y_pos += 26;

    // VRAM
    let percent = vram_percent(gpu);
    let vram_text = format!(
        "VRAM {} / {} GB",
        format_gib(gpu.vram_used_bytes),
        format_gib(gpu.vram_total_bytes)
    );
    canvas.text(&vram_text, 12, y_pos, 15, COLOR_TEXT, false);
    canvas.text_right(&percent_text(percent), width - 12, y_pos, 15, vram_color(percent), true);
    y_pos += 22;
    canvas.bar(12, y_pos, width - 24, 12, percent.unwrap_or(0.0), vram_color(percent), COLOR_BAR_BG);
    y_pos += 24;

    canvas.line(10, y_pos, width - 10, y_pos, COLOR_SEPARATOR);
    y_pos += 8;
    canvas.text("GPU MODELS", 12, y_pos, 13, COLOR_MUTED, true);
    y_pos += 20;

    let (rows, overflow) = prioritize_workloads(snapshot.workloads.clone());
    if rows.is_empty() {
        canvas.text("(GPU 사용 중인 AI 워크로드 없음)", 12, y_pos, 13, COLOR_FAINT, false);
        y_pos += 20;
    }
    for workload in rows {
        canvas.circle(16, y_pos + 8, 4, COLOR_ACCENT);
        let vram = format!("{} GB", format_gib(workload.vram_bytes));
        canvas.text(&truncate(&workload.service, 18), 27, y_pos, 15, COLOR_TEXT, true);
        canvas.text_right(&vram, width - 12, y_pos, 14, COLOR_MUTED, false);
        y_pos += 20;
        if let Some(model) = workload.model.as_deref() {
            canvas.text(&truncate(model, 26), 33, y_pos, 12, COLOR_MUTED, false);
            y_pos += 17;
        }
    }
    if overflow > 0 {
        let overflow_vram: u64 = 0;
        let _ = overflow_vram;
        canvas.text(&format!("+{overflow} more"), 33, y_pos, 12, COLOR_FAINT, false);
        y_pos += 17;
    }
    y_pos += 6;
    y_pos
}

fn render_api_section(
    canvas: &mut Canvas,
    snapshot: &TurzxDisplaySnapshot,
    mut y_pos: i32,
    width: i32,
    height: i32,
) {
    if y_pos > height - 60 {
        return;
    }
    canvas.line(10, y_pos, width - 10, y_pos, COLOR_SEPARATOR);
    y_pos += 8;
    canvas.text("API USAGE", 12, y_pos, 13, COLOR_MUTED, true);
    canvas.text_right("left", width - 12, y_pos, 11, COLOR_FAINT, false);
    y_pos += 20;

    if snapshot.api_usage.is_empty() {
        canvas.text("(사용량 스냅샷 없음)", 12, y_pos, 13, COLOR_FAINT, false);
        return;
    }
    for row in &snapshot.api_usage {
        if y_pos > height - 40 {
            break;
        }
        canvas.text(&truncate(&row.label, 16), 12, y_pos, 14, COLOR_TEXT, false);
        let stale = if row.age_secs > 900 {
            format!(" ·{}m", row.age_secs / 60)
        } else {
            String::new()
        };
        let right = width - 12;
        let secondary = row
            .secondary_remaining_percent
            .map(|value| format!("{} {}", row.secondary_label, percent_text(Some(value))))
            .unwrap_or_default();
        let primary = match (row.primary_remaining_percent, row.amount_text.as_deref()) {
            (Some(value), _) => format!("{} {}", row.primary_label, percent_text(Some(value))),
            (None, Some(amount)) => amount.to_string(),
            _ => "N/A".to_string(),
        };
        let text = format!(
            "{}{}{}",
            primary,
            if secondary.is_empty() { String::new() } else { format!("  {secondary}") },
            stale
        );
        canvas.text_right(&text, right, y_pos, 13, remaining_color(row.primary_remaining_percent), false);
        y_pos += 18;
    }
}

// ---------------------------------------------------------------- 샘플(프리뷰용)

pub fn sample_snapshot() -> TurzxDisplaySnapshot {
    TurzxDisplaySnapshot {
        timestamp: "sample".into(),
        gpu: DisplayGpu {
            name: "NVIDIA GeForce RTX 5090".into(),
            utilization_percent: Some(37),
            vram_used_bytes: Some((11.7 * GIB) as u64),
            vram_total_bytes: Some((31.8 * GIB) as u64),
            temperature_c: Some(51),
            power_watts: Some(82.0),
        },
        workloads: vec![
            DisplayWorkload {
                service: "Ollama".into(),
                model: Some("qwen3.5:9b".into()),
                vram_bytes: None,
                kind: "ollama".into(),
                confidence: "medium".into(),
                cpu_only: false,
            },
            DisplayWorkload {
                service: "Laya".into(),
                model: Some("english · multilingual · typed-decisions".into()),
                vram_bytes: Some((1.4 * GIB) as u64),
                kind: "managed_service".into(),
                confidence: "high".into(),
                cpu_only: false,
            },
            DisplayWorkload {
                service: "ComfyUI".into(),
                model: Some("sd_xl_base_1.0.safetensors".into()),
                vram_bytes: Some((0.3 * GIB) as u64),
                kind: "comfyui".into(),
                confidence: "low".into(),
                cpu_only: false,
            },
        ],
        api_usage: vec![
            DisplayApiRow {
                id: "codex".into(),
                label: "Codex".into(),
                primary_remaining_percent: Some(42.0),
                primary_label: "Wk".into(),
                secondary_remaining_percent: Some(68.0),
                secondary_label: "5h".into(),
                amount_text: None,
                status: "ok".into(),
                age_secs: 90,
            },
            DisplayApiRow {
                id: "antigravity".into(),
                label: "Antigravity".into(),
                primary_remaining_percent: Some(73.0),
                primary_label: "Gem".into(),
                secondary_remaining_percent: Some(54.0),
                secondary_label: "Cl".into(),
                amount_text: None,
                status: "ok".into(),
                age_secs: 120,
            },
            DisplayApiRow {
                id: "grok".into(),
                label: "Grok".into(),
                primary_remaining_percent: None,
                primary_label: "Mo".into(),
                secondary_remaining_percent: None,
                secondary_label: String::new(),
                amount_text: Some("$6.2 / $10".into()),
                status: "ok".into(),
                age_secs: 60,
            },
            DisplayApiRow {
                id: "opencode".into(),
                label: "OpenCode".into(),
                primary_remaining_percent: Some(37.0),
                primary_label: "Ro".into(),
                secondary_remaining_percent: None,
                secondary_label: String::new(),
                amount_text: None,
                status: "ok".into(),
                age_secs: 30,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_full() -> TurzxDisplaySnapshot {
        sample_snapshot()
    }

    #[test]
    fn renders_portrait_and_landscape_sizes() {
        let snapshot = snapshot_full();
        let portrait = render(&snapshot, "single", Orientation::Portrait);
        assert_eq!(portrait.len(), 320 * 480 * 3);
        let landscape = render(&snapshot, "single", Orientation::Landscape);
        assert_eq!(landscape.len(), 480 * 320 * 3);
        // 배경색이 실제로 칠해졌는지(첫 픽셀)
        assert_eq!(&portrait[0..3], &[COLOR_BG.0, COLOR_BG.1, COLOR_BG.2]);
    }

    #[test]
    fn renders_without_gpu_or_api() {
        let snapshot = TurzxDisplaySnapshot::default();
        let frame = render(&snapshot, "single", Orientation::Portrait);
        assert_eq!(frame.len(), 320 * 480 * 3);
    }

    #[test]
    fn renders_each_page_mode() {
        let snapshot = snapshot_full();
        for page in ["single", "runtime", "api"] {
            let frame = render(&snapshot, page, Orientation::Portrait);
            assert_eq!(frame.len(), 320 * 480 * 3, "page {page}");
        }
    }

    #[test]
    fn formats_missing_vram_as_na() {
        assert_eq!(format_gib(None), "N/A");
        assert_eq!(format_gib(Some(0)), "0.0");
        assert_eq!(format_gib(Some(1024 * 1024 * 1024)), "1.0");
        let gpu = DisplayGpu::default();
        assert_eq!(vram_percent(&gpu), None);
        assert_eq!(percent_text(None), "N/A");
    }

    #[test]
    fn normalizes_codex_remaining_semantics() {
        let details = serde_json::json!({
            "primaryUsedPercent": 58.0,
            "secondaryUsedPercent": 32.0
        })
        .to_string();
        let row = normalize_monitor_row("codex", Some(&details)).unwrap();
        assert_eq!(row.primary_label, "Wk");
        assert_eq!(row.primary_remaining_percent, Some(42.0));
        assert_eq!(row.secondary_label, "5h");
        assert_eq!(row.secondary_remaining_percent, Some(68.0));
    }

    #[test]
    fn normalizes_antigravity_and_opencode() {
        let antigravity = serde_json::json!({
            "geminiRemainingPercent": 61.0,
            "claudeRemainingPercent": 48.0
        })
        .to_string();
        let row = normalize_monitor_row("antigravity", Some(&antigravity)).unwrap();
        assert_eq!(row.primary_remaining_percent, Some(61.0));
        assert_eq!(row.secondary_remaining_percent, Some(48.0));

        let opencode = serde_json::json!({
            "rolling": { "percent": 5.0 },
            "weekly": { "percent": 63.0 },
            "monthly": { "percent": 30.0 }
        })
        .to_string();
        let row = normalize_monitor_row("opencode", Some(&opencode)).unwrap();
        // 남은 값: rolling 95%, weekly 37%, monthly 70% → 빠듯한 두 창(Wk 37, Mo 70)
        assert_eq!(row.primary_label, "Wk");
        assert_eq!(row.primary_remaining_percent, Some(37.0));
        assert_eq!(row.secondary_label, "Mo");
        assert_eq!(row.secondary_remaining_percent, Some(70.0));
    }

    #[test]
    fn normalizes_usage_amounts() {
        let row = normalize_usage_row("openrouter", "오픈라우터", Some(3.8), Some(10.0), Some(6.2), Some("USD"));
        assert_eq!(row.amount_text.as_deref(), Some("$3.8 / $10.0"));
        assert_eq!(row.primary_remaining_percent, Some(62.0));
        let no_limit = normalize_usage_row("deepseek", "딥시크", None, None, Some(5.59), Some("USD"));
        assert_eq!(no_limit.primary_remaining_percent, None);
        assert_eq!(no_limit.amount_text, None);
    }

    #[test]
    fn prioritizes_low_remaining_and_limits_rows() {
        let rows = vec![
            DisplayApiRow { id: "a".into(), label: "A".into(), primary_remaining_percent: Some(80.0), age_secs: 5, ..Default::default() },
            DisplayApiRow { id: "b".into(), label: "B".into(), primary_remaining_percent: Some(12.0), age_secs: 900, ..Default::default() },
            DisplayApiRow { id: "c".into(), label: "C".into(), primary_remaining_percent: None, age_secs: 1, ..Default::default() },
            DisplayApiRow { id: "d".into(), label: "D".into(), primary_remaining_percent: Some(30.0), age_secs: 60, ..Default::default() },
            DisplayApiRow { id: "e".into(), label: "E".into(), primary_remaining_percent: Some(55.0), age_secs: 2, ..Default::default() },
        ];
        let ordered = prioritize_api_rows(rows);
        assert_eq!(ordered.len(), API_LIMIT);
        assert_eq!(ordered[0].id, "b");
        assert_eq!(ordered[1].id, "d");
        assert_eq!(ordered[2].id, "e");
        assert_eq!(ordered[3].id, "a");
    }

    #[test]
    fn prioritizes_managed_service_and_aggregates_overflow() {
        let rows = vec![
            DisplayWorkload { service: "Unknown".into(), kind: "unknown_ai".into(), vram_bytes: Some(9 << 30), ..Default::default() },
            DisplayWorkload { service: "Laya".into(), kind: "managed_service".into(), vram_bytes: None, ..Default::default() },
            DisplayWorkload { service: "Ollama".into(), kind: "ollama".into(), vram_bytes: Some(7 << 30), ..Default::default() },
            DisplayWorkload { service: "vLLM".into(), kind: "vllm".into(), vram_bytes: Some(1 << 30), ..Default::default() },
        ];
        let (kept, overflow) = prioritize_workloads(rows);
        assert_eq!(kept.len(), WORKLOAD_LIMIT);
        assert_eq!(kept[0].service, "Laya");
        assert_eq!(kept[1].service, "Ollama");
        assert_eq!(kept[2].service, "vLLM");
        assert_eq!(overflow, 1);
    }

    #[test]
    fn colors_follow_thresholds() {
        assert_eq!(remaining_color(Some(50.0)), COLOR_TEXT);
        assert_eq!(remaining_color(Some(15.0)), COLOR_WARN);
        assert_eq!(remaining_color(Some(5.0)), COLOR_DANGER);
        assert_eq!(remaining_color(None), COLOR_FAINT);
        assert_eq!(vram_color(Some(40.0)), COLOR_ACCENT);
        assert_eq!(vram_color(Some(80.0)), COLOR_WARN);
        assert_eq!(vram_color(Some(95.0)), COLOR_DANGER);
    }

    #[test]
    fn snapshot_has_no_secret_fields() {
        let value = serde_json::to_value(snapshot_full()).unwrap();
        let text = value.to_string();
        for forbidden in ["secret", "token", "apiKey", "api_key", "password", "Bearer", "commandLine", "env"] {
            assert!(!text.contains(forbidden), "leaked field: {forbidden}");
        }
    }
}
