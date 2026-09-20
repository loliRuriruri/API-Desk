$ErrorActionPreference = "SilentlyContinue"

Write-Host "API Desk WebView2 복구 스크립트"
Write-Host "1) 실행 중인 API Desk 종료"

$app = Get-Process api-desk
if ($app) {
  $app.CloseMainWindow() | Out-Null
  Start-Sleep -Seconds 2
  Get-Process api-desk | Stop-Process -Force
}

Start-Sleep -Seconds 2

Write-Host "2) 남아 있는 WebView2 프로세스 정리"
Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" |
  Where-Object { $_.CommandLine -like "*com.apidesk.desktop*" } |
  ForEach-Object { Stop-Process -Id $_.ProcessId -Force }

Start-Sleep -Seconds 2

Write-Host "3) WebView2 사용자 데이터 초기화 (백업 후)"
$dir = "$env:LOCALAPPDATA\com.apidesk.desktop"
if (Test-Path -LiteralPath "$dir\EBWebView") {
  $backup = "EBWebView.bak-" + (Get-Date -Format "yyyyMMdd-HHmmss")
  Rename-Item -LiteralPath "$dir\EBWebView" -NewName $backup
  Write-Host "   $dir\EBWebView -> $backup"
} else {
  Write-Host "   EBWebView 폴더가 없습니다 (이미 깨끗한 상태)"
}

Write-Host "4) 완료. API Desk를 다시 실행하세요."
Write-Host "   (앱을 종료할 때는 트레이 아이콘 우클릭 > 종료 를 사용하세요)"
