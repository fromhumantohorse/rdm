# Builds RDM-<version>-<arch>-setup.msi (per-user installer) at the repository root.
#   powershell -ExecutionPolicy Bypass -File packaging\windows\build-msi.ps1 [-NoBuild] [-Arch x64|arm64] [-Exe <rdm.exe>]
# -Arch: this computer's by default; -Exe: a program built elsewhere (target\release\rdm.exe by default).
# Needs WiX 3 (candle.exe / light.exe): on PATH, in "WiX Toolset v3.x", or %LOCALAPPDATA%\Programs\wix3.
param(
    [switch]$NoBuild,
    [ValidateSet('x64', 'arm64')][string]$Arch = $(if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }),
    [string]$Exe
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
Set-Location $root

$candle = @(
    (Get-Command candle.exe -ErrorAction SilentlyContinue).Source,
    (Get-ChildItem "${env:ProgramFiles(x86)}\WiX Toolset v3*\bin\candle.exe" -ErrorAction SilentlyContinue | Select-Object -Last 1).FullName,
    (Join-Path $env:LOCALAPPDATA 'Programs\wix3\candle.exe')
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $candle) { throw 'WiX 3 not found (candle.exe).' }
$bin = Split-Path $candle

$manifest = Get-Content Cargo.toml -Raw
$version = [regex]::Match($manifest, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
$repository = [regex]::Match($manifest, '(?m)^repository\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $NoBuild) { cargo build --release -p rdm; if ($LASTEXITCODE) { throw 'cargo build failed' } }

$obj = Join-Path $root 'target\wix'
New-Item -ItemType Directory -Force $obj | Out-Null
$out = Join-Path $root "RDM-$version-$Arch-setup.msi"
if (-not $Exe) { $Exe = "$root\target\release\rdm.exe" }
& "$bin\candle.exe" -nologo -arch $Arch -ext WixUtilExtension -out "$obj\rdm.wixobj" `
    "-dVersion=$version" "-dRepository=$repository" "-dExe=$Exe" "-dAssets=$root\crates\app\assets" `
    packaging\windows\rdm.wxs
if ($LASTEXITCODE) { throw 'candle failed' }
& "$bin\light.exe" -nologo -ext WixUtilExtension -sice:ICE91 -cultures:en-US -out $out "$obj\rdm.wixobj"
if ($LASTEXITCODE) { throw 'light failed' }
Write-Output "MSI: $out"
