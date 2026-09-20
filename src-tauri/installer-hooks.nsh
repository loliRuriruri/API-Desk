; ============================================================
;  API Desk 설치 프로그램 - 한글 소개/안내 문구 (NSIS_HOOK text overrides)
;  이 파일의 !define 들은 installer.nsi 가 각 페이지를 만들기 전에 적용됩니다.
; ============================================================

!define MUI_ABORTWARNING

!define MUI_WELCOMEPAGE_TITLE "API Desk 설치 마법사"
!define MUI_WELCOMEPAGE_TEXT "이 마법사가 컴퓨터에 API Desk를 설치합니다.$\r$\n$\r$\nAPI Desk는 API 키와 계정 쿼터·사용량을 한곳에서 관리하는 로컬 우선 데스크톱 앱입니다.$\r$\n$\r$\n  -  API 키는 Stronghold 볼트에 암호화해 저장하고, 필요할 때만 화면에 표시합니다.$\r$\n  -  모니터: Codex · Grok Bot/Build · Antigravity · OpenCode Go · OpenRouter · DeepSeek · Tavily$\r$\n  -  트레이 상주, 미니 창, 임계치 알림, 프로젝트 .env 반영, 모델/프로젝트 정리$\r$\n  -  모든 데이터는 내 PC에만 저장됩니다 (외부 서버 전송 없음).$\r$\n$\r$\n설치를 계속하려면 [다음]을 클릭하세요.$\r$\n설치 중 파일이 잠기지 않도록 API Desk를 종료해 주세요."

!define MUI_FINISHPAGE_TITLE "API Desk 설치가 완료되었습니다"
!define MUI_FINISHPAGE_TEXT "API Desk가 설치되었습니다.$\r$\n$\r$\n  -  처음 실행하면 마스터 비밀번호로 볼트를 만들어 API 키를 저장합니다.$\r$\n  -  트레이 아이콘 왼쪽 클릭 = 미니 창, 우클릭 = 열기 · 사용량 새로고침 · 종료$\r$\n  -  기존 API 키와 설정은 재설치해도 그대로 유지됩니다.$\r$\n$\r$\n아래 'API Desk 실행'을 체크하면 설치 직후 바로 시작합니다."
!define MUI_FINISHPAGE_RUN_TEXT "API Desk 실행"
