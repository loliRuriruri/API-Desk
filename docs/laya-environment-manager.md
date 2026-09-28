# Laya 환경 매니저 (GPU 환경 구성 · 안전 업데이트)

API Desk가 Laya 로컬 서비스의 Python 환경을 진단·구성·활성화·롤백까지 관리한다.
사용자는 PowerShell/pip 명령을 직접 입력하지 않아도 된다.

## 환경 수명주기

```
CURRENT (production)          CANDIDATE (검증 대기)             ACTIVE
C:\Laya\.venv                 C:\Laya\envs\laya-<ver>-gpu       (설정이 가리키는 환경)
  Laya 0.3.18 / CPU      →      Laya 0.3.21 / CUDA / TileLang  →  검증 통과 후에만 전환
```

- 기존 production 환경은 **절대 덮어쓰지 않는다**. 새 버전은 항상 새 디렉터리에 만든다.
- 후보 환경은 `C:\Laya\envs\laya-<version>-gpu` (버전별) 이며 `laya-env.json`에 상태를 기록한다.
  - `incomplete | ready | active` — `ready`가 아니면 활성화할 수 없다(중단된 후보는 다음 실행에서 무시).
- 활성화 전까지 서비스 설정은 그대로이며, 실패 시 자동으로 이전 설정으로 되돌린다.

## 버전 확인

- 소스: **PyPI stable** (`https://pypi.org/pypi/laya/json`, pre-release 제외). HTML/GitHub 스크래핑 없음.
- 캐시: 마지막 확인 시각 기준 **24시간**. `[업데이트 확인]`은 강제 새로 확인.
- 네트워크 실패 시 "최신 확인 불가"로 표시하고 설치 버전을 구버전으로 단정하지 않는다.
- 앱 시작 시 pip를 실행하지 않는다(가벼운 캐시 조회만).

## 진단(DTO)

`laya_env_status`가 선택된 Python 한 번으로 구조화 JSON을 얻는다(셸 미사용).

| 필드 | 의미 |
| --- | --- |
| pythonPath / pythonVersion | 실행 중인 인터프리터 |
| layaVersion / latestLayaVersion / updateAvailable | 설치/최신 stable |
| torchVersion / torchCudaVersion | PyTorch 빌드 |
| cudaAvailable / gpuName / computeCapability | `torch.cuda` 기준 실제 GPU |
| tilelangInstalled / tilelangVersion / fastPathSupported | fast path 요건 |
| fastPathActive / effectiveDevice / backend | **실제** 서빙 상태(/health) |
| serviceRunning / servicePid / health | 로컬 서비스 상태 |
| problems[] | 사용자 안내(예: "PyTorch가 CPU 전용 빌드입니다") |

시크릿(환경변수·API 키·토큰·명령줄 원문)은 절대 DTO에 포함하지 않는다.

## GPU 후보 생성 파이프라인

`[GPU 환경 구성/복구]` → (백그라운드, 진행 이벤트 `laya-env-progress`)

1. base Python(`pyvenv.cfg`의 home) 확인 → `python -m venv <후보>`
2. pip/wheel 업그레이드
3. PyTorch CUDA 설치 — 인덱스 우선순위 **cu130 → cu128 → cu126**(호환성 레이어, 하드코딩 아님)
4. CUDA 검증(`torch.cuda.is_available()`, 실패 시 해당 인덱스 폐기)
5. `laya[serve,fast]==<stable>` 설치
6. 진단(DTO)
7. 실제 체크포인트 로드 + 추론 스모크(english, multilingual, typed-decisions)
8. HTTP 서버 테스트 — 래퍼를 임시 포트에 띄워 `/health.ready` + 추론 1건 검증

모든 단계는 `Command::new(executable).args([...])`로 실행한다(셸 문자열 미사용, 공백 경로 안전).
실패하면 후보는 `incomplete`로 남고 active 환경은 그대로다. 로그/진행 메시지는 시크릿을 마스킹한다.

## Fast Path(TileLang)

- `laya[fast]`(tilelang) 설치 + CUDA가 모두 있어야 fast path가 활성화된다.
- Laya Agent는 `fast=True`에서 `accelerate()`가 실패하면 **stock 경로로 자연 강등**된다(예외 없음).
- Windows에서 nvcc가 최신 MSVC를 거부하는 경우가 있어 `NVCC_APPEND_FLAGS=-allow-unsupported-compiler`를
  서비스 환경·관리 스크립트에 주입한다(이 PC: nvcc 12.8 + MSVC 14.50).
- TileLang 설치 = fast path 아님. 반드시 실제 추론까지 검증한다(후보 게이트 7·8단계).

## 런타임 정책(폴백 순서)

```
CUDA_FAST (tilelang)  →  CUDA_STOCK (torch)  →  CPU_FALLBACK
```

`/health`가 **실제** 상태를 보고한다(요청값을 그대로 되돌려주지 않음).

```json
{
  "status": "ok", "ready": true, "laya_version": "0.3.21",
  "device_requested": "cuda", "device_effective": "cuda",
  "gpu": "NVIDIA GeForce RTX 5090",
  "backend": "tilelang", "fast_path": true,
  "loaded": ["english", "multilingual", "typed-decisions"],
  "cpu_fallback_count": 0, "fallback_reason": null,
  "warmup": { "totalMs": 664, "models": 3 }
}
```

- `ready`는 **warm-up 완료 후**에만 true가 된다. warm-up은 짧은 질문 1건을 각 모델에 실행한다.
- warm-up 실패는 서비스를 죽이지 않고 `fallback_reason`에 남긴다.
- API Desk는 서비스 설정의 executable=`<후보>\Scripts\python.exe`, args=`[envs\laya_health.py]`로 실행한다.

## 활성화 · 롤백

`[검증된 후보 활성화]`:

1. 현재 Laya 중지 + 포트 해제 대기
2. 서비스 설정 교체(executable/args/runtimeRoot/pythonExecutable/environmentKind/environmentVersion)
3. 후보 시작 → `/health.ready` 대기 → **Vault의 LAYA_API_KEY로 추론 검증**(401 방지)
4. 성공 시 후보 상태를 `active`로, 이전 환경을 `previous`로 기록
5. 실패 시 이전 설정 복원 + 이전 Laya 재시작 + `/health` 확인 → "업데이트 실패, 기존 환경으로 자동 복구했습니다"

`[이전 버전으로 되돌리기]`는 `previous`가 있을 때만 노출된다. 최초 마이그레이션에서는
기존 CPU 환경(`C:\Laya\.venv`)이 그대로 보존되며, 이 버튼은 비활성이다(수동 복구 가능).

## 업데이트

`[GPU 환경 구성/복구]`는 항상 **stable 최신 버전**을 해석해 새 후보를 만든다.
현재 == 최신이면 그대로 재검증(복구)만 수행한다. in-place `pip install -U`는 하지 않는다.

## 설정 스키마(하위 호환)

`local-services.json`에 다음이 추가된다(없으면 빈 값으로 로드):

```json
{
  "id": "laya",
  "executable": "C:\\Laya\\envs\\laya-0.3.21-gpu\\Scripts\\python.exe",
  "args": ["C:\\Laya\\envs\\laya_health.py"],
  "runtimeRoot": "C:\\Laya\\envs\\laya-0.3.21-gpu",
  "pythonExecutable": "C:\\Laya\\envs\\laya-0.3.21-gpu\\Scripts\\python.exe",
  "environmentKind": "gpu",
  "environmentVersion": "0.3.21"
}
```

주입 환경변수: `USE_TF=0`, `LAYA_PRELOAD=1`, `LAYA_DEVICE=cuda`,
`NVCC_APPEND_FLAGS=-allow-unsupported-compiler`, `LAYA_API_KEY=<Vault 값>`.

## GPU 모니터/TURZX 통합

별도 수집기를 만들지 않는다. API Desk GPU 프로세스 귀속(managed PID + descendant + 포트 + /health)을
그대로 사용해 Laya를 "Laya · english · multilingual · typed-decisions · <VRAM>" 행으로 표시하고,
TURZX도 같은 스냅샷을 소비한다. `/health`가 CPU면 GPU 상주로 표기하지 않는다.

## 문제 해결

| 증상 | 확인 |
| --- | --- |
| CUDA를 사용할 수 없음 | 후보의 `torch.version.cuda`, GPU 드라이버, cu130/cu128 인덱스 |
| TileLang 컴파일 실패 | `NVCC_APPEND_FLAGS=-allow-unsupported-compiler` 주입 여부, nvcc/MSVC 조합 |
| 활성화 후 401 | Vault의 `LAYA_API_KEY`와 검증 스크립트의 키 일치(자동 처리) |
| 포트 점유로 활성화 실패 | 외부에서 띄운 laya-serve 프로세스 정리(관리 대상 외 프로세스) |
| 후보가 목록에 안 보임 | `laya-env.json`의 state가 `incomplete`인지 확인(중단된 후보는 의도적으로 비활성화) |

## 이 PC 실측(2026-09-28)

- 현재(롤백용): `C:\Laya\.venv` — Laya 0.3.18, torch 2.14.0+cpu, device CPU
- 활성: `C:\Laya\envs\laya-0.3.21-gpu` — Laya 0.3.21, torch 2.14.0+cu130, CUDA 13.0,
  RTX 5090 (cc 12.0), TileLang 0.1.14, backend tilelang, fast_path true, cpu_fallback_count 0
- 벤치(english, 5 iter): CPU 웜 p50 70ms → GPU fast 웜 p50 4.3ms, VRAM 7.44GB(3 모델, PDH)
