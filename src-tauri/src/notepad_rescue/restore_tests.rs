#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crc32fast::Hasher;

    use crate::notepad_rescue::models::{NotepadSnapshot, ParseStatus, SourceType};
    use crate::notepad_rescue::parser::generate_synthetic_unsaved;
    use crate::notepad_rescue::restore::*;
    use crate::notepad_rescue::snapshot::{compute_sha256, store_blob};

    fn test_dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("nr7-test-{name}-{}", generate_uuid_v4()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn test_generate_uuid_v4_format() {
        let u1 = generate_uuid_v4();
        let u2 = generate_uuid_v4();

        assert_ne!(u1, u2);
        assert_eq!(u1.len(), 36);
        assert_eq!(u2.len(), 36);

        // Check dashes
        let parts: Vec<&str> = u1.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);

        // Check Version 4
        assert_eq!(&parts[2][0..1], "4");
        // Check Variant 1 (8, 9, a, or b)
        let first_char = parts[3].chars().next().unwrap();
        assert!(matches!(first_char, '8' | '9' | 'a' | 'b'));
    }

    #[test]
    fn test_guid_str_to_bytes_le_accuracy() {
        // "6d73141e-0c18-4b63-bb5a-ab06c17de1b1"
        // Expected LE bytes: 1e 14 73 6d  18 0c  63 4b  bb 5a  ab 06 c1 7d e1 b1
        let guid_str = "6d73141e-0c18-4b63-bb5a-ab06c17de1b1";
        let bytes = guid_str_to_bytes_le(guid_str).expect("Failed to convert GUID");

        let expected: [u8; 16] = [
            0x1e, 0x14, 0x73, 0x6d, 0x18, 0x0c, 0x63, 0x4b, 0xbb, 0x5a, 0xab, 0x06, 0xc1, 0x7d,
            0xe1, 0xb1,
        ];
        assert_eq!(bytes, expected);

        // Braced format
        let braced = "{6d73141e-0c18-4b63-bb5a-ab06c17de1b1}";
        assert_eq!(guid_str_to_bytes_le(braced).unwrap(), expected);

        // Invalid format
        assert!(guid_str_to_bytes_le("invalid-guid").is_none());
        assert!(guid_str_to_bytes_le("12345678-1234-1234-1234").is_none());
    }

    #[test]
    fn test_build_window_state_content() {
        let tab_guid_le: [u8; 16] = [
            0x1e, 0x14, 0x73, 0x6d, 0x18, 0x0c, 0x63, 0x4b, 0xbb, 0x5a, 0xab, 0x06, 0xc1, 0x7d,
            0xe1, 0xb1,
        ];
        let ws = build_window_state_content(&tab_guid_le);

        assert_eq!(ws.len(), 52);
        assert_eq!(&ws[0..6], b"NP\x00\x2c\x00\x01");
        assert_eq!(&ws[6..22], &tab_guid_le);

        // Check CRC32 (stored at last 4 bytes)
        let mut hasher = Hasher::new();
        hasher.update(&ws[2..48]);
        let expected_crc = hasher.finalize();

        let stored_crc = u32::from_be_bytes([ws[48], ws[49], ws[50], ws[51]]);
        assert_eq!(stored_crc, expected_crc);
    }

    #[test]
    fn test_atomic_write() {
        let dir = test_dir("atomic");
        let target = dir.join("target.bin");
        let data = b"hello atomic write world!";

        atomic_write(&target, data).unwrap();
        assert!(target.exists());

        let read_back = fs::read(&target).unwrap();
        assert_eq!(read_back, data);

        // Ensure no leftover .tmp files
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(!name.ends_with(".tmp"), "Found leftover tmp file: {name}");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_snapshot_for_restore_success_and_rejections() {
        let dir = test_dir("val-snap");
        let storage_root = &dir;

        let content = "NR7-TEST-CONTENT\n한국어 복원 테스트\n🚀";
        let valid_bin = generate_synthetic_unsaved(content, false);
        let hash = compute_sha256(&valid_bin);
        let rel_blob = store_blob(storage_root, &hash, &valid_bin).unwrap();

        let valid_snapshot = NotepadSnapshot {
            id: "snap-1".to_string(),
            note_id: "note-1".to_string(),
            content_hash: hash.clone(),
            raw_snapshot_path: rel_blob.clone(),
            recovered_text: Some(content.to_string()),
            source_type: SourceType::TabState,
            source_filename: "tab1.bin".to_string(),
            source_size: valid_bin.len() as i64,
            parse_status: ParseStatus::Parsed,
            notepad_format_version: Some(1),
            captured_at: "2026-10-04T00:00:00Z".to_string(),
            created_at: "2026-10-04T00:00:00Z".to_string(),
        };

        // 1. Success case
        let validated = validate_snapshot_for_restore(storage_root, &valid_snapshot).unwrap();
        assert_eq!(validated, valid_bin);

        // 2. Reject non-parsed status
        let mut unparsed = valid_snapshot.clone();
        unparsed.parse_status = ParseStatus::Corrupt;
        let err = validate_snapshot_for_restore(storage_root, &unparsed).unwrap_err();
        assert!(err.to_string().contains("정상 파싱된"));

        // 3. Reject WindowState source
        let mut win_snap = valid_snapshot.clone();
        win_snap.source_type = SourceType::WindowState;
        let err = validate_snapshot_for_restore(storage_root, &win_snap).unwrap_err();
        assert!(err.to_string().contains("TabState 형식"));

        // 4. Reject hash mismatch
        let mut tampered_hash = valid_snapshot.clone();
        tampered_hash.content_hash = "deadbeef".to_string();
        let err = validate_snapshot_for_restore(storage_root, &tampered_hash).unwrap_err();
        assert!(err.to_string().contains("해시 무결성"));

        // 5. Reject corrupt CRC
        let corrupt_crc_bin = generate_synthetic_unsaved(content, true);
        let corrupt_hash = compute_sha256(&corrupt_crc_bin);
        let corrupt_blob = store_blob(storage_root, &corrupt_hash, &corrupt_crc_bin).unwrap();
        let mut corrupt_snap = valid_snapshot.clone();
        corrupt_snap.content_hash = corrupt_hash;
        corrupt_snap.raw_snapshot_path = corrupt_blob;
        let err = validate_snapshot_for_restore(storage_root, &corrupt_snap).unwrap_err();
        assert!(err.to_string().contains("CRC32"));

        // 6. Reject bad magic header
        let mut bad_magic_bin = valid_bin.clone();
        bad_magic_bin[0] = b'X';
        let bad_magic_hash = compute_sha256(&bad_magic_bin);
        let bad_magic_blob = store_blob(storage_root, &bad_magic_hash, &bad_magic_bin).unwrap();
        let mut bad_magic_snap = valid_snapshot.clone();
        bad_magic_snap.content_hash = bad_magic_hash;
        bad_magic_snap.raw_snapshot_path = bad_magic_blob;
        let err = validate_snapshot_for_restore(storage_root, &bad_magic_snap).unwrap_err();
        assert!(err.to_string().contains("매직 바이트") || err.to_string().contains("CRC32"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_safety_backup_and_rollback_flow() {
        let dir = test_dir("backup-flow");
        let storage_root = dir.join("storage");
        let notepad_local_state = dir.join("LocalState");

        let live_tab_dir = notepad_local_state.join("TabState");
        let live_win_dir = notepad_local_state.join("WindowState");
        fs::create_dir_all(&live_tab_dir).unwrap();
        fs::create_dir_all(&live_win_dir).unwrap();

        // Populate mock live state
        let existing_tab = live_tab_dir.join("existing-tab-1.bin");
        fs::write(&existing_tab, b"user important tab data").unwrap();
        let existing_win = live_win_dir.join("existing-win-1.0.bin");
        fs::write(&existing_win, b"user window session").unwrap();

        // 1. Create safety backup
        let (backup_dir, ts, backed_tabs, backed_wins) =
            create_safety_backup(&storage_root, &notepad_local_state).unwrap();

        assert!(backup_dir.exists());
        assert_eq!(backed_tabs.len(), 1);
        assert_eq!(backed_wins.len(), 1);
        assert!(backup_dir.join("TabState").join("existing-tab-1.bin").exists());
        assert!(backup_dir.join("WindowState").join("existing-win-1.0.bin").exists());

        // 2. Simulate restoring a new tab
        let restored_tab_path = live_tab_dir.join("restored-tab.bin");
        let restored_win_0 = live_win_dir.join("restored-win.0.bin");
        let restored_win_1 = live_win_dir.join("restored-win.1.bin");
        fs::write(&restored_tab_path, b"restored tab data").unwrap();
        fs::write(&restored_win_0, b"restored win 0").unwrap();
        fs::write(&restored_win_1, b"").unwrap();

        // Save last_restore.json
        let meta = SafetyBackupMetadata {
            timestamp: ts,
            backup_dir: backup_dir.to_string_lossy().to_string(),
            note_id: "test-note".to_string(),
            snapshot_id: "test-snap".to_string(),
            restored_tab_filename: "restored-tab.bin".to_string(),
            restored_tab_path: restored_tab_path.to_string_lossy().to_string(),
            restored_window_uuid: "restored-win".to_string(),
            restored_window_paths: vec![
                restored_win_0.to_string_lossy().to_string(),
                restored_win_1.to_string_lossy().to_string(),
            ],
            backed_up_tabs: backed_tabs,
            backed_up_windows: backed_wins,
        };
        let last_restore_file = storage_root.join("restore_backups").join("last_restore.json");
        fs::write(&last_restore_file, serde_json::to_string_pretty(&meta).unwrap()).unwrap();

        assert!(can_rollback(&storage_root));

        // 3. Rollback
        let report = rollback_last_restore_internal(&storage_root, &notepad_local_state, false).unwrap();
        assert!(report.success);
        assert_eq!(report.restored_note_id.as_deref(), Some("test-note"));

        // Verify restored files were deleted
        assert!(!restored_tab_path.exists());
        assert!(!restored_win_0.exists());
        assert!(!restored_win_1.exists());

        // Verify original files remain intact
        assert!(existing_tab.exists());
        assert!(existing_win.exists());
        assert_eq!(fs::read(&existing_tab).unwrap(), b"user important tab data");

        // Verify last_restore.json is cleaned up
        assert!(!can_rollback(&storage_root));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_notepad_running_error_message() {
        assert_eq!(NOTEPAD_RUNNING_ERROR, "메모장 탭 복원을 위해 메모장을 먼저 닫아주세요.");
    }
}
