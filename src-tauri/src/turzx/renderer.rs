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
/// 플랫 카드 표면(참조 CSS theme-surface).
pub const COLOR_SURFACE: (u8, u8, u8) = (18, 24, 32);
/// 카드 테두리(참조 CSS theme-border).
pub const COLOR_BORDER: (u8, u8, u8) = (40, 52, 66);
pub const COLOR_SEPARATOR: (u8, u8, u8) = (44, 58, 74);
/// 10~20% 남음(경고) 단계 색.
pub const COLOR_ORANGE: (u8, u8, u8) = (250, 150, 74);
/// 값 텍스트 뒤에 까는 칩(채움/카드 어디서든 대비 보장).
const COLOR_GAUGE_CHIP: (u8, u8, u8) = (6, 9, 13);
/// 칩 테두리(채움 위에서도 칩 경계가 보이게).
const COLOR_CHIP_BORDER: (u8, u8, u8) = (86, 104, 126);

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
pub const WORKLOAD_LIMIT: usize = 3;
/// 컴팩트 API 게이지: 기본 대시보드에 표시하는 최대 행 수(스펙 §8, 4~6).
pub const API_GAUGE_LIMIT: usize = 6;
pub const API_GAUGE_ROW_HEIGHT: i32 = 23;
pub const API_GAUGE_ROW_GAP: i32 = 3;
/// 스펙 §1 고정 우선순위: Codex > Antigravity > Grok(계열) > OpenCode Go.
pub const API_PIN_ORDER: [&str; 5] = ["codex", "antigravity", "grok", "grok-build", "opencode"];

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DisplayGpu {
    pub name: String,
    pub utilization_percent: Option<u32>,
    pub vram_used_bytes: Option<u64>,
    pub vram_total_bytes: Option<u64>,
    pub temperature_c: Option<u32>,
    pub power_watts: Option<f64>,
    #[serde(default)]
    pub ram_used_bytes: Option<u64>,
    #[serde(default)]
    pub ram_total_bytes: Option<u64>,
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
    /// 프로세스의 헤드라인 GPU %(가장 바쁜 엔진). 없으면 N/A.
    #[serde(default)]
    pub gpu_percent: Option<f64>,
    /// ai|game|graphics|browser|video|system|unknown
    #[serde(default)]
    pub classification: String,
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
    pub primary_reset_in_secs: Option<i64>,
    pub secondary_reset_in_secs: Option<i64>,
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

/// VRAM(사용률) 게이지 색(스펙 §11): >90% 위험, >75% 주의.
/// GPU 사용률은 높아도 그 자체로 위험이 아니므로 별도 경고색을 쓰지 않는다.
pub fn vram_gauge_color(percent: Option<f64>) -> (u8, u8, u8) {
    match percent {
        None => COLOR_FAINT,
        Some(value) if value > 90.0 => COLOR_DANGER,
        Some(value) if value > 75.0 => COLOR_WARN,
        Some(_) => COLOR_ACCENT,
    }
}

/// VRAM 사용률 색상: >90% 위험, >75% 경고.


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
fn value_secs(value: Option<&Value>, keys: &[&str], now_secs: i64) -> Option<i64> {
    let value = value?;
    for key in keys {
        if let Some(entry) = value.get(*key) {
            if let Some(number) = entry.as_i64() {
                if key.eq_ignore_ascii_case("resetInSec") || key.to_lowercase().contains("insec") {
                    return Some(number.max(0));
                }
                if number > 1_000_000_000 {
                    return Some((number - now_secs).max(0));
                }
                return Some(number.max(0));
            }
            if let Some(text) = entry.as_str() {
                if let Some(at) = crate::monitors::parse_rfc3339_secs(text) {
                    return Some((at - now_secs).max(0));
                }
            }
        }
    }
    None
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

/// details의 리셋 시각/잔여 초에서 남은 초를 계산한다(테스트용 now 주입).
#[allow(dead_code)]
pub fn reset_in_secs_from(value: &Value, keys: &[&str], now: i64) -> Option<i64> {
    value_secs(Some(value), keys, now)
}

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
            row.primary_reset_in_secs = value_secs(
                Some(&details),
                &["primaryResetAt", "primaryResetInSec"],
                now_secs(),
            );
            row.secondary_reset_in_secs = value_secs(
                Some(&details),
                &["secondaryResetAt", "secondaryResetInSec"],
                now_secs(),
            );
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
            row.primary_reset_in_secs =
                value_secs(Some(&details), &["periodEnd", "resetAt"], now_secs());
        }
        "antigravity" => {
            let now = now_secs();
            let mut weekly: Vec<(String, f64, Option<i64>)> = Vec::new();
            if let Some(groups) = details.get("quotaGroups").and_then(Value::as_array) {
                for group in groups {
                    let name = group
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_lowercase();
                    let label = if name.contains("claude") { "Cl" } else { "Gem" };
                    if let Some(remaining) =
                        group.get("weeklyRemainingPercent").and_then(Value::as_f64)
                    {
                        weekly.push((
                            label.to_string(),
                            remaining,
                            value_secs(Some(group), &["weeklyResetAt"], now),
                        ));
                    }
                }
            }
            if weekly.is_empty() {
                // quotaGroups가 없으면 상위 필드로 폴백(리셋 없음).
                row.primary_label = "Gem".into();
                row.secondary_label = "Cl".into();
                row.primary_remaining_percent = number(details.get("geminiRemainingPercent"));
                row.secondary_remaining_percent = number(details.get("claudeRemainingPercent"));
            } else {
                // 데스크탑 앱의 기본 그룹인 Gemini 주간을 primary로 노출한다(Claude는 secondary).
                let primary_index = weekly
                    .iter()
                    .position(|(label, _, _)| label == "Gem")
                    .unwrap_or(0);
                let (label, remaining, reset) = weekly[primary_index].clone();
                row.primary_label = label;
                row.primary_remaining_percent = Some(remaining);
                row.primary_reset_in_secs = reset;
                if let Some((label, remaining, reset)) =
                    weekly.iter().enumerate().find(|(index, _)| *index != primary_index).map(|(_, entry)| entry)
                {
                    row.secondary_label = label.clone();
                    row.secondary_remaining_percent = Some(*remaining);
                    row.secondary_reset_in_secs = *reset;
                }
            }
        }
        "opencode" => {
            // 주간(Wk) 창을 primary로 노출하고 리셋 시각을 함께 담는다.
            let now = now_secs();
            let window = |key: &str| -> Option<(Option<f64>, Option<i64>)> {
                let entry = details.get(key)?;
                Some((
                    remaining_from_used(number(entry.get("percent"))),
                    value_secs(Some(entry), &["resetsAt", "resetInSec"], now),
                ))
            };
            let mut others: Vec<(String, f64, Option<i64>)> = Vec::new();
            for (key, label) in [("rolling", "Ro"), ("monthly", "Mo")] {
                if let Some((Some(remaining), reset)) = window(key) {
                    others.push((label.to_string(), remaining, reset));
                }
            }
            match window("weekly") {
                Some((Some(remaining), reset)) => {
                    row.primary_label = "Wk".into();
                    row.primary_remaining_percent = Some(remaining);
                    row.primary_reset_in_secs = reset;
                }
                _ => {
                    if let Some((label, remaining, reset)) = others
                        .iter()
                        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                    {
                        row.primary_label = label.clone();
                        row.primary_remaining_percent = Some(*remaining);
                        row.primary_reset_in_secs = *reset;
                    }
                }
            }
            if let Some((label, remaining, reset)) = others
                .iter()
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            {
                row.secondary_label = label.clone();
                row.secondary_remaining_percent = Some(*remaining);
                row.secondary_reset_in_secs = *reset;
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
        primary_reset_in_secs: None,
        secondary_reset_in_secs: None,
    }
}

/// 표시 우선순위: 남은 비율이 낮은 것 → 최신(age 작은 것) 순. 최대 API_LIMIT개.
pub fn prioritize_api_rows(mut rows: Vec<DisplayApiRow>) -> Vec<DisplayApiRow> {
    // 정렬만 수행한다. 행 선택/고정/리밋은 build_api_gauges가 담당하며
    // 여기서 잘라내면 Codex가 사라질 수 있다(스펙 §1).
    rows.sort_by(|a, b| {
        let a_remaining = a.primary_remaining_percent.unwrap_or(f64::INFINITY);
        let b_remaining = b.primary_remaining_percent.unwrap_or(f64::INFINITY);
        a_remaining
            .partial_cmp(&b_remaining)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.age_secs.cmp(&b.age_secs))
    });
    rows
}

// ---------------------------------------------------------------- 컴팩트 API 게이지

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ApiGaugeState {
    Normal,
    Caution,
    Warning,
    Critical,
    Limit,
    Auth,
    #[default]
    Unknown,
}

impl ApiGaugeState {
    /// 채움 색. None이면 채우지 않는다(AUTH/N/A — 가짜 0% 게이지 금지).
    pub fn fill_color(self) -> Option<(u8, u8, u8)> {
        match self {
            ApiGaugeState::Normal => Some(COLOR_ACCENT),
            ApiGaugeState::Caution => Some(COLOR_WARN),
            ApiGaugeState::Warning => Some(COLOR_ORANGE),
            ApiGaugeState::Critical | ApiGaugeState::Limit => Some(COLOR_DANGER),
            ApiGaugeState::Auth | ApiGaugeState::Unknown => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApiGaugeRow {
    pub label: String,
    pub sublabel: String,
    pub remaining: Option<f64>,
    pub state: ApiGaugeState,
    pub stale: bool,
    pub amount_text: Option<String>,
    pub reset_in_secs: Option<i64>,
}

/// 남은 비율 -> 게이지 상태(스펙 §4). 0%는 실제 소진일 때만 LIMIT.
pub fn gauge_state(remaining: Option<f64>) -> ApiGaugeState {
    match remaining {
        None => ApiGaugeState::Unknown,
        Some(value) if value <= 0.0 => ApiGaugeState::Limit,
        Some(value) if value <= 10.0 => ApiGaugeState::Critical,
        Some(value) if value <= 20.0 => ApiGaugeState::Warning,
        Some(value) if value <= 50.0 => ApiGaugeState::Caution,
        Some(_) => ApiGaugeState::Normal,
    }
}

/// 채움 폭 = 남은 비율(반전 금지, 스펙 §10).
pub fn gauge_fill_width(row_width: i32, remaining: Option<f64>) -> i32 {
    let Some(value) = remaining else { return 0 };
    let clamped = value.clamp(0.0, 100.0);
    ((row_width as f64) * clamped / 100.0).round() as i32
}

/// 우측 값 텍스트(스펙 §6/§7): 0%는 LIMIT, 인증 실패는 AUTH, 오래되면 stale.
/// 리셋까지 남은 시간을 짧게: 1일 이상 "Nd", 미만 "Nh".
pub fn format_reset_in(secs: Option<i64>) -> String {
    match secs {
        None => String::new(),
        Some(value) if value <= 0 => String::new(),
        Some(value) if value >= 86_400 => format!("{}d", value / 86_400),
        Some(value) => format!("{}h", (value + 3_599) / 3_600),
    }
}

pub fn gauge_value_text(row: &ApiGaugeRow) -> String {
    if row.state == ApiGaugeState::Auth {
        return "AUTH".into();
    }
    let mut text = match (row.remaining, row.amount_text.as_deref()) {
        (Some(value), _) if value <= 0.0 => "0% LIMIT".into(),
        (Some(value), _) => format!("{}%", value.round() as i64),
        (None, Some(amount)) => amount.to_string(),
        (None, None) => "N/A".into(),
    };
    if row.state == ApiGaugeState::Critical {
        text = format!("! {text}");
    }
    let reset = format_reset_in(row.reset_in_secs);
    if !reset.is_empty() {
        text.push_str(" · ");
        text.push_str(&reset);
    }
    if row.stale {
        text.push_str(" · stale");
    }
    text
}

/// provider 1개 -> 주간(primary) 게이지 1행. primary가 없으면 secondary로 폴백한다.
/// (5시간/월간 창은 컴팩트 대시보드에 표시하지 않는다 — 주간 한도만.)
fn expand_provider(row: &DisplayApiRow, compact_label: &str) -> Vec<ApiGaugeRow> {
    let stale = row.age_secs > 900;
    let status_ok = row.status == "ok";
    let primary = row.primary_remaining_percent.map(|value| {
        (
            row.primary_label.clone(),
            Some(value),
            row.primary_reset_in_secs,
        )
    });
    let fallback = row.secondary_remaining_percent.map(|value| {
        (
            row.secondary_label.clone(),
            Some(value),
            row.secondary_reset_in_secs,
        )
    });
    let Some((sublabel, remaining, reset_in_secs)) = primary.or(fallback) else {
        // 값 없음: 인증 실패/만료(status != ok)는 AUTH, 그 외는 N/A.
        let auth = !status_ok && row.amount_text.is_none();
        return vec![ApiGaugeRow {
            label: compact_label.to_string(),
            sublabel: String::new(),
            remaining: None,
            state: if auth {
                ApiGaugeState::Auth
            } else {
                ApiGaugeState::Unknown
            },
            stale,
            amount_text: row.amount_text.clone(),
            reset_in_secs: None,
        }];
    };
    let sublabel = if sublabel.is_empty() {
        "Wk".to_string()
    } else {
        sublabel
    };
    vec![ApiGaugeRow {
        label: compact_label.to_string(),
        sublabel,
        remaining,
        state: gauge_state(remaining),
        stale,
        amount_text: None,
        reset_in_secs,
    }]
}

/// 고정 우선순위(스펙 §1)에서의 위치. 목록 밖 provider는 뒤로 밀린다.
fn pin_rank(id: &str) -> usize {
    API_PIN_ORDER
        .iter()
        .position(|pinned| *pinned == id)
        .unwrap_or(API_PIN_ORDER.len())
}

fn compact_label_for(id: &str, fallback: &str) -> String {
    match id {
        "openrouter" => "OpenRouter".into(),
        "opencode" => "OpenCode".into(),
        _ => {
            let source = if fallback.is_empty() { id } else { fallback };
            truncate(source, 12)
        }
    }
}

/// 컴팩트 API 게이지 목록(스펙 §1/§3/§8).
/// Codex는 스냅샷이 있으면 반드시 포함(없으면 AUTH 행), 우선순위대로 채우고
/// 나머지는 남은 비율이 낮은 순으로 채운다. 반환: (표시 행, 오버플로 행 수)
pub fn build_api_gauges(rows: &[DisplayApiRow], limit: usize) -> (Vec<ApiGaugeRow>, usize) {
    let mut out: Vec<ApiGaugeRow> = Vec::new();
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();

    // 1) Codex 고정 — 스냅샷이 없어도 AUTH 행을 만든다(절대 생략 금지).
    match rows.iter().find(|row| row.id == "codex") {
        Some(row) => out.extend(expand_provider(row, "Codex")),
        None => out.push(ApiGaugeRow {
            label: "Codex".into(),
            remaining: None,
            state: ApiGaugeState::Auth,
            ..Default::default()
        }),
    }
    used.insert("codex".into());

    // 2) Antigravity(Gem/Cl)
    if let Some(row) = rows.iter().find(|row| row.id == "antigravity") {
        out.extend(expand_provider(row, "AntiG"));
        used.insert("antigravity".into());
    }

    // 3) Grok 계열은 한 슬롯: 유효한 스냅샷 우선, 동률이면 남은 값이 낮은 쪽.
    //    두 서비스를 한 행으로 합치지 않고 라벨로 구분한다(스펙 §9).
    {
        let mut candidates: Vec<&DisplayApiRow> = rows
            .iter()
            .filter(|row| (row.id == "grok" || row.id == "grok-build") && !used.contains(row.id.as_str()))
            .collect();
        candidates.sort_by(|a, b| {
            let a_valid = a.primary_remaining_percent.is_some();
            let b_valid = b.primary_remaining_percent.is_some();
            b_valid.cmp(&a_valid).then_with(|| {
                let a_value = a.primary_remaining_percent.unwrap_or(f64::INFINITY);
                let b_value = b.primary_remaining_percent.unwrap_or(f64::INFINITY);
                a_value
                    .partial_cmp(&b_value)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });
        if let Some(row) = candidates.first() {
            let label = if row.id == "grok-build" {
                "Grok Build"
            } else {
                "Grok Bot"
            };
            out.extend(expand_provider(row, label));
            used.insert(row.id.clone());
        }
    }

    // 4) OpenCode Go(최대 두 창)
    if let Some(row) = rows.iter().find(|row| row.id == "opencode") {
        out.extend(expand_provider(row, "OpenCode"));
        used.insert("opencode".into());
    }

    // 5) 나머지(OpenRouter 등): 남은 비율 낮은 순 -> 최신, 각 1행.
    let mut rest: Vec<&DisplayApiRow> = rows
        .iter()
        .filter(|row| !used.contains(row.id.as_str()))
        .collect();
    rest.sort_by(|a, b| {
        let a_value = a.primary_remaining_percent.unwrap_or(f64::INFINITY);
        let b_value = b.primary_remaining_percent.unwrap_or(f64::INFINITY);
        a_value
            .partial_cmp(&b_value)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.age_secs.cmp(&b.age_secs))
            .then_with(|| pin_rank(&a.id).cmp(&pin_rank(&b.id)))
    });
    for row in rest {
        let label = compact_label_for(&row.id, &row.label);
        out.extend(expand_provider(row, &label));
    }

    let overflow = out.len().saturating_sub(limit);
    out.truncate(limit);
    (out, overflow)
}

/// 워크로드 정렬: 관리형 서비스 → VRAM 큰 순 → unknown. 최대 WORKLOAD_LIMIT.
pub fn prioritize_workloads(mut rows: Vec<DisplayWorkload>) -> (Vec<DisplayWorkload>, usize) {
    rows.sort_by(|a, b| {
        // 1) AI/관리 워크로드 우선, 2) GPU % → VRAM (엔진 합산 금지)
        let rank = |row: &DisplayWorkload| -> u8 {
            match row.classification.as_str() {
                "ai" => 0,
                _ => match row.kind.as_str() {
                    "managed_service" | "ollama" | "llama_cpp" | "vllm" | "comfyui" | "laya_app" => 0,
                    "unknown_ai" => 2,
                    _ => 1,
                },
            }
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| {
                b.gpu_percent
                    .unwrap_or(-1.0)
                    .partial_cmp(&a.gpu_percent.unwrap_or(-1.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
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

    let mut y_pos = 8;
    let show_gpu = page != "api";
    let show_api = page != "runtime";

    if show_gpu {
        y_pos = render_metrics_card(&mut canvas, snapshot, y_pos, width as i32);
        y_pos = render_process_card(&mut canvas, snapshot, y_pos, width as i32);
    }
    if show_api {
        // 단일 대시보드는 사용자가 정한 4개 주간 항목만, API 전용 페이지는 최대 6행.
        let limit = if page == "api" { API_GAUGE_LIMIT } else { 4 };
        render_api_card(&mut canvas, snapshot, y_pos, width as i32, height as i32, limit);
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

/// 플랫 카드(참조 CSS): surface + 1px border + radius.
fn draw_card(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32) {
    canvas.round_rect(x, y, w, h, 10, COLOR_BORDER);
    canvas.round_rect(x + 1, y + 1, w - 2, h - 2, 9, COLOR_SURFACE);
}

/// 상단 카드: GPU/VRAM/RAM 게이지(값은 accent/임계 색, 굵게).
fn render_metrics_card(canvas: &mut Canvas, snapshot: &TurzxDisplaySnapshot, y_pos: i32, width: i32) -> i32 {
    let gpu = &snapshot.gpu;
    let x = 8;
    let w = width - 16;
    let inner_x = x + 12;
    let inner_w = w - 24;
    let card_h = 118;
    draw_card(canvas, x, y_pos, w, card_h);

    let name = if gpu.name.is_empty() {
        "GPU 없음".to_string()
    } else {
        gpu.name.replace("NVIDIA GeForce ", "")
    };
    canvas.text(&truncate(&name, 20), inner_x, y_pos + 10, 19, COLOR_TEXT, true);
    let mut info = String::new();
    if let Some(temp) = gpu.temperature_c {
        info.push_str(&format!("{temp}°C"));
    }
    if let Some(power) = gpu.power_watts {
        if !info.is_empty() {
            info.push_str(" · ");
        }
        info.push_str(&format!("{power:.0}W"));
    }
    if !info.is_empty() {
        canvas.text_right(&info, x + w - 12, y_pos + 13, 13, COLOR_MUTED, false);
    }

    let mut row_y = y_pos + 36;
    let utilization = gpu.utilization_percent.map(|value| value as f64);
    let util_text = utilization
        .map(|value| format!("{}%", value.round() as i64))
        .unwrap_or_else(|| "N/A".into());
    draw_value_gauge(canvas, inner_x, inner_w, "GPU", utilization, COLOR_ACCENT, &util_text, row_y);
    row_y += API_GAUGE_ROW_HEIGHT;

    let percent = vram_percent(gpu);
    let vram_text = format!(
        "{} / {} GB",
        format_gib(gpu.vram_used_bytes),
        format_gib(gpu.vram_total_bytes)
    );
    draw_value_gauge(canvas, inner_x, inner_w, "VRAM", percent, vram_gauge_color(percent), &vram_text, row_y);
    row_y += API_GAUGE_ROW_HEIGHT;

    let ram_percent = match (gpu.ram_used_bytes, gpu.ram_total_bytes) {
        (Some(used), Some(total)) if total > 0 => Some(used as f64 / total as f64 * 100.0),
        _ => None,
    };
    let ram_text = format!(
        "{} / {} GB",
        format_gib(gpu.ram_used_bytes),
        format_gib(gpu.ram_total_bytes)
    );
    draw_value_gauge(canvas, inner_x, inner_w, "RAM", ram_percent, vram_gauge_color(ram_percent), &ram_text, row_y);

    y_pos + card_h + 6
}

/// GPU PROCESSES 카드.
fn render_process_card(canvas: &mut Canvas, snapshot: &TurzxDisplaySnapshot, y_pos: i32, width: i32) -> i32 {
    let x = 8;
    let w = width - 16;
    let inner_x = x + 12;

    let (rows, overflow) = prioritize_workloads(snapshot.workloads.clone());
    let row_lines: i32 = rows
        .iter()
        .map(|row| if row.model.is_some() { 34 } else { 19 })
        .sum();
    let card_h = 12 + 18 + row_lines + if overflow > 0 { 15 } else { 0 } + if rows.is_empty() { 18 } else { 0 } + 8;
    draw_card(canvas, x, y_pos, w, card_h);

    let mut row_y = y_pos + 10;
    canvas.text("GPU PROCESSES", inner_x, row_y, 12, COLOR_MUTED, true);
    row_y += 18;

    if rows.is_empty() {
        canvas.text("(GPU 사용 중인 워크로드 없음)", inner_x, row_y, 12, COLOR_FAINT, false);
        row_y += 18;
    }
    for workload in rows {
        let label = truncate(&workload.service, 17);
        canvas.text(&label, inner_x + 10, row_y, 14, COLOR_TEXT, true);
        canvas.circle(inner_x + 3, row_y + 7, 3, COLOR_ACCENT);
        let gpu_percent = workload
            .gpu_percent
            .map(|percent| format!("{}%", percent.round() as i64))
            .unwrap_or_else(|| "N/A".into());
        let vram = format!("{} GB", format_gib(workload.vram_bytes));
        canvas.text_right(
            &format!("{gpu_percent} · {vram}"),
            x + w - 12,
            row_y,
            12,
            COLOR_MUTED,
            false,
        );
        row_y += 19;
        if let Some(model) = workload.model.as_deref() {
            if !model.eq_ignore_ascii_case(&workload.service) {
                canvas.text(&truncate(model, 24), inner_x + 10, row_y, 11, COLOR_MUTED, false);
                row_y += 15;
            }
        }
    }
    if overflow > 0 {
        canvas.text(&format!("+{overflow} more"), inner_x + 10, row_y, 11, COLOR_FAINT, false);
    }
    y_pos + card_h + 6
}

fn api_card_height(rows: usize, overflow: usize) -> i32 {
    10 + 18
        + rows as i32 * (API_GAUGE_ROW_HEIGHT + API_GAUGE_ROW_GAP)
        + if overflow > 0 { 14 } else { 0 }
        + 6
}

/// API 주간 한도 카드(Codex 고정 + 주간 잔여/리셋).
fn render_api_card(
    canvas: &mut Canvas,
    snapshot: &TurzxDisplaySnapshot,
    y_pos: i32,
    width: i32,
    height: i32,
    limit: usize,
) -> i32 {
    let x = 8;
    let w = width - 16;
    let inner_x = x + 12;
    let inner_w = w - 24;

    if snapshot.api_usage.is_empty() {
        draw_card(canvas, x, y_pos, w, 46);
        canvas.text("API USAGE", inner_x, y_pos + 10, 12, COLOR_MUTED, true);
        canvas.text("(사용량 스냅샷 없음)", inner_x, y_pos + 26, 12, COLOR_FAINT, false);
        return y_pos + 52;
    }
    let (mut rows, mut overflow) = build_api_gauges(&snapshot.api_usage, limit.max(1));
    // 어떤 경우에도 카드(그리고 Codex)는 사라지지 않는다 — 공간이 부족하면 행을 줄인다.
    let available = height - 26 - y_pos;
    let mut card_h = api_card_height(rows.len(), overflow);
    while rows.len() > 1 && card_h > available {
        if rows.pop().is_some() {
            overflow += 1;
        }
        card_h = api_card_height(rows.len(), overflow);
    }
    if card_h > available {
        return y_pos; // 푸터 침범 방지(이 경우엔 페이지가 이미 가득 참)
    }
    draw_card(canvas, x, y_pos, w, card_h);

    let mut row_y = y_pos + 9;
    canvas.text("API USAGE", inner_x, row_y, 12, COLOR_MUTED, true);
    canvas.text_right("Wk · 리셋", x + w - 12, row_y, 10, COLOR_FAINT, false);
    row_y += 18;
    for row in &rows {
        draw_api_gauge(canvas, row, row_y, width, inner_x, inner_w);
        row_y += API_GAUGE_ROW_HEIGHT + API_GAUGE_ROW_GAP;
    }
    if overflow > 0 {
        canvas.text(&format!("+{overflow} more"), inner_x, row_y, 11, COLOR_FAINT, false);
    }
    y_pos + card_h + 6
}

/// 값 라벨형 오버레이 게이지(GPU/VRAM): 채움 = 값, 우측 값 텍스트는 칩 위에 올린다.
fn draw_value_gauge(
    canvas: &mut Canvas,
    x: i32,
    w: i32,
    label: &str,
    percent: Option<f64>,
    fill_color: (u8, u8, u8),
    value: &str,
    y: i32,
) {
    let h = API_GAUGE_ROW_HEIGHT;
    canvas.round_rect(x, y, w, h, 8, COLOR_BAR_BG);
    let fill_w = gauge_fill_width(w, percent);
    if fill_w > 0 {
        canvas.round_rect(x, y, fill_w.max(3), h, 8, fill_color);
    }
    let label_w = canvas.text_width(label, 14, true);
    let label_chip_w = label_w + 14;
    canvas.round_rect(x + 2, y + 2, label_chip_w, h - 4, 6, COLOR_CHIP_BORDER);
    canvas.round_rect(x + 3, y + 3, label_chip_w - 2, h - 6, 5, COLOR_GAUGE_CHIP);
    canvas.text(label, x + 9, y + 4, 14, COLOR_TEXT, true);
    let value_w = canvas.text_width(value, 14, true);
    let chip_w = value_w + 14;
    canvas.round_rect(x + w - chip_w - 2, y + 2, chip_w, h - 4, 6, COLOR_CHIP_BORDER);
    canvas.round_rect(x + w - chip_w - 1, y + 3, chip_w - 2, h - 6, 5, COLOR_GAUGE_CHIP);
    canvas.text_right(value, x + w - 9, y + 4, 14, COLOR_TEXT, true);
}

/// 컴팩트 오버레이 게이지 1행: 배경=남은 비율 채움, 텍스트는 그 위에.
fn draw_api_gauge(canvas: &mut Canvas, row: &ApiGaugeRow, y: i32, width: i32, x: i32, w: i32) {
    let _ = width;
    let h = API_GAUGE_ROW_HEIGHT;

    canvas.round_rect(x, y, w, h, 8, COLOR_BAR_BG);
    let fill_w = gauge_fill_width(w, row.remaining);
    if let Some(color) = row.state.fill_color() {
        if fill_w > 0 {
            canvas.round_rect(x, y, fill_w.max(3), h, 8, color);
        }
    } else if row.stale {
        canvas.round_rect(x, y, (w / 8).max(3), h, 8, COLOR_FAINT);
    }

    let label = if row.sublabel.is_empty() {
        row.label.clone()
    } else {
        format!("{} {}", row.label, row.sublabel)
    };
    // 라벨도 어두운 칩 위에 올려 채움과 무관하게 항상 읽히게 한다.
    let label_text = truncate(&label, 18);
    let label_w = canvas.text_width(&label_text, 14, true);
    let label_chip_w = label_w + 14;
    canvas.round_rect(x + 2, y + 2, label_chip_w, h - 4, 6, COLOR_CHIP_BORDER);
    canvas.round_rect(x + 3, y + 3, label_chip_w - 2, h - 6, 5, COLOR_GAUGE_CHIP);
    let label_color = if row.state == ApiGaugeState::Auth || row.state == ApiGaugeState::Unknown {
        COLOR_MUTED
    } else {
        COLOR_TEXT
    };
    canvas.text(&label_text, x + 9, y + 4, 14, label_color, true);

    let value = gauge_value_text(row);
    let value_w = canvas.text_width(&value, 14, true);
    let chip_w = value_w + 14;
    canvas.round_rect(x + w - chip_w - 2, y + 2, chip_w, h - 4, 6, COLOR_CHIP_BORDER);
    canvas.round_rect(x + w - chip_w - 1, y + 3, chip_w - 2, h - 6, 5, COLOR_GAUGE_CHIP);
    let value_color = match row.state {
        ApiGaugeState::Critical | ApiGaugeState::Limit => COLOR_DANGER,
        ApiGaugeState::Warning => COLOR_ORANGE,
        ApiGaugeState::Caution => COLOR_WARN,
        ApiGaugeState::Auth | ApiGaugeState::Unknown => COLOR_MUTED,
        ApiGaugeState::Normal => COLOR_TEXT,
    };
    let value_color = if row.stale { COLOR_MUTED } else { value_color };
    canvas.text_right(&value, x + w - 9, y + 4, 14, value_color, true);
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
            ram_used_bytes: Some((22.9 * GIB) as u64),
            ram_total_bytes: Some((63.9 * GIB) as u64),
        },
        workloads: vec![
            DisplayWorkload {
                service: "Ollama".into(),
                model: Some("qwen3.5:9b".into()),
                vram_bytes: None,
                kind: "ollama".into(),
                confidence: "exact".into(),
                cpu_only: false,
                gpu_percent: Some(16.0),
                classification: "ai".into(),
            },
            DisplayWorkload {
                service: "Laya".into(),
                model: Some("english · multilingual · typed-decisions".into()),
                vram_bytes: Some((1.4 * GIB) as u64),
                kind: "managed_service".into(),
                confidence: "exact".into(),
                cpu_only: false,
                gpu_percent: Some(24.0),
                classification: "ai".into(),
            },
            DisplayWorkload {
                service: "ComfyUI".into(),
                model: Some("sd_xl_base_1.0.safetensors".into()),
                vram_bytes: Some((0.3 * GIB) as u64),
                kind: "comfyui".into(),
                confidence: "medium".into(),
                cpu_only: false,
                gpu_percent: Some(8.0),
                classification: "ai".into(),
            },
            DisplayWorkload {
                service: "Granblue Fantasy: Relink".into(),
                model: None,
                vram_bytes: Some((6.1 * GIB) as u64),
                kind: "game".into(),
                confidence: "high".into(),
                cpu_only: false,
                gpu_percent: Some(71.0),
                classification: "game".into(),
            },
            DisplayWorkload {
                service: "msedge.exe".into(),
                model: None,
                vram_bytes: Some((0.8 * GIB) as u64),
                kind: "browser".into(),
                confidence: "high".into(),
                cpu_only: false,
                gpu_percent: Some(3.0),
                classification: "browser".into(),
            },
        ],
        api_usage: vec![
            // Codex 주간 한도(고정) 93% + 리셋 6일.
            DisplayApiRow {
                id: "codex".into(),
                label: "Codex".into(),
                primary_remaining_percent: Some(93.0),
                primary_label: "Wk".into(),
                primary_reset_in_secs: Some(594_000),
                amount_text: None,
                status: "ok".into(),
                age_secs: 60,
                ..Default::default()
            },
            // Antigravity: 더 빠듯한 주간 창(Claude 38%, 리셋 3시간)과 Gemini(63%, 2일).
            DisplayApiRow {
                id: "antigravity".into(),
                label: "Antigravity".into(),
                primary_remaining_percent: Some(63.0),
                primary_label: "Gem".into(),
                primary_reset_in_secs: Some(241_920),
                secondary_remaining_percent: Some(38.0),
                secondary_label: "Cl".into(),
                secondary_reset_in_secs: Some(10_800),
                amount_text: None,
                status: "ok".into(),
                age_secs: 120,
                ..Default::default()
            },
            // Grok Build 주간 91% + 리셋 5일.
            DisplayApiRow {
                id: "grok-build".into(),
                label: "Grok Build".into(),
                primary_remaining_percent: Some(91.0),
                primary_label: "Wk".into(),
                primary_reset_in_secs: Some(506_000),
                amount_text: None,
                status: "ok".into(),
                age_secs: 45,
                ..Default::default()
            },
            // OpenCode Go 주간 25% + 리셋 10시간.
            DisplayApiRow {
                id: "opencode".into(),
                label: "OpenCode".into(),
                primary_remaining_percent: Some(25.0),
                primary_label: "Wk".into(),
                primary_reset_in_secs: Some(36_000),
                secondary_remaining_percent: Some(7.0),
                secondary_label: "Mo".into(),
                amount_text: None,
                status: "ok".into(),
                age_secs: 30,
                ..Default::default()
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
    fn sorts_low_remaining_first_without_dropping_codex() {
        let mut rows = vec![
            DisplayApiRow { id: "a".into(), label: "A".into(), primary_remaining_percent: Some(80.0), age_secs: 5, ..Default::default() },
            DisplayApiRow { id: "b".into(), label: "B".into(), primary_remaining_percent: Some(12.0), age_secs: 900, ..Default::default() },
            DisplayApiRow { id: "codex".into(), label: "Codex".into(), primary_remaining_percent: Some(94.0), primary_label: "Wk".into(), secondary_remaining_percent: Some(72.0), secondary_label: "5h".into(), age_secs: 1, ..Default::default() },
            DisplayApiRow { id: "d".into(), label: "D".into(), primary_remaining_percent: Some(30.0), age_secs: 60, ..Default::default() },
        ];
        let ordered = prioritize_api_rows(rows.clone());
        // 정렬만 수행하고 잘라내지 않는다(스냅샷은 전체를 유지).
        assert_eq!(ordered.len(), rows.len());
        assert_eq!(ordered[0].id, "b");
        assert!(ordered.iter().any(|row| row.id == "codex"));
        rows.clear();
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
        // AI 랭크 안에서는 GPU % → VRAM; unknown_ai는 최하위라 밀려난다.
        assert_eq!(kept[0].service, "Ollama");
        assert_eq!(kept[1].service, "vLLM");
        assert_eq!(kept[2].service, "Laya");
        assert!(kept.iter().all(|row| row.kind != "unknown_ai"));
        assert_eq!(overflow, 1);
    }

    #[test]
    fn gauge_thresholds_follow_spec() {
        // 스펙 §4: >50 정상, 20–50 주의, 10–20 경고, <=10 위험, 0 LIMIT.
        assert_eq!(gauge_state(Some(94.0)), ApiGaugeState::Normal);
        assert_eq!(gauge_state(Some(51.0)), ApiGaugeState::Normal);
        assert_eq!(gauge_state(Some(50.0)), ApiGaugeState::Caution);
        assert_eq!(gauge_state(Some(35.0)), ApiGaugeState::Caution);
        assert_eq!(gauge_state(Some(20.1)), ApiGaugeState::Caution);
        assert_eq!(gauge_state(Some(20.0)), ApiGaugeState::Warning);
        assert_eq!(gauge_state(Some(15.0)), ApiGaugeState::Warning);
        assert_eq!(gauge_state(Some(10.0)), ApiGaugeState::Critical);
        assert_eq!(gauge_state(Some(7.0)), ApiGaugeState::Critical);
        assert_eq!(gauge_state(Some(0.0)), ApiGaugeState::Limit);
        assert_eq!(gauge_state(None), ApiGaugeState::Unknown);

        assert_eq!(ApiGaugeState::Normal.fill_color(), Some(COLOR_ACCENT));
        assert_eq!(ApiGaugeState::Caution.fill_color(), Some(COLOR_WARN));
        assert_eq!(ApiGaugeState::Warning.fill_color(), Some(COLOR_ORANGE));
        assert_eq!(ApiGaugeState::Critical.fill_color(), Some(COLOR_DANGER));
        assert_eq!(ApiGaugeState::Limit.fill_color(), Some(COLOR_DANGER));
        assert_eq!(ApiGaugeState::Auth.fill_color(), None);
        assert_eq!(ApiGaugeState::Unknown.fill_color(), None);

        // VRAM(사용률) 의미는 별도: 높은 사용률이 위험, GPU 사용률은 항상 accent.
        assert_eq!(vram_gauge_color(Some(40.0)), COLOR_ACCENT);
        assert_eq!(vram_gauge_color(Some(80.0)), COLOR_WARN);
        assert_eq!(vram_gauge_color(Some(95.0)), COLOR_DANGER);
    }

    // ── 컴팩트 API 게이지(스펙 §1~§14) ───────────────────────────────

    fn api_row(id: &str, remaining: Option<f64>, window: &str) -> DisplayApiRow {
        DisplayApiRow {
            id: id.into(),
            label: id.into(),
            primary_remaining_percent: remaining,
            primary_label: window.into(),
            status: "ok".into(),
            age_secs: 10,
            ..Default::default()
        }
    }

    #[test]
    fn codex_is_pinned_with_weekly_quota_and_reset() {
        // 스펙(사용자): 주간 한도만 + Codex 고정. 우선순위 문서도 함께 확인.
        assert_eq!(API_PIN_ORDER[0], "codex");
        assert_eq!(API_PIN_ORDER[1], "antigravity");
        assert!(API_PIN_ORDER.contains(&"grok"));
        assert!(API_PIN_ORDER.contains(&"opencode"));
        let (gauges, _) = build_api_gauges(&sample_snapshot().api_usage, API_GAUGE_LIMIT);
        assert_eq!(gauges[0].label, "Codex");
        assert_eq!(gauges[0].sublabel, "Wk");
        assert_eq!(gauges[0].remaining, Some(93.0));
        assert_eq!(gauge_value_text(&gauges[0]), "93% · 6d");
        // 주간 전용: 5시간/월간 행은 렌더하지 않는다.
        assert!(gauges.iter().all(|row| row.sublabel != "5h" && row.sublabel != "Mo"));
    }


    #[test]
    fn formats_reset_countdown_in_days_and_hours() {
        assert_eq!(format_reset_in(None), "");
        assert_eq!(format_reset_in(Some(-5)), "");
        assert_eq!(format_reset_in(Some(3_600)), "1h");
        assert_eq!(format_reset_in(Some(36_000)), "10h");
        assert_eq!(format_reset_in(Some(86_400)), "1d");
        assert_eq!(format_reset_in(Some(506_000)), "5d");
        assert_eq!(format_reset_in(Some(594_000)), "6d");
        let row = ApiGaugeRow {
            label: "Codex".into(),
            sublabel: "Wk".into(),
            remaining: Some(93.0),
            state: ApiGaugeState::Normal,
            reset_in_secs: Some(594_000),
            ..Default::default()
        };
        assert_eq!(gauge_value_text(&row), "93% · 6d");
    }

    #[test]
    fn reset_helpers_parse_iso_and_epoch() {
        let value = serde_json::json!({"primaryResetAt": "2026-10-04T11:49:37Z"});
        let now = 1_790_000_000; // 2026-09-27 근처
        let secs = reset_in_secs_from(&value, &["primaryResetAt"], now).unwrap();
        assert!(secs > 900_000 && secs < 1_400_000, "약 11~16일(2026-09-21 기준): {secs}");
        let epoch = serde_json::json!({"resetInSec": 1234});
        assert_eq!(reset_in_secs_from(&epoch, &["resetInSec"], now), Some(1234));
    }

    #[test]
    fn codex_survives_provider_sorting_with_lower_quotas() {
        let mut rows: Vec<DisplayApiRow> = (0..20)
            .map(|index| api_row(&format!("cred{index}"), Some(1.0), ""))
            .collect();
        rows.push(api_row("codex", Some(94.0), "Wk"));
        let (gauges, _) = build_api_gauges(&rows, API_GAUGE_LIMIT);
        assert!(
            gauges.iter().any(|row| row.label == "Codex"),
            "Codex는 낮은 잔여량 정렬로 사라지면 안 된다"
        );
        assert_eq!(gauges[0].label, "Codex");
    }

    #[test]
    fn missing_codex_renders_auth_row_instead_of_omitting() {
        let rows = vec![api_row("grok", Some(50.0), "Mo")];
        let (gauges, _) = build_api_gauges(&rows, API_GAUGE_LIMIT);
        assert_eq!(gauges[0].label, "Codex");
        assert_eq!(gauges[0].state, ApiGaugeState::Auth);
        assert_eq!(gauge_value_text(&gauges[0]), "AUTH");
    }

    #[test]
    fn auth_failure_is_not_zero_quota() {
        let row = DisplayApiRow {
            id: "grok".into(),
            label: "Grok".into(),
            primary_remaining_percent: None,
            primary_label: "Mo".into(),
            status: "unavailable".into(),
            age_secs: 10,
            ..Default::default()
        };
        let gauges = expand_provider(&row, "Grok Bot");
        assert_eq!(gauges.len(), 1);
        assert_eq!(gauges[0].state, ApiGaugeState::Auth);
        assert_eq!(gauges[0].remaining, None, "인증 실패를 0%로 만들지 않는다");
        assert_eq!(gauges[0].state.fill_color(), None);
        assert_eq!(gauge_value_text(&gauges[0]), "AUTH");
    }

    #[test]
    fn stale_rows_are_marked_and_dimmed() {
        let row = DisplayApiRow {
            id: "codex".into(),
            label: "Codex".into(),
            primary_remaining_percent: Some(72.0),
            primary_label: "Wk".into(),
            status: "ok".into(),
            age_secs: 3600,
            ..Default::default()
        };
        let gauges = expand_provider(&row, "Codex");
        assert!(gauges[0].stale);
        assert!(gauge_value_text(&gauges[0]).contains("stale"));
    }

    #[test]
    fn fill_width_represents_remaining_not_used() {
        // 남은 94% -> 거의 가득, 남은 7% -> 거의 비어 있음(반전 금지).
        assert_eq!(gauge_fill_width(300, Some(94.0)), 282);
        assert!(gauge_fill_width(300, Some(94.0)) > gauge_fill_width(300, Some(7.0)));
        assert_eq!(gauge_fill_width(300, Some(100.0)), 300);
        assert_eq!(gauge_fill_width(300, Some(0.0)), 0);
        assert_eq!(gauge_fill_width(300, None), 0);
    }

    #[test]
    fn limit_and_critical_value_text() {
        let limit = ApiGaugeRow {
            label: "OpenCode".into(),
            remaining: Some(0.0),
            state: ApiGaugeState::Limit,
            ..Default::default()
        };
        assert_eq!(gauge_value_text(&limit), "0% LIMIT");
        let critical = ApiGaugeRow {
            label: "OpenCode".into(),
            remaining: Some(7.0),
            state: ApiGaugeState::Critical,
            ..Default::default()
        };
        assert_eq!(gauge_value_text(&critical), "! 7%");
    }

    #[test]
    fn compact_gauge_rows_fit_320_width_without_clipping() {
        let mut canvas = Canvas::new(320, 480);
        let (gauges, _) = build_api_gauges(&sample_snapshot().api_usage, API_GAUGE_LIMIT);
        let row_width = 320 - 20;
        for gauge in &gauges {
            let label = if gauge.sublabel.is_empty() {
                gauge.label.clone()
            } else {
                format!("{} {}", gauge.label, gauge.sublabel)
            };
            let label_w = canvas.text_width(&truncate(&label, 18), 14, true);
            let value_w = canvas.text_width(&gauge_value_text(gauge), 14, true);
            assert!(
                label_w + value_w + 12 + 24 <= row_width,
                "라벨+값이 320px 행을 넘침: {label} (label={label_w}, value={value_w})"
            );
        }
        // GDI 캔버스가 정상 종료되는지(그리기 경로 예외 없음)
        let _ = canvas.finish();
    }

    #[test]
    fn overflow_rows_are_counted() {
        let mut rows: Vec<DisplayApiRow> = (0..8)
            .map(|index| api_row(&format!("cred{index}"), Some(50.0 - index as f64), "Wk"))
            .collect();
        rows.insert(0, api_row("codex", Some(93.0), "Wk"));
        let (gauges, overflow) = build_api_gauges(&rows, API_GAUGE_LIMIT);
        assert_eq!(gauges.len(), API_GAUGE_LIMIT);
        assert_eq!(gauges[0].label, "Codex");
        assert_eq!(overflow, 3, "9개 provider - 6행");
    }


    #[test]
    fn threshold_colors_are_drawn_in_compact_ai_section() {
        // 94 정상(accent) · 35 주의(amber) · 15 경고(orange) · 7 위험(red) 를 직접 구성한다.
        let mut snapshot = sample_snapshot();
        snapshot.api_usage = vec![
            api_row("codex", Some(94.0), "Wk"),
            api_row("antigravity", Some(35.0), "Wk"),
            api_row("grok-build", Some(15.0), "Wk"),
            api_row("opencode", Some(7.0), "Wk"),
        ];
        let frame = render(&snapshot, "single", Orientation::Portrait);
        let has = |color: (u8, u8, u8)| {
            frame
                .chunks_exact(3)
                .any(|pixel| pixel[0] == color.0 && pixel[1] == color.1 && pixel[2] == color.2)
        };
        assert!(has(COLOR_ACCENT), "정상 게이지(accent) 필요");
        assert!(has(COLOR_WARN), "주의(amber) 필요");
        assert!(has(COLOR_ORANGE), "경고(orange) 필요");
        assert!(has(COLOR_DANGER), "위험(red) 필요");
    }


    #[test]
    fn gauge_rows_do_not_overlap_footer() {
        // 푸터 영역(y>=452)에 게이지 채움색이 침범하면 안 된다(마지막 행+오버플로 여유).
        let frame = render(&sample_snapshot(), "single", Orientation::Portrait);
        let width = 320usize;
        for y in 452..480usize {
            for x in 0..width {
                let offset = (y * width + x) * 3;
                let pixel = (frame[offset], frame[offset + 1], frame[offset + 2]);
                assert_ne!(pixel, COLOR_ORANGE, "y={y} 에서 경고색 게이지가 푸터와 겹침");
                assert_ne!(pixel, COLOR_DANGER, "y={y} 에서 위험색 게이지가 푸터와 겹침");
                assert_ne!(pixel, COLOR_WARN, "y={y} 에서 주의색 게이지가 푸터와 겹침");
            }
        }
    }

    #[test]
    fn antigravity_weekly_from_quota_groups() {
        let details = serde_json::json!({
            "geminiRemainingPercent": 38.9,
            "claudeRemainingPercent": 100,
            "quotaGroups": [
                {"name": "Gemini Models", "weeklyRemainingPercent": 62.8, "weeklyResetAt": "2026-09-30T15:53:09Z"},
                {"name": "Claude and GPT models", "weeklyRemainingPercent": 38.2, "weeklyResetAt": "2026-09-27T01:22:09Z"}
            ]
        })
        .to_string();
        let row = normalize_monitor_row("antigravity", Some(&details)).unwrap();
        // 데스크탑 앱 기본 그룹인 Gemini 주간(62.8%)이 primary.
        assert_eq!(row.primary_label, "Gem");
        assert_eq!(row.primary_remaining_percent, Some(62.8));
        assert!(row.primary_reset_in_secs.is_some(), "주간 리셋 시각 파싱");
        assert_eq!(row.secondary_label, "Cl");
        assert_eq!(row.secondary_remaining_percent, Some(38.2));
    }

    #[test]
    fn opencode_prefers_weekly_window_with_reset() {
        let details = serde_json::json!({
            "rolling": {"percent": 12, "resetsAt": "2026-09-27T14:40:04Z"},
            "weekly": {"percent": 75, "resetsAt": "2026-09-28T00:00:00Z"},
            "monthly": {"percent": 93, "resetsAt": "2026-10-10T13:15:30Z"}
        })
        .to_string();
        let row = normalize_monitor_row("opencode", Some(&details)).unwrap();
        assert_eq!(row.primary_label, "Wk");
        assert_eq!(row.primary_remaining_percent, Some(25.0));
        assert!(row.primary_reset_in_secs.is_some());
        assert_eq!(row.secondary_label, "Mo");
        assert_eq!(row.secondary_remaining_percent, Some(7.0));
    }

    #[test]
    fn dto_includes_ram_and_reset_fields() {
        let value = serde_json::to_value(sample_snapshot()).unwrap();
        assert_eq!(value["gpu"]["ramTotalBytes"].as_u64().unwrap(), (63.9 * GIB) as u64);
        assert!(value["gpu"]["ramUsedBytes"].as_u64().unwrap() > 0);
        let codex = value["apiUsage"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "codex")
            .unwrap();
        assert!(codex["primaryResetInSecs"].as_i64().unwrap() > 0);
        // 시크릿/커맨드 라인 미노출 유지
        let text = value.to_string().to_lowercase();
        for forbidden in ["commandline", "api_key", "api-key", "sk-"] {
            assert!(!text.contains(forbidden), "{forbidden} 노출");
        }
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
