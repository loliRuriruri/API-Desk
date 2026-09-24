# Local AI Service Manager — 구현 검증 보고서

> 대상: API Desk (Tauri 2 + React 19 + TypeScript, Windows 전용)
> 범위: "Local AI Service Manager" 신규 기능 (첫 지원 서비스: Laya)
> 작성 목적: 제3자(GPT) 검증용. 아래 주장에는 근거(파일:줄, 명령, 출력)를 함께 적었다.

---

## 1. 요약

- 사이드바에 **로컬 서비스** 화면을 추가하고, Laya 서비스를 GUI에서 **Start / Stop / Restart / Health Check** 할 수 있게 했다.
- 서비스 정의는 백엔드(`src-tauri/src/local_services.rs`)에 **선언형**으로 등록한다. 현재 Laya 1종이 등록되어 있고, Ollama·FishAudio·MCP 서버 등은 정의 추가만으로 확장 가능하다.
- `LAYA_API_KEY`는 **선택 사항**이다. Vault에 키가 있으면 시작 시 **자식 프로세스 환경변수로만** 주입해 Bearer 인증으로 동작하고, 키가 없으면 주입 없이 **localhost 전용 무인증 모드**로 실행한다(시작 차단 없음). 화면에는 `설정됨 (••••abcd) · Bearer 인증` 또는 `미설정 — localhost 전용 무인증`으로 표시하며, 로그 수집 단계에서 키 문자열을 자동 마스킹한다.
- API Desk를 재시작해도 **포트/PID를 다시 조회**해 기존 Laya 프로세스를 그대로 감지한다(외부 프로세스로 표시).

---

## 2. 스펙 대응표 (요구 1~16)

| # | 요구 | 구현 | 근거 |
| --- | --- | --- | --- |
| 1 | 사이드바 `로컬 서비스` 메뉴 | 추가됨 | `src/components/Sidebar.tsx:7`(PageKey), `:16`(NAV_ITEMS), `src/App.tsx:60`(PAGE_TITLES), `src/App.tsx:381`(라우트) |
| 2 | 카드 정보(상태/Endpoint/Port/Device/PID/uptime/키 설정 여부/최근 로그) | 표시됨 | `src/pages/LocalServicesPage.tsx` `.service-info` 블록, `src-tauri/src/local_services.rs` `LocalServiceStatus` |
| 3 | Start/Stop/Restart/Health Check | 제공 | 페이지 헤더 버튼 4종, 커맨드 `local_service_start|stop|restart|health` |
| 4 | 키 평문 미저장 | 소스/스크립트에 키 없음 | 키는 런타임에 Vault에서 읽음(`local_services.rs:641`) |
| 5 | Vault 저장 + 자식 env 주입 (키가 있을 때만) | 구현 | `secret_id = local:laya:api_key`(`:73`), `write_secret`(`:842`), `service_env`(`:369~`) → `command.env(...)`. 키 미설정 시 주입하지 않고 localhost 무인증 모드로 실행(스펙 변경 반영) |
| 6 | UI에 전체 키 비노출 | 마지막 4자리 + 인증 모드 표기 | `api_key_hint`(`:393`), `apiKeyLabel()`: `설정됨 (••••abcd) · Bearer 인증` / `미설정 — localhost 전용 무인증` |
| 7 | 실행 환경(USE_TF=0, LAYA_DEVICE=cuda, LAYA_PRELOAD=1, LAYA_API_KEY 선택) | 구현 | `local_services.rs:70-72`, 테스트 `builds_laya_environment`(키 있음) / `omits_api_key_when_absent`(키 없음) |
| 8 | 실행 파일/venv 경로 설정 | 설정 패널 | 페이지 `service-settings` (실행 파일·인자·작업폴더) |
| 9 | 기본 endpoint `http://127.0.0.1:8000` | 기본값 | `definitions()` `default_endpoint`, 테스트 `merges_defaults_into_config` |
| 10 | 중복 실행 방지 | 구현 | 관리 PID 생존 확인(`:620~`), 포트 LISTEN 확인(`:632`), 메시지 "이미 API Desk가 실행 중…" / "포트 N이 이미 사용 중입니다 (PID …)" |
| 11 | 앱 시작 시 기존 프로세스 감지 | 구현 | `port_owner_pid`(netstat 파싱, `:330~`), `process_start_unix`(Win32 `GetProcessTimes`, `:274`), 상태 계산 `service_state_from` |
| 12 | 앱 종료 시에도 유지 옵션 | 구현 | 설정 `keepAliveOnExit`(기본 true), `shutdown()`(`:900~`) — 끈 서비스만 `taskkill /T /F` |
| 13 | 앱 시작 시 자동 실행 옵션 | 구현 | 설정 `autoStart`, 커맨드 `local_service_autostart`, `src/App.tsx:224`(Vault 잠금 해제 후 1회 호출) |
| 14 | Windows 로그인 시 실행 인터페이스 분리 | 인터페이스만 | `LoginStartupMethod` enum + `local_service_login_startup_plan` 커맨드(`:921`), UI에 "추후 지원 예정" 안내 |
| 15 | 범용 추상화(UI에 Laya 하드코딩 금지) | 충족 | 서비스 정의/환경/비밀 키 이름이 모두 Rust 정의에서 내려옴. UI는 정의 목록을 그대로 렌더링 |
| 16 | 향후 서비스 등록 가능 | 충족 | `definitions()` 벡터에 항목 추가로 확장 (id/label/기본포트/정적 env/비밀 env/헬스경로) |

---

## 3. 아키텍처

```
React (LocalServicesPage)
   │ invoke
   ▼
Tauri commands (local_services.rs)
   ├─ 정의/설정:  local_service_definitions / configs / save_config
   ├─ 상태:      local_service_status (관리 프로세스 + netstat 포트 소유자 조회)
   ├─ 제어:      local_service_start / stop / restart
   ├─ 진단:      local_service_health (HTTP GET {endpoint}{healthPath}) / logs
   ├─ 비밀:      local_service_set_api_key / clear_api_key  → Vault(Stronghold)
   └─ 옵션:      local_service_autostart / login_startup_plan(스텁)
```

- 설정 영속화: `%APPDATA%\com.apidesk.desktop\local-services.json`
- 상태 저장(휘발): `LocalServicesState { processes: HashMap<String, ManagedProcess>, logs: VecDeque<String> }`
- 로그: 자식 stdout/stderr를 스레드로 읽어 최대 400줄 보관, 저장 전 `scrub_secrets()` 적용
- 종료 경로: `quit_app()` → `local_services::shutdown(&handle)` → 유지 옵션이 꺼진 서비스만 정리

---

## 4. 보안 검증 포인트

1. **평문 저장 없음**: 저장소/스크립트 어디에도 키 문자열이 없다. 키는 Vault(`local:laya:api_key`)에만 존재.
2. **주입 경로 단일**: `Command::env(def.secret_env, secret)` — 자식 프로세스 환경변수로만 전달. UI(JS)로는 값이 전달되지 않는다(상태에는 힌트만).
3. **마스킹**: 로그 라인에 키가 포함되면 `***`로 치환(`scrub_secrets`, 테스트 존재).
4. **오류 메시지**: 시작 실패 시 키가 포함된 문자열을 만들지 않는다(설정 경로/포트/PID만 노출).
5. **잠금 상태**: Vault가 잠겨 있으면 키 저장/삭제 버튼은 비활성화되지만 Start는 가능하다(키 미주입 무인증). 이때 UI는 Vault 잠금 — 무인증 모드로 실행으로 표시한다.
6. **헬스체크 인증**: 키가 있으면 Health Check 요청에도 Authorization: Bearer <키>를 붙인다(키는 JS로 전달되지 않음).

---

## 5. 검증 증거 (재현 가능)

### 5.1. 자동 테스트

| 항목 | 명령 | 결과 |
| --- | --- | --- |
| Rust | `cd src-tauri && cargo test` | `test result: ok. 56 passed; 0 failed` |
| 로컬 서비스 전용 | `cargo test local_services::` | 7개: `detects_listening_port_owner`(실제 TcpListener + netstat 실측), `parses_port_owner_from_netstat`, `scrubs_secret_values_from_logs`, `builds_laya_environment`, `masks_api_key_hint`, `merges_defaults_into_config`, `derives_service_state` |
| 프런트 | `npm test` | `Tests 59 passed` (`localServices.test.ts`: `formatUptime`, `serviceStateLabel/Tone`, `apiKeyLabel`) |
| 정적 검사 | `npm run lint` / `npm run typecheck` | `0 problems` / `tsc --noEmit` 통과 |

### 5.2. 빌드·설치

- `npm run tauri build` → `Finished 2 bundles` (MSI + NSIS)
- 무음 설치: `API Desk_0.1.0_x64-setup.exe /S` → exit 0
- 설치본 `api-desk.exe` 타임스탬프 14:11:22, 실행 프로세스 14:12:08 시작(Responding=True)
- 실행 파일 문자열 확인: `local_service_start`, `local_service_status`, `local_service_login_startup_plan`, `Laya`, `로컬 서비스`, `local-services.json`

### 5.3. UI 확인 (사용자 스크린샷)

- 사이드바에 `로컬 서비스` 표시, Laya 카드에 상태 배지(중지됨)·Endpoint·Port·Device·PID·가동시간·키 설정 여부(미설정)·헬스체크 결과(실패 · HTTP 0 · 2022ms — 포트 미사용 시 정상 동작) 표시
- Start 활성, Stop/Restart 비활성(미실행/경로 미설정 시) — 의도된 동작

---

## 6. 변경 파일

```
M README.md
M src-tauri/src/lib.rs                  (모듈 등록, 상태 manage, 커맨드 12종 등록, quit_app에서 shutdown)
M src/App.tsx                           (라우트/타이틀, 잠금 해제 후 자동시작 1회)
M src/components/Sidebar.tsx            (PageKey + 메뉴)
M src/styles.css                        (서비스 카드/설정/로그 스타일)
A src-tauri/src/local_services.rs       (백엔드 전체)
A src/lib/localServices.ts              (타입 + invoke 래퍼 + 순수 헬퍼)
A src/lib/__tests__/localServices.test.ts
A src/pages/LocalServicesPage.tsx
```

---

## 7. 미검증 / 잔여 항목 (GPT 검증 시 확인 요청)

1. **실제 Laya 종단 시나리오**: 실제 Laya 실행 파일 + Vault 비밀번호가 필요해 자동 검증하지 못했다. 사용자 수동 절차:
   - 잠금 해제 → 로컬 서비스 → 설정 → 실행 파일 경로/인자 저장 → `LAYA_API_KEY` 저장 → Start → 상태/PID/로그 확인 → Stop/Restart/Health Check 확인
2. **로그인 시 자동 실행**: 인터페이스(`LoginStartupMethod`, `local_service_login_startup_plan`)만 존재하며 실제 등록은 미구현(요구사항 14의 "추후 확장" 조건).
3. **외부 프로세스 Stop**: 관리 대상이 아닌 프로세스도 포트 소유 PID를 찾아 `taskkill /T /F`로 종료한다(의도된 동작이나, 사용자 확인 권장).
4. **회귀**: 기존 테스트 전부 통과(모니터/사용량/설정/보안 등), 기존 기능 코드는 수정 없이 추가만 했다.

---

## 8. 검증 체크리스트 (복사해서 사용)

- [ ] `cargo test` 56 passed 재현
- [ ] `npm test` 59 passed 재현, `npm run lint` 0 problems
- [ ] `src-tauri/src/local_services.rs`에서 키가 소스에 하드코딩되지 않았는지 확인
- [ ] `service_env()`가 `USE_TF=0`, `LAYA_PRELOAD=1`, `LAYA_DEVICE`, `LAYA_API_KEY`를 만드는지 확인
- [ ] `start_service()`가 포트/관리 PID 중복 시 spawn하지 않는지 확인
- [ ] `build_status()`가 외부 프로세스를 PID/가동시간과 함께 감지하는지 확인
- [ ] UI에서 키가 마지막 4자리만 표시되는지(`api_key_hint`) 확인
- [ ] 키 미설정 상태에서 Start가 차단되지 않고 localhost 무인증으로 실행되는지 확인
- [ ] `shutdown()`이 `keepAliveOnExit=false`인 서비스만 정리하는지 확인

---

## 9. 추가 기능: 미니 창 요약 컨트롤 (스펙 추가분)

| 요구 | 구현 | 근거 |
| --- | --- | --- |
| 미니 창 상단에 로컬 AI 서비스 섹션(계정 쿼터 위) | 구현 | src/MiniApp.tsx — serviceRows 섹션(계정 쿼터 앞) |
| Laya 카드: 상태/Device/Port/PID/uptime/인증 상태만 | 구현 | 동일 섹션, miniAuthLabel()(src/lib/localServices.ts:124) |
| 버튼: Start/Stop/Restart/Health/상세 | 구현 | mini-service-actions 5버튼, 상세 → openMainPage("services") |
| 백엔드 재사용(미니 전용 프로세스 로직 없음) | 충족 | 미니는 기존 커맨드(local_service_start|stop|restart|health|status)만 호출 |
| 이벤트 local-service-status-changed emit/subscribe | 구현 | 백엔드 emit_status_changed()(start/stop/restart/save_config/set·clear key/프로세스 종료 감지 시), 미니·메인 구독(onLocalServiceStatusChanged) |
| 이벤트 없을 때 폴링 대체(교체 가능하게 분리) | 충족 | 미니 5초, 메인 3초 폴링 병행(별도 effect) |
| 키/경로/작업폴더 미노출 | 충족 | 미니 카드에는 인증 상태 라벨만, 설정 패널 없음 |
| Vault 잠금 + 키 필요 시 Start 차단("Vault 잠금 해제 필요") | 구현 | start_block_reason()(local_services.rs:410) + 프런트 startBlockReason()(localServices.ts:131) — 단위 테스트 양쪽 존재 |
| 무인증 모드면 잠금 상태에서도 Start 가능 | 구현 | 키 미설정 시 게이트 통과(테스트 locks_start_only_when_configured_key_unavailable) |

### 9.1. 스모크 테스트 (실측)

Laya 대체 스텁(포트 8000 리스너 + 환경변수 덤프)으로 자동 실행 경로를 검증했다.

| 단계 | 결과 |
| --- | --- |
| 설정: executable=powershell(스텁), autoStart=true, keepAliveOnExit=false, apiKeyConfigured=false | %APPDATA%\com.apidesk.desktop\local-services.json |
| Vault 잠금 상태로 API Desk 실행 | 자동 실행 성공(무인증 허용 규칙 확인) |
| 자식 프로세스 환경변수 덤프 | USE_TF=0, LAYA_PRELOAD=1, LAYA_DEVICE=cuda, LAYA_API_KEY=[](미주입) |
| 포트 | 127.0.0.1:8000 LISTEN 확인 |
| 미니 창 | Laya [실행 중] [관리] / Device cuda · Port 8000 · PID 58708 · 17s / 인증 무인증 (localhost) + Start/Stop/Restart/Health/상세 렌더 확인 |
| API Desk 종료(keepAliveOnExit=false) | 자식 프로세스 종료 + 포트 8000 해제 확인 |