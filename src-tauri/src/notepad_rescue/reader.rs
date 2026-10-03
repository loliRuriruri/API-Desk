use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::error::AppError;

/// Windows 파일 공유 플래그 (공유 읽기, 쓰기, 삭제 허용)
#[cfg(target_os = "windows")]
const FILE_SHARE_READ: u32 = 0x00000001;
#[cfg(target_os = "windows")]
const FILE_SHARE_WRITE: u32 = 0x00000002;
#[cfg(target_os = "windows")]
const FILE_SHARE_DELETE: u32 = 0x00000004;

/// 안전하게 파일을 읽기 전용으로 열어 파일 핸들을 반환합니다.
/// Windows에서는 Notepad가 배타적 잠금을 걸지 않도록 완전 공유 모드를 사용합니다.
fn open_readonly_shared(path: &Path) -> Result<File, std::io::Error> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
        options.open(path)
    }

    #[cfg(not(target_os = "windows"))]
    {
        File::open(path)
    }
}

/// 파일의 메타데이터(크기 및 수정 시각)를 조회합니다.
fn read_file_meta(file: &File) -> (u64, Option<std::time::SystemTime>) {
    file.metadata()
        .map(|m| (m.len(), m.modified().ok()))
        .unwrap_or((0, None))
}

/// Notepad가 열어둔 상태에서도 안전하게 파일을 읽어오는 함수.
/// 동시 쓰기로 인한 일관성 깨짐을 감지하기 위해 읽기 전후의 메타데이터를 비교하며,
/// 불일치 시 최대 3회까지 재시도합니다. 절대 Notepad를 차단하지 않습니다.
pub fn safe_read_file(path: &Path) -> Result<Vec<u8>, AppError> {
    const MAX_ATTEMPTS: usize = 3;
    const RETRY_DELAY: Duration = Duration::from_millis(30);

    for attempt in 1..=MAX_ATTEMPTS {
        let mut file = match open_readonly_shared(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AppError::Io(format!(
                    "파일이 존재하지 않습니다: {}",
                    path.display()
                )));
            }
            Err(e) => {
                if attempt == MAX_ATTEMPTS {
                    return Err(AppError::Io(format!(
                        "파일 열기 실패 ({}회 시도): {}: {e}",
                        MAX_ATTEMPTS,
                        path.display()
                    )));
                }
                thread::sleep(RETRY_DELAY);
                continue;
            }
        };

        let (size_before, mod_before) = read_file_meta(&file);

        // 빈 파일인 경우 즉시 빈 버퍼 반환
        if size_before == 0 {
            let (size_after, _) = read_file_meta(&file);
            if size_after == 0 {
                return Ok(Vec::new());
            }
        }

        let mut buffer = Vec::with_capacity(size_before.min(64 * 1024 * 1024) as usize);
        if let Err(e) = file.read_to_end(&mut buffer) {
            if attempt == MAX_ATTEMPTS {
                return Err(AppError::Io(format!(
                    "파일 읽기 실패: {}: {e}",
                    path.display()
                )));
            }
            thread::sleep(RETRY_DELAY);
            continue;
        }

        let (size_after, mod_after) = read_file_meta(&file);

        // 읽는 중간에 크기나 수정 시각이 변경된 경우 동시 쓰기 발생으로 간주하여 재시도
        if size_before != size_after || mod_before != mod_after {
            if attempt < MAX_ATTEMPTS {
                thread::sleep(RETRY_DELAY);
                continue;
            }
            return Err(AppError::Io(format!(
                "파일 읽기 일관성 검증 실패 (동시 수정 감지): {}",
                path.display()
            )));
        }

        return Ok(buffer);
    }

    Err(AppError::Io(format!(
        "파일 읽기 재시도 초과: {}",
        path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_read_normal_file() {
        let temp_dir = std::env::temp_dir().join("api-desk-reader-normal");
        let _ = std::fs::create_dir_all(&temp_dir);
        let file_path = temp_dir.join("normal.bin");

        let expected_data = b"Hello Notepad Rescue Safe Reader!";
        std::fs::write(&file_path, expected_data).unwrap();

        let read_data = safe_read_file(&file_path).unwrap();
        assert_eq!(read_data, expected_data);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_read_empty_file() {
        let temp_dir = std::env::temp_dir().join("api-desk-reader-empty");
        let _ = std::fs::create_dir_all(&temp_dir);
        let file_path = temp_dir.join("empty.bin");

        std::fs::write(&file_path, b"").unwrap();

        let read_data = safe_read_file(&file_path).unwrap();
        assert!(read_data.is_empty());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_read_missing_file() {
        let missing_path = std::env::temp_dir().join("api-desk-nonexistent-file.bin");
        let result = safe_read_file(&missing_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_concurrent_shared_file() {
        let temp_dir = std::env::temp_dir().join("api-desk-reader-shared");
        let _ = std::fs::create_dir_all(&temp_dir);
        let file_path = temp_dir.join("shared.bin");

        let initial = b"Initial Shared Content";
        std::fs::write(&file_path, initial).unwrap();

        // 다른 프로세스(또는 스레드)가 파일 핸들을 열고 있는 상황 모사
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let mut writer_opts = std::fs::OpenOptions::new();
            writer_opts.append(true);
            writer_opts.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
            let mut writer = writer_opts.open(&file_path).unwrap();

            // writer가 열려 있는 상태에서 safe_read_file 실행
            let read_data = safe_read_file(&file_path).unwrap();
            assert_eq!(read_data, initial);

            // 추가 쓰기 후 다시 읽기
            writer.write_all(b" - Appended").unwrap();
            writer.flush().unwrap();

            let read_updated = safe_read_file(&file_path).unwrap();
            assert_eq!(read_updated, b"Initial Shared Content - Appended");
        }

        #[cfg(not(target_os = "windows"))]
        {
            let read_data = safe_read_file(&file_path).unwrap();
            assert_eq!(read_data, initial);
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
