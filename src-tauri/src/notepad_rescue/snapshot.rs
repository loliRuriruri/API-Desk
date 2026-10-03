use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::error::AppError;

/// 원본 바이너리 바이트열의 SHA-256 해시(64자리 16진수 문자열)를 계산합니다.
pub fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    format!("{:x}", result)
}

/// 해시값을 기반으로 스냅샷의 상대 경로를 반환합니다.
/// 2글자 prefix 디렉터리를 두어 단일 디렉터리 내 과도한 파일 누적을 방지합니다.
/// e.g. "snapshots/e3/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855.bin"
pub fn relative_blob_path(hash: &str) -> String {
    if hash.len() < 2 {
        return format!("snapshots/{hash}.bin");
    }
    let prefix = &hash[..2];
    format!("snapshots/{prefix}/{hash}.bin")
}

/// 스토리지 루트와 상대 경로를 결합하여 실제 파일시스템 절대 경로를 구합니다.
pub fn resolve_blob_path(storage_root: &Path, relative_path: &str) -> PathBuf {
    storage_root.join(relative_path)
}

/// Content-Addressed Storage에 바이너리 블롭을 원자적으로 저장합니다.
/// 동일한 SHA-256 해시의 파일이 이미 존재하면 물리적 중복 저장을 건너뛰고 기존 상대 경로를 반환합니다.
pub fn store_blob(storage_root: &Path, hash: &str, data: &[u8]) -> Result<String, AppError> {
    let rel_path = relative_blob_path(hash);
    let abs_path = resolve_blob_path(storage_root, &rel_path);

    // 1. 이미 동일 해시의 물리 블롭이 존재하면 재작성하지 않고 즉시 성공 반환 (Deduplication)
    if abs_path.exists() {
        if let Ok(meta) = fs::metadata(&abs_path) {
            if meta.len() == data.len() as u64 {
                return Ok(rel_path);
            }
        }
    }

    // 2. 대상 디렉터리 생성 (snapshots/ab/)
    let parent_dir = abs_path.parent().ok_or_else(|| {
        AppError::Io(format!(
            "유효하지 않은 스토리지 경로입니다: {}",
            abs_path.display()
        ))
    })?;
    fs::create_dir_all(parent_dir).map_err(|e| {
        AppError::Io(format!(
            "스냅샷 디렉터리 생성 실패 ({}): {e}",
            parent_dir.display()
        ))
    })?;

    // 3. 임시 파일에 쓰기 후 원자적 이름 변경 (Atomic Publish)
    let tmp_path = parent_dir.join(format!("{hash}.{}.tmp", std::process::id()));
    {
        let mut tmp_file = File::create(&tmp_path).map_err(|e| {
            AppError::Io(format!(
                "임시 스냅샷 파일 생성 실패 ({}): {e}",
                tmp_path.display()
            ))
        })?;
        tmp_file.write_all(data).map_err(|e| {
            AppError::Io(format!(
                "스냅샷 데이터 기록 실패 ({}): {e}",
                tmp_path.display()
            ))
        })?;
        tmp_file.sync_all().map_err(|e| {
            AppError::Io(format!(
                "스냅샷 디스크 동기화 실패 ({}): {e}",
                tmp_path.display()
            ))
        })?;
    }

    // 4. rename 수행
    if let Err(e) = fs::rename(&tmp_path, &abs_path) {
        // 동시 실행 등으로 대상 파일이 이미 생성되었는지 확인
        if abs_path.exists() {
            let _ = fs::remove_file(&tmp_path);
            return Ok(rel_path);
        }
        let _ = fs::remove_file(&tmp_path);
        return Err(AppError::Io(format!(
            "스냅샷 원자적 저장 실패 ({} -> {}): {e}",
            tmp_path.display(),
            abs_path.display()
        )));
    }

    Ok(rel_path)
}

/// 비정상 종료 등으로 남은 오래된 임시 파일(.tmp)을 정리합니다.
pub fn cleanup_orphaned_tmp(storage_root: &Path, max_age: Duration) -> Result<usize, AppError> {
    let snapshots_dir = storage_root.join("snapshots");
    if !snapshots_dir.exists() {
        return Ok(0);
    }

    let mut removed_count = 0;
    let now = SystemTime::now();

    let walker = match fs::read_dir(&snapshots_dir) {
        Ok(w) => w,
        Err(_) => return Ok(0),
    };

    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Ok(sub_entries) = fs::read_dir(&path) {
                for sub_entry in sub_entries.flatten() {
                    let sub_path = sub_entry.path();
                    if sub_path.extension().and_then(|s| s.to_str()) == Some("tmp") {
                        if let Ok(meta) = sub_entry.metadata() {
                            if let Ok(modified) = meta.modified() {
                                if let Ok(age) = now.duration_since(modified) {
                                    if age >= max_age {
                                        if fs::remove_file(&sub_path).is_ok() {
                                            removed_count += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else if path.extension().and_then(|s| s.to_str()) == Some("tmp") {
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if let Ok(age) = now.duration_since(modified) {
                        if age >= max_age {
                            if fs::remove_file(&path).is_ok() {
                                removed_count += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(removed_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_sha256() {
        let data = b"Hello Notepad Rescue!";
        let hash = compute_sha256(data);
        assert_eq!(hash.len(), 64);
        assert_eq!(
            hash,
            "4a4c8132a7f47ca5678476a5512ae30067224cc2ea50a872cfa0a11d50d37d38"
        );
    }

    #[test]
    fn test_relative_blob_path() {
        let hash = "aa5045464522fb4864f1c713b1fce52626e2a229a4371fa7ff0531c034b07d67";
        let rel = relative_blob_path(hash);
        assert_eq!(
            rel,
            "snapshots/aa/aa5045464522fb4864f1c713b1fce52626e2a229a4371fa7ff0531c034b07d67.bin"
        );
    }

    #[test]
    fn test_store_blob_first_time() {
        let temp_dir = std::env::temp_dir().join("api-desk-snap-test-1");
        let _ = fs::remove_dir_all(&temp_dir);

        let data = b"Test Content for Snapshot";
        let hash = compute_sha256(data);

        let rel_path = store_blob(&temp_dir, &hash, data).unwrap();
        assert_eq!(rel_path, relative_blob_path(&hash));

        let abs_path = resolve_blob_path(&temp_dir, &rel_path);
        assert!(abs_path.exists());
        assert_eq!(fs::read(&abs_path).unwrap(), data);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_store_blob_physical_deduplication() {
        let temp_dir = std::env::temp_dir().join("api-desk-snap-test-dedup");
        let _ = fs::remove_dir_all(&temp_dir);

        let data = b"Identical Data across revisions";
        let hash = compute_sha256(data);

        // 첫 번째 저장
        let rel_path_1 = store_blob(&temp_dir, &hash, data).unwrap();
        let abs_path = resolve_blob_path(&temp_dir, &rel_path_1);
        let meta1 = fs::metadata(&abs_path).unwrap();
        let mod_time_1 = meta1.modified().unwrap();

        std::thread::sleep(Duration::from_millis(50));

        // 두 번째 저장 (중복 호출)
        let rel_path_2 = store_blob(&temp_dir, &hash, data).unwrap();
        assert_eq!(rel_path_1, rel_path_2);

        let meta2 = fs::metadata(&abs_path).unwrap();
        let mod_time_2 = meta2.modified().unwrap();

        // 파일이 다시 작성되지 않았으므로 수정 시각이 그대로 유지됨
        assert_eq!(mod_time_1, mod_time_2);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cleanup_orphaned_tmp() {
        let temp_dir = std::env::temp_dir().join("api-desk-snap-test-clean");
        let _ = fs::remove_dir_all(&temp_dir);

        let snapshots_prefix = temp_dir.join("snapshots").join("ab");
        fs::create_dir_all(&snapshots_prefix).unwrap();

        let valid_bin = snapshots_prefix.join("abcdef.bin");
        fs::write(&valid_bin, b"valid binary").unwrap();

        let orphan_tmp = snapshots_prefix.join("abcdef.1234.tmp");
        fs::write(&orphan_tmp, b"incomplete partial write").unwrap();

        // 0초 초과 파일 정리 (즉시 삭제)
        let cleaned = cleanup_orphaned_tmp(&temp_dir, Duration::from_millis(0)).unwrap();
        assert_eq!(cleaned, 1);
        assert!(valid_bin.exists());
        assert!(!orphan_tmp.exists());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
