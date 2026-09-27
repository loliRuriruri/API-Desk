//! Windows PDH 기반 GPU 프로세스 수집기.
//!
//! - `\GPU Engine(*)\Utilization Percentage` (엔진별 사용률, rate 카운터)
//! - `\GPU Process Memory(*)\Dedicated Usage` / `Shared Usage` (bytes)
//! - 쿼리를 영구 보관하고 매 폴링마다 재사용한다(새 프로세스/쿼리 생성 없음).
//! - rate 카운터는 최소 2회 샘플이 필요하므로 첫 수집은 사용률을 비워 둔다(N/A).
//! - 한국어 Windows에서도 동작하도록 `PdhAddEnglishCounterW`를 사용한다.
//!
//! 참조: Microsoft PDH 문서, lablup/all-smi·GuillaumeGomez/sysinfo의 접근 방식(행위 참고).
//! 코드는 원 구현(PDH C API 직접 호출)이며 GPL 코드를 복사하지 않았다.
use std::collections::HashMap;
use std::sync::Mutex;

#[cfg(windows)]
mod ffi {
    pub const PDH_MORE_DATA: u32 = 0x8000_07D2;
    pub const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
    pub const PDH_CSTATUS_VALID_DATA: u32 = 0x0000_0000;
    pub const PDH_CSTATUS_NEW_DATA: u32 = 0x0000_0001;
    pub const PDH_MAX_COUNTER_NAME: usize = 1024;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub union PdhValue {
        pub long_value: i32,
        pub double_value: f64,
        pub large_value: i64,
        pub wide_string: *mut u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct PdhFmtCounterValue {
        pub status: u32,
        pub value: PdhValue,
    }

    #[repr(C)]
    pub struct PdhFmtCounterValueItemW {
        pub name: *mut u16,
        pub value: PdhFmtCounterValue,
    }

    extern "system" {
        pub fn PdhOpenQueryW(source: *const u16, user_data: usize, query: *mut isize) -> u32;
        pub fn PdhAddEnglishCounterW(
            query: isize,
            path: *const u16,
            user_data: usize,
            counter: *mut isize,
        ) -> u32;
        pub fn PdhCollectQueryData(query: isize) -> u32;
        pub fn PdhGetFormattedCounterArrayW(
            counter: isize,
            format: u32,
            buffer_size: *mut u32,
            item_count: *mut u32,
            buffer: *mut PdhFmtCounterValueItemW,
        ) -> u32;
        pub fn PdhCloseQuery(query: isize) -> u32;
    }
}

/// GPU 엔진 종류(알 수 없는 종류는 문자열 그대로 보존).
pub fn engine_rank(engine: &str) -> u8 {
    match engine {
        "3D" => 0,
        "Compute" => 1,
        "VideoEncode" => 2,
        "VideoDecode" => 3,
        "VideoProcessing" => 4,
        "Copy" => 5,
        _ => 6,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PdhProcess {
    /// 엔진 종류별 최대 사용률(%). 같은 종류의 여러 인스턴스는 최대값으로 묶는다.
    pub engines: Vec<(String, f64)>,
    pub dedicated_bytes: Option<u64>,
    pub shared_bytes: Option<u64>,
    pub luid: u64,
}

impl PdhProcess {
    /// 헤드라인 GPU % = 가장 바쁜 엔진(합산 금지, 0..100 클램프).
    #[allow(dead_code)] // 병합 단계(RawGpuProcess::headline)에서 동일 규칙을 사용
    pub fn headline_percent(&self) -> Option<(String, f64)> {
        self.engines
            .iter()
            .max_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| engine_rank(&b.0).cmp(&engine_rank(&a.0)))
            })
            .map(|(engine, percent)| (engine.clone(), percent.clamp(0.0, 100.0)))
    }
}

#[derive(Clone, Debug, Default)]
pub struct PdhSnapshot {
    /// false면 rate 카운터가 아직 준비되지 않음(첫 샘플) → 사용률 N/A(진단용).
    #[allow(dead_code)]
    pub rates_ready: bool,
    pub processes: HashMap<u32, PdhProcess>,
}

/// `pid_1234_luid_0x00000000_0x0000C5A4_phys_0_eng_1_engtype_3D` 파싱.
pub fn parse_engine_instance(name: &str) -> Option<(u32, u64, String)> {
    let tokens: Vec<&str> = name.split('_').collect();
    if tokens.len() < 5 || !tokens[0].eq_ignore_ascii_case("pid") {
        return None;
    }
    let pid = tokens[1].parse::<u32>().ok()?;
    let luid = parse_luid(&tokens)?;
    let engine = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case("engtype"))
        .and_then(|index| tokens.get(index + 1))
        .map(|value| (*value).to_string())
        .filter(|value| !value.is_empty())?;
    Some((pid, luid, engine))
}

/// `pid_1234_luid_0x00000000_0x0000C5A4_phys_0` 파싱.
pub fn parse_memory_instance(name: &str) -> Option<(u32, u64)> {
    let tokens: Vec<&str> = name.split('_').collect();
    if tokens.len() < 3 || !tokens[0].eq_ignore_ascii_case("pid") {
        return None;
    }
    let pid = tokens[1].parse::<u32>().ok()?;
    let luid = parse_luid(&tokens)?;
    Some((pid, luid))
}

fn parse_luid(tokens: &[&str]) -> Option<u64> {
    let index = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case("luid"))?;
    let high = tokens.get(index + 1)?;
    let low = tokens.get(index + 2)?;
    let high = u64::from_str_radix(high.trim_start_matches("0x"), 16).ok()?;
    let low = u64::from_str_radix(low.trim_start_matches("0x"), 16).ok()?;
    Some((high << 32) | low)
}

/// 여러 (인스턴스) 값을 (pid, 엔진종류) 기준으로 묶어 최대값을 남긴다.
pub fn merge_engine_samples(
    samples: impl IntoIterator<Item = (u32, u64, String, f64)>,
) -> HashMap<u32, PdhProcess> {
    let mut map: HashMap<u32, PdhProcess> = HashMap::new();
    for (pid, luid, engine, percent) in samples {
        let entry = map.entry(pid).or_default();
        entry.luid = luid;
        match entry.engines.iter_mut().find(|(kind, _)| *kind == engine) {
            Some((_, slot)) => {
                if percent > *slot {
                    *slot = percent;
                }
            }
            None => entry.engines.push((engine, percent)),
        }
    }
    map
}

pub fn apply_memory_sample(map: &mut HashMap<u32, PdhProcess>, pid: u32, luid: u64, dedicated: bool, bytes: u64) {
    let entry = map.entry(pid).or_default();
    entry.luid = luid;
    if dedicated {
        entry.dedicated_bytes = Some(bytes);
    } else {
        entry.shared_bytes = Some(bytes);
    }
}

// ---------------------------------------------------------------- persistent query

struct QueryHandles {
    query: isize,
    engines: isize,
    dedicated: isize,
    shared: isize,
    samples: u32,
}

unsafe impl Send for QueryHandles {}

static QUERY: Mutex<Option<Option<QueryHandles>>> = Mutex::new(None);

#[cfg(not(windows))]
pub fn collect() -> PdhSnapshot {
    PdhSnapshot::default()
}

#[cfg(windows)]
pub fn collect() -> PdhSnapshot {
    let mut slot = QUERY.lock().unwrap();
    if slot.is_none() {
        *slot = Some(open_query());
    }
    let Some(Some(handles)) = slot.as_mut() else {
        return PdhSnapshot::default();
    };
    unsafe {
        if ffi::PdhCollectQueryData(handles.query) != 0 {
            return PdhSnapshot::default();
        }
        handles.samples = handles.samples.saturating_add(1);
        let rates_ready = handles.samples >= 2;

        let mut processes: HashMap<u32, PdhProcess> = HashMap::new();

        if rates_ready {
            if let Some(items) = read_double_array(handles.engines) {
                let samples = items.into_iter().filter_map(|(name, value)| {
                    let (pid, luid, engine) = parse_engine_instance(&name)?;
                    Some((pid, luid, engine, value))
                });
                processes = merge_engine_samples(samples);
            }
        }

        if let Some(items) = read_double_array(handles.dedicated) {
            for (name, value) in items {
                if let Some((pid, luid)) = parse_memory_instance(&name) {
                    apply_memory_sample(&mut processes, pid, luid, true, value.max(0.0) as u64);
                }
            }
        }
        if let Some(items) = read_double_array(handles.shared) {
            for (name, value) in items {
                if let Some((pid, luid)) = parse_memory_instance(&name) {
                    apply_memory_sample(&mut processes, pid, luid, false, value.max(0.0) as u64);
                }
            }
        }

        PdhSnapshot {
            rates_ready,
            processes,
        }
    }
}

#[cfg(windows)]
fn open_query() -> Option<QueryHandles> {
    let wide = |text: &str| -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    };
    unsafe {
        let mut query: isize = 0;
        if ffi::PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
            return None;
        }
        let mut engines: isize = 0;
        let mut dedicated: isize = 0;
        let mut shared: isize = 0;
        let ok = ffi::PdhAddEnglishCounterW(
            query,
            wide("\\GPU Engine(*)\\Utilization Percentage").as_ptr(),
            0,
            &mut engines,
        ) == 0
            && ffi::PdhAddEnglishCounterW(
                query,
                wide("\\GPU Process Memory(*)\\Dedicated Usage").as_ptr(),
                0,
                &mut dedicated,
            ) == 0
            && ffi::PdhAddEnglishCounterW(
                query,
                wide("\\GPU Process Memory(*)\\Shared Usage").as_ptr(),
                0,
                &mut shared,
            ) == 0;
        if !ok {
            let _ = ffi::PdhCloseQuery(query);
            return None;
        }
        Some(QueryHandles {
            query,
            engines,
            dedicated,
            shared,
            samples: 0,
        })
    }
}

#[cfg(windows)]
unsafe fn read_double_array(counter: isize) -> Option<Vec<(String, f64)>> {
    let mut size: u32 = 0;
    let mut count: u32 = 0;
    let status = ffi::PdhGetFormattedCounterArrayW(
        counter,
        ffi::PDH_FMT_DOUBLE,
        &mut size,
        &mut count,
        std::ptr::null_mut(),
    );
    if status != ffi::PDH_MORE_DATA || size == 0 || count == 0 {
        return None;
    }
    let item_size = std::mem::size_of::<ffi::PdhFmtCounterValueItemW>();
    let mut buffer = vec![0u8; size as usize + item_size];
    let mut size2 = buffer.len() as u32;
    let mut count2: u32 = 0;
    let status = ffi::PdhGetFormattedCounterArrayW(
        counter,
        ffi::PDH_FMT_DOUBLE,
        &mut size2,
        &mut count2,
        buffer.as_mut_ptr() as *mut ffi::PdhFmtCounterValueItemW,
    );
    if status != 0 {
        return None;
    }
    let items = std::slice::from_raw_parts(
        buffer.as_ptr() as *const ffi::PdhFmtCounterValueItemW,
        count2 as usize,
    );
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if item.name.is_null() {
            continue;
        }
        let status = item.value.status;
        if status != ffi::PDH_CSTATUS_VALID_DATA && status != ffi::PDH_CSTATUS_NEW_DATA {
            continue;
        }
        let value = item.value.value.double_value;
        if !value.is_finite() {
            continue;
        }
        let mut length = 0usize;
        while *item.name.add(length) != 0 && length < ffi::PDH_MAX_COUNTER_NAME {
            length += 1;
        }
        let name = String::from_utf16_lossy(std::slice::from_raw_parts(item.name, length));
        out.push((name, value));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_engine_instances() {
        let name = "pid_4212_luid_0x00000000_0x0000C5A4_phys_0_eng_1_engtype_3D";
        let (pid, luid, engine) = parse_engine_instance(name).unwrap();
        assert_eq!(pid, 4212);
        assert_eq!(luid, 0x0000_C5A4);
        assert_eq!(engine, "3D");
        assert!(parse_engine_instance("gpu_engine_total").is_none());
    }

    #[test]
    fn parses_memory_instances() {
        let name = "pid_777_luid_0x00000000_0x0000ABCD_phys_0";
        let (pid, luid) = parse_memory_instance(name).unwrap();
        assert_eq!(pid, 777);
        assert_eq!(luid, 0xABCD);
        assert!(parse_memory_instance("pid_bad_luid_0x0_0x0_phys_0").is_none());
    }

    #[test]
    fn merges_engine_instances_per_kind() {
        let samples = vec![
            (10u32, 1u64, "3D".to_string(), 12.0),
            (10, 1, "3D".to_string(), 40.0),
            (10, 1, "Copy".to_string(), 4.0),
            (10, 1, "Compute".to_string(), 15.0),
            (11, 1, "Compute".to_string(), 88.0),
        ];
        let merged = merge_engine_samples(samples);
        let ten = merged.get(&10).unwrap();
        assert_eq!(ten.engines.len(), 3);
        let three_d = ten.engines.iter().find(|(kind, _)| kind == "3D").unwrap();
        assert_eq!(three_d.1, 40.0);
        assert_eq!(
            ten.headline_percent(),
            Some(("3D".to_string(), 40.0)),
            "헤드라인은 합산이 아니라 가장 바쁜 엔진"
        );
        assert_eq!(merged.get(&11).unwrap().headline_percent(), Some(("Compute".to_string(), 88.0)));
    }

    #[test]
    fn clamps_headline_percent_to_100() {
        let mut process = PdhProcess::default();
        process.engines.push(("3D".to_string(), 123.5));
        assert_eq!(process.headline_percent(), Some(("3D".to_string(), 100.0)));
    }

    #[test]
    fn keeps_memory_kinds_separate() {
        let mut map = HashMap::new();
        apply_memory_sample(&mut map, 5, 1, true, 1024);
        apply_memory_sample(&mut map, 5, 1, false, 2048);
        let entry = map.get(&5).unwrap();
        assert_eq!(entry.dedicated_bytes, Some(1024));
        assert_eq!(entry.shared_bytes, Some(2048));
    }

    #[test]
    fn prefers_3d_for_equal_percentages() {
        let mut process = PdhProcess::default();
        process.engines.push(("Copy".to_string(), 50.0));
        process.engines.push(("3D".to_string(), 50.0));
        assert_eq!(process.headline_percent(), Some(("3D".to_string(), 50.0)));
    }
}
