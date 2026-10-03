use std::path::Path;

use crc32fast::Hasher;

use super::models::{ParseStatus, ParsedTabState, ParsedWindowState};

/// Notepad TabState/WindowState 매직 바이트: "NP\0"
pub const NOTEPAD_MAGIC: &[u8; 3] = b"NP\0";

/// uLEB128(Unsigned Little Endian Base 128) 정수를 가변 길이 바이트열로부터 디코딩합니다.
pub fn decode_uleb128(data: &[u8], offset: &mut usize) -> Result<u64, ParseError> {
    let mut result: u64 = 0;
    let mut shift = 0;
    let start_offset = *offset;

    while *offset < data.len() {
        let byte = data[*offset];
        *offset += 1;

        let val = (byte & 0x7F) as u64;
        if shift > 63 || (shift == 63 && val > 1) {
            return Err(ParseError::Overflow);
        }

        result |= val << shift;

        if (byte & 0x80) == 0 {
            return Ok(result);
        }

        shift += 7;
        if *offset - start_offset > 10 {
            return Err(ParseError::MalformedUleb);
        }
    }

    Err(ParseError::Truncated)
}

/// 정수를 uLEB128 포맷으로 버퍼에 인코딩합니다.
pub fn encode_uleb128(mut value: u64, buf: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// UTF-16LE 바이트 슬라이스를 String으로 안전하게 디코딩합니다.
pub fn decode_utf16_le(
    data: &[u8],
    offset: &mut usize,
    char_count: usize,
) -> Result<String, ParseError> {
    let byte_count = char_count.checked_mul(2).ok_or(ParseError::Overflow)?;

    if *offset + byte_count > data.len() {
        return Err(ParseError::Truncated);
    }

    let slice = &data[*offset..*offset + byte_count];
    *offset += byte_count;

    let u16_vec: Vec<u16> = slice
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    Ok(String::from_utf16_lossy(&u16_vec))
}

/// 문자열을 UTF-16LE 바이트로 변환하여 버퍼에 추가합니다.
pub fn encode_utf16_le(s: &str, buf: &mut Vec<u8>) {
    for code_unit in s.encode_utf16() {
        buf.extend_from_slice(&code_unit.to_le_bytes());
    }
}

/// Windows 11 Notepad TabState 표준 CRC32를 계산합니다.
/// 계산 범위: data[3 .. data.len() - 4]
pub fn compute_tabstate_crc32(data: &[u8]) -> Option<u32> {
    if data.len() < 8 {
        return None;
    }
    let range = &data[3..data.len() - 4];
    let mut hasher = Hasher::new();
    hasher.update(range);
    Some(hasher.finalize())
}

/// 파일 말미의 4바이트 저장된 CRC32 값을 추출합니다.
pub fn extract_stored_crc32(data: &[u8]) -> Option<u32> {
    if data.len() < 8 {
        return None;
    }
    let last4 = &data[data.len() - 4..];
    Some(u32::from_be_bytes([last4[0], last4[1], last4[2], last4[3]]))
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Truncated,
    MalformedUleb,
    Overflow,
    UnsupportedFormat,
    Corrupt,
}

/// TabState 바이너리를 해석하여 탭 메타데이터 및 복구된 텍스트를 추출합니다.
/// 어떤 상황에서도 패닉하지 않고 방어적으로 동작합니다.
pub fn parse_tab_state(source_id: &str, data: &[u8]) -> ParsedTabState {
    let mut result = ParsedTabState {
        source_id: source_id.to_string(),
        original_path: None,
        display_title: "제목 없음".to_string(),
        is_saved: false,
        recovered_text: None,
        format_version: Some(1),
        parse_status: ParseStatus::Pending,
    };

    // 1. 최소 길이 및 매직 바이트 검증 ("NP\0")
    if data.len() < 4 {
        result.parse_status = ParseStatus::Corrupt;
        return result;
    }

    if &data[0..3] != NOTEPAD_MAGIC {
        result.parse_status = ParseStatus::UnsupportedFormat;
        return result;
    }

    // 2. CRC32 검증
    let mut crc_mismatch = false;
    if data.len() >= 8 {
        if let (Some(computed), Some(stored)) =
            (compute_tabstate_crc32(data), extract_stored_crc32(data))
        {
            if computed != stored {
                crc_mismatch = true;
            }
        }
    }

    let flag = data[3];
    let mut offset = 4;

    match flag {
        0x00 => {
            // Unsaved Tab (미저장 탭)
            result.is_saved = false;

            match parse_unsaved_content(data, &mut offset) {
                Ok(text) => {
                    result.display_title = derive_display_title(None, &text);
                    result.recovered_text = Some(text);
                    result.parse_status = if crc_mismatch {
                        ParseStatus::Corrupt
                    } else {
                        ParseStatus::Parsed
                    };
                }
                Err(_) => {
                    result.parse_status = ParseStatus::Corrupt;
                }
            }
        }
        0x01 => {
            // Saved Tab (저장된 탭)
            result.is_saved = true;

            match parse_saved_content(data, &mut offset) {
                Ok((path, text_opt)) => {
                    let title =
                        derive_display_title(Some(&path), text_opt.as_deref().unwrap_or(""));
                    result.original_path = Some(path);
                    result.display_title = title;
                    result.recovered_text = text_opt;
                    result.parse_status = if crc_mismatch {
                        ParseStatus::Corrupt
                    } else {
                        ParseStatus::Parsed
                    };
                }
                Err(_) => {
                    result.parse_status = ParseStatus::Corrupt;
                }
            }
        }
        _ => {
            // 알 수 없는 플래그
            result.parse_status = ParseStatus::UnsupportedFormat;
        }
    }

    result
}

/// 미저장 탭 본문 파싱
fn parse_unsaved_content(data: &[u8], offset: &mut usize) -> Result<String, ParseError> {
    // sequence byte (1)
    if *offset >= data.len() {
        return Err(ParseError::Truncated);
    }
    *offset += 1;

    // selection start, end
    let _sel_start = decode_uleb128(data, offset)?;
    let _sel_end = decode_uleb128(data, offset)?;

    // config block (7 bytes)
    if *offset + 7 > data.len() {
        return Err(ParseError::Truncated);
    }
    *offset += 7;

    // content length in UTF-16 chars
    let content_chars = decode_uleb128(data, offset)? as usize;

    let text = decode_utf16_le(data, offset, content_chars)?;
    Ok(text)
}

/// 저장된 탭 경로 및 캐시 본문 파싱
fn parse_saved_content(
    data: &[u8],
    offset: &mut usize,
) -> Result<(String, Option<String>), ParseError> {
    // path length in UTF-16 chars
    let path_chars = decode_uleb128(data, offset)? as usize;
    let path_str = decode_utf16_le(data, offset, path_chars)?;

    // file metadata: file_size(uleb128), encoding(1), cr_type(1), last_write_time(8), sha256(32)
    let _file_size = decode_uleb128(data, offset)?;
    if *offset + 1 + 1 + 8 + 32 > data.len() {
        return Err(ParseError::Truncated);
    }
    *offset += 1 + 1 + 8 + 32;

    // sequence byte (1)
    if *offset >= data.len() {
        return Err(ParseError::Truncated);
    }
    *offset += 1;

    // selection start, end
    let _sel_start = decode_uleb128(data, offset)?;
    let _sel_end = decode_uleb128(data, offset)?;

    // config block (7 bytes)
    if *offset + 7 > data.len() {
        return Err(ParseError::Truncated);
    }
    *offset += 7;

    // content length in UTF-16 chars
    let content_chars = decode_uleb128(data, offset)? as usize;

    if content_chars > 0 {
        let text = decode_utf16_le(data, offset, content_chars)?;
        Ok((path_str, Some(text)))
    } else {
        Ok((path_str, None))
    }
}

/// 원본 파일 경로 또는 본문 텍스트로부터 사용자 친화적 탭 제목을 도출합니다.
pub fn derive_display_title(path: Option<&str>, text: &str) -> String {
    if let Some(p) = path {
        if let Some(filename) = Path::new(p).file_name().and_then(|n| n.to_str()) {
            if !filename.trim().is_empty() {
                return filename.to_string();
            }
        }
    }

    let first_line = text
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("");

    if first_line.is_empty() {
        "제목 없음".to_string()
    } else {
        // 최대 40자로 말줄임
        let mut chars = first_line.chars();
        let truncated: String = chars.by_ref().take(40).collect();
        if chars.next().is_some() {
            format!("{truncated}...")
        } else {
            truncated
        }
    }
}

/// WindowState 바이너리 헤더를 검증합니다.
pub fn parse_window_state(source_id: &str, data: &[u8]) -> ParsedWindowState {
    if data.len() >= 3 && &data[0..3] == NOTEPAD_MAGIC {
        ParsedWindowState {
            source_id: source_id.to_string(),
            parse_status: ParseStatus::Parsed,
        }
    } else {
        ParsedWindowState {
            source_id: source_id.to_string(),
            parse_status: ParseStatus::UnsupportedFormat,
        }
    }
}

// ---------------------------------------------------------------- 테스트용 Synthetic Fixture 생성기

/// 테스트용 Unsaved TabState 바이너리를 합성 생성합니다.
pub fn generate_synthetic_unsaved(content: &str, corrupt_crc: bool) -> Vec<u8> {
    let mut buf = Vec::new();
    // Magic: NP\0
    buf.extend_from_slice(NOTEPAD_MAGIC);
    // Flag: 0x00 (Unsaved)
    buf.push(0x00);
    // Sequence: 0x01
    buf.push(0x01);
    // Selection start = 0
    encode_uleb128(0, &mut buf);
    // Selection end = 0
    encode_uleb128(0, &mut buf);
    // Config block (7 bytes)
    buf.extend_from_slice(&[0x01, 0x00, 0x00, 0x03, 0x01, 0x01, 0x01]);
    // Content length in UTF-16 code units
    let char_count = content.encode_utf16().count() as u64;
    encode_uleb128(char_count, &mut buf);
    // Content UTF-16LE
    encode_utf16_le(content, &mut buf);
    // Unsaved data flag: 0x00
    buf.push(0x00);

    // CRC32
    let mut hasher = Hasher::new();
    hasher.update(&buf[3..]);
    let crc = hasher.finalize();

    let crc_to_write = if corrupt_crc { crc ^ 0xFFFFFFFF } else { crc };
    buf.extend_from_slice(&crc_to_write.to_be_bytes());

    buf
}

/// 테스트용 Saved TabState 바이너리를 합성 생성합니다.
pub fn generate_synthetic_saved(path: &str, content: &str, corrupt_crc: bool) -> Vec<u8> {
    let mut buf = Vec::new();
    // Magic: NP\0
    buf.extend_from_slice(NOTEPAD_MAGIC);
    // Flag: 0x01 (Saved)
    buf.push(0x01);
    // Path length in UTF-16 code units
    let path_char_count = path.encode_utf16().count() as u64;
    encode_uleb128(path_char_count, &mut buf);
    // Path UTF-16LE
    encode_utf16_le(path, &mut buf);

    // File metadata: size(uleb128), encoding(1), cr_type(1), last_write_time(8), sha256(32)
    encode_uleb128(content.len() as u64, &mut buf);
    buf.push(0x01); // encoding
    buf.push(0x01); // cr_type
    buf.extend_from_slice(&[0u8; 8]); // last_write_time
    buf.extend_from_slice(&[0u8; 32]); // sha256

    // Sequence: 0x01
    buf.push(0x01);
    // Selection start, end
    encode_uleb128(0, &mut buf);
    encode_uleb128(0, &mut buf);
    // Config block (7 bytes)
    buf.extend_from_slice(&[0x01, 0x00, 0x00, 0x03, 0x01, 0x01, 0x01]);

    // Content length in UTF-16 code units
    let content_char_count = content.encode_utf16().count() as u64;
    encode_uleb128(content_char_count, &mut buf);
    if content_char_count > 0 {
        encode_utf16_le(content, &mut buf);
    }
    // Unsaved data flag: 0x00
    buf.push(0x00);

    // CRC32
    let mut hasher = Hasher::new();
    hasher.update(&buf[3..]);
    let crc = hasher.finalize();

    let crc_to_write = if corrupt_crc { crc ^ 0xFFFFFFFF } else { crc };
    buf.extend_from_slice(&crc_to_write.to_be_bytes());

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uleb128_roundtrip() {
        let test_values = [0u64, 1, 127, 128, 255, 300, 16384, 1_000_000, u64::MAX >> 1];
        for val in test_values {
            let mut buf = Vec::new();
            encode_uleb128(val, &mut buf);
            let mut offset = 0;
            let decoded = decode_uleb128(&buf, &mut offset).unwrap();
            assert_eq!(decoded, val);
            assert_eq!(offset, buf.len());
        }
    }

    #[test]
    fn test_utf16_le_roundtrip() {
        let texts = [
            "Hello World",
            "안녕하세요 메모장 복구 테스트",
            "日本語テキストテスト",
            "Emojis 🚀✨💻",
            "",
        ];
        for text in texts {
            let mut buf = Vec::new();
            encode_utf16_le(text, &mut buf);
            let mut offset = 0;
            let char_count = text.encode_utf16().count();
            let decoded = decode_utf16_le(&buf, &mut offset, char_count).unwrap();
            assert_eq!(decoded, text);
            assert_eq!(offset, buf.len());
        }
    }

    // 11. Valid unsaved tabstate
    #[test]
    fn test_parse_valid_unsaved() {
        let bin = generate_synthetic_unsaved("Hello Unsaved Note", false);
        let parsed = parse_tab_state("tab-1", &bin);

        assert_eq!(parsed.source_id, "tab-1");
        assert!(!parsed.is_saved);
        assert_eq!(parsed.original_path, None);
        assert_eq!(parsed.display_title, "Hello Unsaved Note");
        assert_eq!(parsed.recovered_text.as_deref(), Some("Hello Unsaved Note"));
        assert_eq!(parsed.parse_status, ParseStatus::Parsed);
    }

    // 12. Valid saved tabstate
    #[test]
    fn test_parse_valid_saved() {
        let bin = generate_synthetic_saved("C:\\docs\\todo.txt", "Buy milk", false);
        let parsed = parse_tab_state("tab-2", &bin);

        assert_eq!(parsed.source_id, "tab-2");
        assert!(parsed.is_saved);
        assert_eq!(parsed.original_path.as_deref(), Some("C:\\docs\\todo.txt"));
        assert_eq!(parsed.display_title, "todo.txt");
        assert_eq!(parsed.recovered_text.as_deref(), Some("Buy milk"));
        assert_eq!(parsed.parse_status, ParseStatus::Parsed);
    }

    // 13. Korean unicode text
    #[test]
    fn test_parse_korean_text() {
        let content = "첫 번째 줄: 긴급 작업 메모\n두 번째 줄: 정상 복구 확인";
        let bin = generate_synthetic_unsaved(content, false);
        let parsed = parse_tab_state("tab-ko", &bin);

        assert_eq!(parsed.display_title, "첫 번째 줄: 긴급 작업 메모");
        assert_eq!(parsed.recovered_text.as_deref(), Some(content));
        assert_eq!(parsed.parse_status, ParseStatus::Parsed);
    }

    // 14. Emojis and surrogate pairs
    #[test]
    fn test_parse_emoji_surrogates() {
        let content = "아이콘 모음 🚀🎉🔥💻";
        let bin = generate_synthetic_unsaved(content, false);
        let parsed = parse_tab_state("tab-emoji", &bin);

        assert_eq!(parsed.recovered_text.as_deref(), Some(content));
        assert_eq!(parsed.parse_status, ParseStatus::Parsed);
    }

    // 15. Empty text tabstate
    #[test]
    fn test_parse_empty_text() {
        let bin = generate_synthetic_unsaved("", false);
        let parsed = parse_tab_state("tab-empty", &bin);

        assert_eq!(parsed.display_title, "제목 없음");
        assert_eq!(parsed.recovered_text.as_deref(), Some(""));
        assert_eq!(parsed.parse_status, ParseStatus::Parsed);
    }

    // 16. Bad magic header
    #[test]
    fn test_parse_bad_magic() {
        let mut bin = generate_synthetic_unsaved("Some text", false);
        bin[0] = b'X';
        bin[1] = b'X';
        let parsed = parse_tab_state("tab-bad-magic", &bin);

        assert_eq!(parsed.parse_status, ParseStatus::UnsupportedFormat);
    }

    // 17. Truncated data
    #[test]
    fn test_parse_truncated_data() {
        let bin = generate_synthetic_unsaved("Hello", false);
        let truncated = &bin[0..10]; // 중간에서 잘림
        let parsed = parse_tab_state("tab-trunc", truncated);

        assert_eq!(parsed.parse_status, ParseStatus::Corrupt);
    }

    // 18. Malformed uLEB128
    #[test]
    fn test_parse_malformed_uleb128() {
        // 모든 바이트가 0x80인 15바이트 슬라이스
        let bad_uleb = vec![0x80u8; 15];
        let mut offset = 0;
        let res = decode_uleb128(&bad_uleb, &mut offset);
        assert!(matches!(
            res,
            Err(ParseError::MalformedUleb) | Err(ParseError::Truncated) | Err(ParseError::Overflow)
        ));
    }

    // 19. CRC mismatch
    #[test]
    fn test_parse_crc_mismatch() {
        let bin = generate_synthetic_unsaved("Content with CRC mismatch", true);
        let parsed = parse_tab_state("tab-crc-err", &bin);

        assert_eq!(parsed.parse_status, ParseStatus::Corrupt);
        // 복구된 텍스트는 최선으로 유지됨
        assert_eq!(
            parsed.recovered_text.as_deref(),
            Some("Content with CRC mismatch")
        );
    }

    // 20. Unknown format flag
    #[test]
    fn test_parse_unknown_version_flag() {
        let mut bin = generate_synthetic_unsaved("Test", false);
        bin[3] = 0xFE; // 알 수 없는 버전/플래그
        let parsed = parse_tab_state("tab-unknown", &bin);

        assert_eq!(parsed.parse_status, ParseStatus::UnsupportedFormat);
    }

    // WindowState parse test
    #[test]
    fn test_parse_window_state() {
        let valid_win = [b'N', b'P', 0x00, 0x2C, 0x00];
        let parsed_valid = parse_window_state("win-1", &valid_win);
        assert_eq!(parsed_valid.parse_status, ParseStatus::Parsed);

        let invalid_win = [b'B', b'A', b'D'];
        let parsed_invalid = parse_window_state("win-2", &invalid_win);
        assert_eq!(parsed_invalid.parse_status, ParseStatus::UnsupportedFormat);
    }
}
