use base64::Engine;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::Manager;

use crate::error::AppError;

pub const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024; // 25 MB limit

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomBackgroundPayload {
    pub path: String,
    pub data_url: String,
}

pub fn detect_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        return Some("image/png");
    }
    if bytes.len() >= 3 && &bytes[0..3] == b"\xff\xd8\xff" {
        return Some("image/jpeg");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

pub fn bytes_to_data_url(bytes: &[u8], mime: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:{mime};base64,{b64}")
}

pub fn get_background_dir(app: &tauri::AppHandle) -> Result<PathBuf, AppError> {
    let local_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| AppError::Io(e.to_string()))?;
    let bg_dir = local_dir.join("backgrounds");
    fs::create_dir_all(&bg_dir)
        .map_err(|e| AppError::Io(format!("배경 저장 디렉터리 생성 실패: {e}")))?;
    Ok(bg_dir)
}

#[tauri::command]
pub fn custom_background_set(
    app: tauri::AppHandle,
    source_path: String,
) -> Result<CustomBackgroundPayload, AppError> {
    let src = Path::new(&source_path);
    if !src.is_file() {
        return Err(AppError::InvalidRequest(format!(
            "선택한 이미지 파일을 찾을 수 없습니다: {source_path}"
        )));
    }

    let bytes = fs::read(src).map_err(|e| AppError::Io(format!("이미지 읽기 실패: {e}")))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(AppError::InvalidRequest(
            "이미지 파일 크기가 25MB를 초과합니다.".to_string(),
        ));
    }

    let mime = detect_mime_type(&bytes).ok_or_else(|| {
        AppError::InvalidRequest(
            "지원되지 않는 이미지 형식입니다. (PNG, JPG, WEBP만 가능)".to_string(),
        )
    })?;

    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    };

    let bg_dir = get_background_dir(&app)?;
    // 기존에 존재하던 custom_background.* 파일 정리
    if let Ok(entries) = fs::read_dir(&bg_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Some(name) = p.file_stem().and_then(|s| s.to_str()) {
                if name == "custom_background" {
                    let _ = fs::remove_file(p);
                }
            }
        }
    }

    let dest = bg_dir.join(format!("custom_background.{ext}"));
    fs::write(&dest, &bytes).map_err(|e| AppError::Io(format!("배경 이미지 저장 실패: {e}")))?;

    let data_url = bytes_to_data_url(&bytes, mime);
    Ok(CustomBackgroundPayload {
        path: dest.to_string_lossy().to_string(),
        data_url,
    })
}

#[tauri::command]
pub fn custom_background_load(
    app: tauri::AppHandle,
    path: Option<String>,
) -> Result<Option<String>, AppError> {
    let target_path = if let Some(p) = path {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            Some(pb)
        } else {
            None
        }
    } else {
        None
    };

    let resolved_path = match target_path {
        Some(p) => Some(p),
        None => {
            if let Ok(bg_dir) = get_background_dir(&app) {
                let candidates = [
                    "custom_background.png",
                    "custom_background.jpg",
                    "custom_background.jpeg",
                    "custom_background.webp",
                ];
                candidates
                    .iter()
                    .map(|c| bg_dir.join(c))
                    .find(|p| p.is_file())
            } else {
                None
            }
        }
    };

    let Some(file_path) = resolved_path else {
        return Ok(None);
    };

    let bytes = match fs::read(&file_path) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };

    let Some(mime) = detect_mime_type(&bytes) else {
        return Ok(None);
    };

    Ok(Some(bytes_to_data_url(&bytes, mime)))
}

#[tauri::command]
pub fn custom_background_delete(app: tauri::AppHandle) -> Result<(), AppError> {
    if let Ok(bg_dir) = get_background_dir(&app) {
        if let Ok(entries) = fs::read_dir(&bg_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if let Some(name) = p.file_stem().and_then(|s| s.to_str()) {
                    if name == "custom_background" {
                        let _ = fs::remove_file(p);
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_mime_type_png() {
        let png = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0x00];
        assert_eq!(detect_mime_type(&png), Some("image/png"));
    }

    #[test]
    fn test_detect_mime_type_jpeg() {
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10];
        assert_eq!(detect_mime_type(&jpeg), Some("image/jpeg"));
    }

    #[test]
    fn test_detect_mime_type_webp() {
        let webp = b"RIFF\x00\x00\x00\x00WEBPVP8 ";
        assert_eq!(detect_mime_type(webp), Some("image/webp"));
    }

    #[test]
    fn test_detect_mime_type_invalid() {
        let invalid = b"NOT AN IMAGE";
        assert_eq!(detect_mime_type(invalid), None);
    }

    #[test]
    fn test_bytes_to_data_url() {
        let data = b"hello image";
        let url = bytes_to_data_url(data, "image/png");
        assert!(url.starts_with("data:image/png;base64,"));
        let b64 = url.trim_start_matches("data:image/png;base64,");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_large_image_performance() {
        // 5 MB synthetic image payload
        let large_bytes = vec![0xABu8; 5 * 1024 * 1024];
        let start = std::time::Instant::now();
        let url = bytes_to_data_url(&large_bytes, "image/jpeg");
        let elapsed = start.elapsed();
        assert!(url.starts_with("data:image/jpeg;base64,"));
        // Base64 encoding 5MB in Rust typically takes < 15ms
        assert!(
            elapsed.as_millis() < 500,
            "Encoding 5MB took too long: {:?}",
            elapsed
        );
    }

    #[test]
    fn test_max_image_bytes_limit() {
        assert_eq!(MAX_IMAGE_BYTES, 25 * 1024 * 1024);
    }

    #[test]
    fn test_image_cleanup_and_replacement_lifecycle() {
        let tmp = std::env::temp_dir().join(format!("api_desk_bg_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // 1. Initial PNG
        let png = tmp.join("custom_background.png");
        fs::write(&png, b"png_data").unwrap();
        assert!(png.is_file());

        // 2. Simulate replacement with JPG (clean old custom_background.*)
        for entry in fs::read_dir(&tmp).unwrap().flatten() {
            let p = entry.path();
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                if stem == "custom_background" {
                    let _ = fs::remove_file(p);
                }
            }
        }
        let jpg = tmp.join("custom_background.jpg");
        fs::write(&jpg, b"jpg_data").unwrap();
        assert!(!png.exists());
        assert!(jpg.is_file());

        let files: Vec<_> = fs::read_dir(&tmp).unwrap().flatten().collect();
        assert_eq!(files.len(), 1, "Exactly 1 background image must exist");

        // 3. Cleanup on delete
        for entry in fs::read_dir(&tmp).unwrap().flatten() {
            let p = entry.path();
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                if stem == "custom_background" {
                    let _ = fs::remove_file(p);
                }
            }
        }
        let files_after: Vec<_> = fs::read_dir(&tmp).unwrap().flatten().collect();
        assert_eq!(files_after.len(), 0);

        let _ = fs::remove_dir_all(&tmp);
    }
}
