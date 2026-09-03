# multi_ai_cli 실행.
# 사용법:  .\scripts\run.ps1 [-Workspace <path>] [-Room <id>]
#
# 콘솔 코드페이지와 Java 출력 인코딩을 UTF-8 로 맞춘다.
# 스트림 디코딩(UTF-8)과 콘솔 출력 코드페이지는 별개다 — 이걸 안 맞추면
# 한글과 발화자 블록이 CP949 로 깨진다 (SPEC §6.3, 교차검토 R11).

param(
    [string]$Workspace = (Get-Location).Path,
    [string]$Room = ''
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$out  = Join-Path $root 'out'

if (-not (Test-Path (Join-Path $out 'io\multiai\cli\Main.class'))) {
    Write-Host "빌드 산출물이 없다. 먼저 컴파일한다."
    & (Join-Path $PSScriptRoot 'compile.ps1')
}

chcp 65001 > $null
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
[Console]::InputEncoding  = [System.Text.Encoding]::UTF8

$java = 'java'
if ($env:JAVA_HOME) {
    $candidate = Join-Path $env:JAVA_HOME 'bin\java.exe'
    if (Test-Path $candidate) { $java = $candidate }
}

$jvmArgs = @(
    '-Dfile.encoding=UTF-8'
    '-Dsun.stdout.encoding=UTF-8'
    '-Dsun.stderr.encoding=UTF-8'
    '-cp', $out
    'io.multiai.cli.Main'
    '--workspace', $Workspace
)
if ($Room) { $jvmArgs += @('--room', $Room) }

& $java @jvmArgs
