# Notepad Rescue Integration — Implementation Plan

## 1. Architectural Baseline (Phase NR.0 Findings)

### 1.1 Navigation & Layout
- **Current Sidebar (`src/components/Sidebar.tsx`)**: Flat array of `NAV_ITEMS` with keys: `dashboard`, `apis`, `models`, `services`, `projects`, `settings`, `import`.
- **Target Navigation**: Organize a dedicated **Tools** group in the sidebar:
  ```text
  Tools
   ├─ Local Services (key: "services")
   └─ Notepad Rescue (key: "notepad-rescue")
  ```
- **Routing (`src/App.tsx`)**: Extend `PageKey` to include `"notepad-rescue"`, rendering `<NotepadRescuePage />`.

### 1.2 Database & Migration Infrastructure
- **Database**: Single SQLite database `sqlite:apidb.db` located at `%APPDATA%\com.apidesk.desktop\apidb.db`.
- **Migration System**: Versioned migrations via `src-tauri/src/migrations.rs` (`tauri_plugin_sql`).
- **Migration 5 (`NOTEPAD_RESCUE`)**:
  - `notepad_notes`: Stores active and deleted notes (logical tab/document entity).
  - `notepad_snapshots`: Stores versioned snapshots with SHA-256 hash, parse status, and recovered text.
  - `notepad_sessions`: Stores window/tab ordering sessions.
  - `notepad_rescue_settings`: Configuration key-value store.

### 1.3 Rust Backend Architecture
- **State Management**: `NotepadRescueState` registered with Tauri `manage(...)`.
- **Watch Target**:
  - Primary: `%LocalAppData%\Packages\Microsoft.WindowsNotepad_8wekyb3d8bbwe\LocalState\TabState`
  - Secondary: `%LocalAppData%\Packages\Microsoft.WindowsNotepad_8wekyb3d8bbwe\LocalState\WindowState`
- **File Sharing Rules**: Read files using shared read/write (`FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`) to prevent sharing violation conflicts while Notepad is writing.
- **Safety Policy**:
  - Strictly **READ + COPY** only. No writes, updates, deletes, or process kills directed at Notepad.
  - Deleting a tab in Notepad marks the note as `is_deleted = 1` in API Desk; the last snapshot and historical versions are **never deleted**.

### 1.4 Binary Parser Specification
- **Magic Bytes**: `4E 50 00` (`NP\0`).
- **Format Structure**:
  - Flag byte at index 3: `0x00` = unsaved tab, `0x01` = saved tab.
  - Unsaved Tab: `selection_start` (uLEB128), `selection_end` (uLEB128), `config_block`, `content_length` (uLEB128 chars), `content` (UTF-16LE), `contain_unsaved_data` (bool), `checksum` (CRC32), `unsaved_chunks`.
  - Saved Tab: `path_length` (uLEB128), `file_path` (UTF-16LE), `file_size` (uLEB128), `encoding`, `cr_type`, `last_write_time`, `sha256_hash`, `selection_start`, `selection_end`, `config_block`, `content_length`, `content`, `contain_unsaved_data`, `checksum`, `unsaved_chunks`.
- **Fallback**: If parsing fails or encounters an unknown version, the raw binary snapshot is stored unconditionally and flagged as `UNSUPPORTED_FORMAT` or `PARSE_ERROR`.

### 1.5 Storage Convention
- Content-addressed blob store in `%LocalAppData%\com.apidesk.desktop\notepad_rescue\snapshots\<sha256>.bin`.
- Ensures zero duplicate storage across identical snapshots.

---

## 2. File Organization

### 2.1 Backend (`src-tauri/src/notepad_rescue/`)
- `mod.rs`: Module definition, state, Tauri commands, initialization.
- `models.rs`: Data transfer objects and internal structs.
- `parser.rs`: Binary parser for TabState and WindowState formats, uLEB128 decoding, UTF-16 extraction.
- `repository.rs`: SQLite queries for notes, snapshots, search, and settings.
- `snapshot.rs`: Content-addressed file copy and metadata capture.
- `watcher.rs`: Filesystem watcher with debouncer for `TabState` and `WindowState`.
- `recovery.rs`: TXT export and launching recovered files in Notepad.

### 2.2 Frontend (`src/`)
- `src/lib/notepadRescue.ts`: API client functions, event listeners, TypeScript interfaces.
- `src/components/notepad-rescue/RescueStatus.tsx`: Header banner showing active status, last snapshot time, storage stats.
- `src/components/notepad-rescue/NoteList.tsx`: Tabbed list (`Today`, `Previous`, `Deleted`) with search filter.
- `src/components/notepad-rescue/NotePreview.tsx`: Side-by-side or modal preview of note content with line count and metadata.
- `src/components/notepad-rescue/VersionHistory.tsx`: Timeline of snapshots for the selected note.
- `src/components/notepad-rescue/RescueSettings.tsx`: Settings panel for retention, storage limit, and toggle.
- `src/pages/NotepadRescuePage.tsx`: Main page assembling the components.

---

## 3. Implementation Phases

### Phase NR.1: Database Migration & Core Domain Models
1. Add Migration 5 to `src-tauri/src/migrations.rs`.
2. Define models in `src-tauri/src/notepad_rescue/models.rs`.
3. Implement `src-tauri/src/notepad_rescue/repository.rs` for database persistence and querying.

### Phase NR.2: Binary Parser Engine
1. Implement uLEB128 reader, UTF-16 decoder, CRC32 verifier in `parser.rs`.
2. Parse unsaved and saved TabState structures.
3. Parse WindowState tab session references.
4. Unit tests with synthetic TabState binaries (both valid and corrupted).

### Phase NR.3: Snapshot Engine & Filesystem Watcher
1. Implement content-addressed storage in `snapshot.rs` with SHA-256 deduplication.
2. Implement debounced filesystem watcher in `watcher.rs` using `notify`.
3. Handle create, modify, rename, and remove events safely.
4. Ensure watcher failures do not panic or crash the application.

### Phase NR.4: Tauri Commands, State & Events
1. Implement `NotepadRescueState` and Tauri commands:
   - `notepad_rescue_status`
   - `notepad_rescue_list_notes`
   - `notepad_rescue_get_note`
   - `notepad_rescue_list_snapshots`
   - `notepad_rescue_get_snapshot_content`
   - `notepad_rescue_search`
   - `notepad_rescue_export_txt`
   - `notepad_rescue_open_in_notepad`
   - `notepad_rescue_get_settings`
   - `notepad_rescue_save_settings`
   - `notepad_rescue_force_scan`
2. Register commands and background watcher in `src-tauri/src/lib.rs`.

### Phase NR.5: Frontend Service Client & Navigation
1. Create `src/lib/notepadRescue.ts` wrapping Tauri invokes and events.
2. Update `src/components/Sidebar.tsx` to add `Tools` group navigation.
3. Update `src/App.tsx` routing.

### Phase NR.6: Notepad Rescue UI Components
1. `RescueStatus.tsx`: Active/Disabled/Error badge, snapshot counts, disk usage.
2. `NoteList.tsx`: Today / Previous / Deleted tabs with search input.
3. `NotePreview.tsx`: Previewing recovered text with copy button.
4. `VersionHistory.tsx`: Version diff/timeline selector.
5. `RescueSettings.tsx`: Configurable retention and limits.
6. Assemble into `src/pages/NotepadRescuePage.tsx`.

### Phase NR.7: Recovery Flows & Text Export
1. Direct clipboard copy.
2. TXT export dialog via `tauri-plugin-dialog` or safe file save.
3. Safe invocation of Notepad (`powershell/cmd` hidden helper, opening a temp recovered copy without touching the original TabState).

### Phase NR.8: Full Verification & Acceptance Testing
1. Unit tests: parser, repository, snapshot deduplication.
2. Frontend tests: Vitest test suite for Notepad Rescue components and client.
3. Real smoke test:
   - Create new tab in Notepad, type text, verify automatic snapshot.
   - Edit text, verify version history.
   - Close tab, verify appearance in Deleted list.
   - Export to TXT and verify content.
   - Restart API Desk and verify persistence.
4. Regression suite: `cargo test`, `npm test`, `npx tsc --noEmit`, `npx eslint .`, `cargo build --release`.
