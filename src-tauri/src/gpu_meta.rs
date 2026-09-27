//! 프로세스 메타데이터 수집(네이티브 Windows API, PowerShell 미사용).
//!
//! - identity: CreateToolhelp32Snapshot로 (pid, ppid, exe 이름) 일괄 수집(짧은 TTL 캐시)
//! - exe 경로: QueryFullProcessImageNameW(기존 GPU 모니터 헬퍼 재사용)
//! - 시작 시각: GetProcessTimes의 생성 FILETIME → PID 재사용 방지 캐시 키
//! - 커맨드 라인: PEB 읽기(best-effort, 권한 실패는 정상) — 내부 분류 전용
//! - 버전 리소스: ProductName/FileDescription/CompanyName (경로별 장기 캐시)
//!
//! 커맨드 라인은 프런트엔드로 절대 전달하지 않는다(분류/모델 추출 전용).
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const IDENTITY_TTL_MS: u64 = 2_000;
pub const CMDLINE_TTL_MS: u64 = 30_000;
pub const VERSION_TTL_MS: u64 = 600_000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcIdentity {
    pub name: String,
    pub parent: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VersionInfo {
    pub product_name: Option<String>,
    pub file_description: Option<String>,
    pub company_name: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ProcMeta {
    pub parent: Option<u32>,
    /// 가까운 순서의 조상 exe 이름(부모 → 조부모 …). 게임 런처/서비스 판정에 사용.
    pub ancestor_names: Vec<String>,
    pub name: String,
    pub exe_path: Option<String>,
    pub version: VersionInfo,
    /// 내부 전용. DTO로 직렬화하지 않는다.
    pub command_line: Option<String>,
}

impl ProcMeta {
    pub fn base_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        self.exe_path
            .as_deref()
            .map(|path| path.rsplit(['\\', '/']).next().unwrap_or(path).to_string())
            .unwrap_or_default()
    }

    pub fn display_name(&self) -> String {
        self.version
            .file_description
            .clone()
            .or_else(|| self.version.product_name.clone())
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_string())
            .unwrap_or_else(|| self.base_name())
    }
}

/// PID 재사용 방지: (pid, 시작 시각)이 같을 때만 캐시를 신뢰한다.
pub fn cache_entry_valid(cached_start: Option<u64>, current_start: Option<u64>) -> bool {
    match (cached_start, current_start) {
        (Some(a), Some(b)) => a == b,
        // 시작 시각을 못 읽는 프로세스(권한 제한)는 짧은 TTL 캐시만 사용한다.
        (None, None) => true,
        _ => false,
    }
}

fn base_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

// ---------------------------------------------------------------- caches

#[derive(Default)]
struct IdentityCache {
    map: HashMap<u32, ProcIdentity>,
    fetched_at: Option<Instant>,
}

#[derive(Default)]
pub struct MetaCache {
    identity: Mutex<IdentityCache>,
    versions: Mutex<HashMap<String, (VersionInfo, Instant)>>,
    cmdlines: Mutex<HashMap<u32, (Option<u64>, Option<String>, Instant)>>,
}

impl MetaCache {
    /// (pid, ppid, exe 이름) 맵. 짧은 TTL로 캐시한다.
    pub fn identities(&self) -> HashMap<u32, ProcIdentity> {
        {
            let cache = self.identity.lock().unwrap();
            if let Some(at) = cache.fetched_at {
                if at.elapsed().as_millis() as u64 <= IDENTITY_TTL_MS {
                    return cache.map.clone();
                }
            }
        }
        let fresh = identity_snapshot();
        let mut cache = self.identity.lock().unwrap();
        if !fresh.is_empty() {
            cache.map = fresh;
            cache.fetched_at = Some(Instant::now());
        }
        cache.map.clone()
    }

    /// GPU PID용 상세 메타데이터. version/cmdline은 개별 캐시를 사용한다.
    pub fn resolve(&self, pid: u32, identities: &HashMap<u32, ProcIdentity>) -> ProcMeta {
        let identity = identities.get(&pid).cloned().unwrap_or_default();
        let exe_path = crate::gpu_monitor::exe_path_pub(pid);
        let version = exe_path
            .as_deref()
            .map(|path| self.version_info(path))
            .unwrap_or_default();
        let command_line = self.command_line(pid);
        let ancestor_names = MetaCache::ancestors(pid, identities)
            .into_iter()
            .filter_map(|ancestor| identities.get(&ancestor).map(|item| item.name.clone()))
            .collect();
        ProcMeta {
            parent: identity.parent,
            ancestor_names,
            name: if identity.name.is_empty() {
                exe_path
                    .as_deref()
                    .map(base_name)
                    .unwrap_or_default()
            } else {
                identity.name
            },
            exe_path,
            version,
            command_line,
        }
    }

    pub fn version_info(&self, path: &str) -> VersionInfo {
        {
            let cache = self.versions.lock().unwrap();
            if let Some((info, at)) = cache.get(path) {
                if at.elapsed().as_millis() as u64 <= VERSION_TTL_MS {
                    return info.clone();
                }
            }
        }
        let info = read_version_info(path);
        let mut cache = self.versions.lock().unwrap();
        cache.insert(path.to_string(), (info.clone(), Instant::now()));
        if cache.len() > 512 {
            let cutoff = Instant::now() - Duration::from_millis(VERSION_TTL_MS);
            cache.retain(|_, (_, at)| *at > cutoff);
        }
        info
    }

    /// 커맨드 라인(best-effort). 분류에만 사용하고 외부로 내보내지 않는다.
    pub fn command_line(&self, pid: u32) -> Option<String> {
        let start = process_start_time(pid);
        {
            let cache = self.cmdlines.lock().unwrap();
            if let Some((cached_start, line, at)) = cache.get(&pid) {
                let fresh = at.elapsed().as_millis() as u64 <= CMDLINE_TTL_MS;
                if cache_entry_valid(*cached_start, start) && (fresh || line.is_some()) {
                    return line.clone();
                }
            }
        }
        let line = read_command_line(pid);
        let mut cache = self.cmdlines.lock().unwrap();
        cache.insert(pid, (start, line.clone(), Instant::now()));
        if cache.len() > 256 {
            let cutoff = Instant::now() - Duration::from_millis(CMDLINE_TTL_MS);
            cache.retain(|_, (_, _, at)| *at > cutoff);
        }
        line
    }

    /// 부모 체인(조상)을 가까운 순서로 돌려준다. 순환은 깊이 제한으로 차단.
    pub fn ancestors(pid: u32, identities: &HashMap<u32, ProcIdentity>) -> Vec<u32> {
        let mut result = Vec::new();
        let mut current = pid;
        for _ in 0..16 {
            let Some(identity) = identities.get(&current) else {
                break;
            };
            let Some(parent) = identity.parent else {
                break;
            };
            if parent == 0 || parent == current || result.contains(&parent) {
                break;
            }
            result.push(parent);
            current = parent;
        }
        result
    }
}

// ---------------------------------------------------------------- native calls

#[cfg(windows)]
fn identity_snapshot() -> HashMap<u32, ProcIdentity> {
    use std::ffi::c_void;

    #[repr(C)]
    struct ProcessEntry32W {
        size: u32,
        usage: u32,
        pid: u32,
        default_heap_id: usize,
        module_id: u32,
        threads: u32,
        parent_pid: u32,
        pri_class_base: i32,
        flags: u32,
        exe: [u16; 260],
    }

    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        fn Process32FirstW(snapshot: *mut c_void, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: *mut c_void, entry: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;

    let mut map = HashMap::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot as isize == -1 {
            return map;
        }
        let mut entry: ProcessEntry32W = std::mem::zeroed();
        entry.size = std::mem::size_of::<ProcessEntry32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let length = entry
                    .exe
                    .iter()
                    .position(|value| *value == 0)
                    .unwrap_or(entry.exe.len());
                let name = String::from_utf16_lossy(&entry.exe[..length]);
                map.insert(
                    entry.pid,
                    ProcIdentity {
                        name,
                        parent: Some(entry.parent_pid),
                    },
                );
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    map
}

#[cfg(not(windows))]
fn identity_snapshot() -> HashMap<u32, ProcIdentity> {
    HashMap::new()
}

#[cfg(windows)]
fn process_start_time(pid: u32) -> Option<u64> {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
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
        let mut creation = FileTime::default();
        let mut exit = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        Some(((creation.high as u64) << 32) | creation.low as u64)
    }
}

#[cfg(not(windows))]
fn process_start_time(pid: u32) -> Option<u64> {
    let _ = pid;
    None
}

/// x64 PEB에서 커맨드 라인을 읽는다(권한/구조 문제 시 None).
#[cfg(windows)]
fn read_command_line(pid: u32) -> Option<String> {
    use std::ffi::c_void;

    #[repr(C)]
    struct ProcessBasicInformation {
        exit_status: i32,
        peb_base_address: *mut c_void,
        affinity_mask: usize,
        base_priority: i32,
        unique_process_id: usize,
        inherited_from: usize,
    }

    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn ReadProcessMemory(
            handle: *mut c_void,
            address: *const c_void,
            buffer: *mut c_void,
            size: usize,
            read: *mut usize,
        ) -> i32;
        fn NtQueryInformationProcess(
            handle: *mut c_void,
            class: u32,
            info: *mut c_void,
            size: u32,
            returned: *mut u32,
        ) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_VM_READ: u32 = 0x0010;
    const PEB_PROCESS_PARAMETERS_OFFSET: usize = 0x20;
    const PARAMETERS_COMMAND_LINE_OFFSET: usize = 0x70;

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
        if handle.is_null() {
            return None;
        }
        let result = (|| -> Option<String> {
            let mut info = ProcessBasicInformation {
                exit_status: 0,
                peb_base_address: std::ptr::null_mut(),
                affinity_mask: 0,
                base_priority: 0,
                unique_process_id: 0,
                inherited_from: 0,
            };
            let mut returned = 0u32;
            let status = NtQueryInformationProcess(
                handle,
                0,
                &mut info as *mut _ as *mut c_void,
                std::mem::size_of::<ProcessBasicInformation>() as u32,
                &mut returned,
            );
            if status != 0 || info.peb_base_address.is_null() {
                return None;
            }
            let mut parameters: usize = 0;
            let mut read = 0usize;
            if ReadProcessMemory(
                handle,
                (info.peb_base_address as usize + PEB_PROCESS_PARAMETERS_OFFSET) as *const c_void,
                &mut parameters as *mut _ as *mut c_void,
                std::mem::size_of::<usize>(),
                &mut read,
            ) == 0
                || parameters == 0
            {
                return None;
            }
            // UNICODE_STRING { length: u16, maximum_length: u16, buffer: *mut u16 }
            let mut header = [0u8; 16];
            if ReadProcessMemory(
                handle,
                (parameters + PARAMETERS_COMMAND_LINE_OFFSET) as *const c_void,
                header.as_mut_ptr() as *mut c_void,
                header.len(),
                &mut read,
            ) == 0
            {
                return None;
            }
            let length = u16::from_le_bytes([header[0], header[1]]) as usize;
            let buffer = usize::from_le_bytes([
                header[8], header[9], header[10], header[11], header[12], header[13], header[14],
                header[15],
            ]);
            if buffer == 0 || length == 0 || length > 32_768 {
                return None;
            }
            let mut wide = vec![0u16; length / 2];
            if ReadProcessMemory(
                handle,
                buffer as *const c_void,
                wide.as_mut_ptr() as *mut c_void,
                length,
                &mut read,
            ) == 0
            {
                return None;
            }
            let text = String::from_utf16_lossy(&wide);
            let trimmed = text.trim_matches('\0').trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })();
        CloseHandle(handle);
        result
    }
}

#[cfg(not(windows))]
fn read_command_line(pid: u32) -> Option<String> {
    let _ = pid;
    None
}

#[cfg(windows)]
fn read_version_info(path: &str) -> VersionInfo {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn GetFileVersionInfoSizeW(path: *const u16, handle: *mut u32) -> u32;
        fn GetFileVersionInfoW(path: *const u16, handle: u32, size: u32, data: *mut c_void) -> i32;
        fn VerQueryValueW(
            data: *const c_void,
            sub_block: *const u16,
            buffer: *mut *mut c_void,
            length: *mut u32,
        ) -> i32;
    }

    let wide = |text: &str| -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let query = |data: *const c_void, block: &str| -> Option<String> {
        let block_wide = wide(block);
        let mut buffer: *mut c_void = std::ptr::null_mut();
        let mut length: u32 = 0;
        let ok = unsafe {
            VerQueryValueW(
                data,
                block_wide.as_ptr(),
                &mut buffer,
                &mut length,
            )
        };
        if ok == 0 || buffer.is_null() || length == 0 {
            return None;
        }
        let wide_slice = unsafe { std::slice::from_raw_parts(buffer as *const u16, length as usize) };
        let text = String::from_utf16_lossy(wide_slice)
            .trim_matches('\0')
            .trim()
            .to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    };

    unsafe {
        let path_wide: Vec<u16> = std::ffi::OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut handle: u32 = 0;
        let size = GetFileVersionInfoSizeW(path_wide.as_ptr(), &mut handle);
        if size == 0 {
            return VersionInfo::default();
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(path_wide.as_ptr(), 0, size, data.as_mut_ptr() as *mut c_void) == 0 {
            return VersionInfo::default();
        }
        let data_ptr = data.as_ptr() as *const c_void;

        // 번역(언어/코드페이지) 목록에서 첫 항목을 사용한다.
        let mut prefixes: Vec<String> = Vec::new();
        {
            let mut buffer: *mut c_void = std::ptr::null_mut();
            let mut length: u32 = 0;
            let block = wide("\\VarFileInfo\\Translation");
            if VerQueryValueW(data_ptr, block.as_ptr(), &mut buffer, &mut length) != 0
                && !buffer.is_null()
                && length >= 4
            {
                let values = std::slice::from_raw_parts(buffer as *const u16, (length / 2) as usize);
                let language = values[0];
                let codepage = values[1];
                prefixes.push(format!("{language:04X}{codepage:04X}"));
            }
        }
        prefixes.push("040904B0".to_string());
        prefixes.push("000004B0".to_string());

        let mut info = VersionInfo::default();
        for prefix in prefixes {
            if info.product_name.is_none() {
                info.product_name = query(
                    data_ptr,
                    &format!("\\StringFileInfo\\{prefix}\\ProductName"),
                );
            }
            if info.file_description.is_none() {
                info.file_description = query(
                    data_ptr,
                    &format!("\\StringFileInfo\\{prefix}\\FileDescription"),
                );
            }
            if info.company_name.is_none() {
                info.company_name =
                    query(data_ptr, &format!("\\StringFileInfo\\{prefix}\\CompanyName"));
            }
            if info.product_name.is_some()
                && info.file_description.is_some()
                && info.company_name.is_some()
            {
                break;
            }
        }
        info
    }
}

#[cfg(not(windows))]
fn read_version_info(path: &str) -> VersionInfo {
    let _ = path;
    VersionInfo::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_entry_valid_rejects_pid_reuse() {
        assert!(cache_entry_valid(Some(100), Some(100)));
        assert!(!cache_entry_valid(Some(100), Some(200)));
        assert!(cache_entry_valid(None, None));
        assert!(!cache_entry_valid(Some(100), None));
        assert!(!cache_entry_valid(None, Some(100)));
    }

    #[test]
    fn chooses_display_name_from_version_info() {
        let meta = ProcMeta {
            name: "game.exe".into(),
            version: VersionInfo {
                file_description: Some("Granblue Fantasy: Relink".into()),
                product_name: Some("Granblue Fantasy: Relink".into()),
                company_name: Some("Cygames".into()),
            },
            ..Default::default()
        };
        assert_eq!(meta.display_name(), "Granblue Fantasy: Relink");
    }

    #[test]
    fn falls_back_to_executable_name() {
        let meta = ProcMeta {
            name: "eldenring.exe".into(),
            version: VersionInfo::default(),
            ..Default::default()
        };
        assert_eq!(meta.display_name(), "eldenring.exe");
        assert_eq!(meta.base_name(), "eldenring.exe");
    }

    #[test]
    fn ancestors_walk_is_cycle_safe() {
        let mut identities = HashMap::new();
        identities.insert(
            10,
            ProcIdentity {
                name: "python.exe".into(),
                parent: Some(20),
            },
        );
        identities.insert(
            20,
            ProcIdentity {
                name: "laya-serve.exe".into(),
                parent: Some(30),
            },
        );
        identities.insert(
            30,
            ProcIdentity {
                name: "explorer.exe".into(),
                parent: Some(40),
            },
        );
        // 순환(40 -> 20)
        identities.insert(
            40,
            ProcIdentity {
                name: "loop.exe".into(),
                parent: Some(20),
            },
        );
        assert_eq!(MetaCache::ancestors(10, &identities), vec![20, 30, 40]);
    }
}
