# R1 실행. 반드시 **실제 터미널**에서 돌려야 한다.
#
# ConPTY 는 콘솔이 붙어 있어야 동작한다. 파이프로 캡처하거나 Start-Job 으로
# 돌리면 자식 출력이 오지 않는다(실측).
#
#   .\run.ps1              claude 를 PTY 로 띄운다
#   .\run.ps1 codex        다른 에이전트
#   .\run.ps1 -SelfTest    PTY+VT 파이프라인만 자동 점검
param([string]$Agent = '', [switch]$SelfTest, [switch]$Solo)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

if (-not (Test-Path 'target\debug\multi_ai_cli.exe')) {
    & "$PSScriptRoot\build.ps1"
}

if ($SelfTest) {
    & '.\target\debug\multi_ai_cli.exe' --selftest
} else {
    Write-Host "Ctrl+] 로 빠져나옵니다."
    & '.\target\debug\multi_ai_cli.exe' $Agent
}
