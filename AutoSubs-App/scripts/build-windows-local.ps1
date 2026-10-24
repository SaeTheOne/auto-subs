# Local Windows release build: NSIS installer + portable zip.
#
# CI builds this on windows-latest with nothing extra, but a workstation needs
# four knobs that CI gets for free (or never hits):
#
#  * MSVC developer environment (vcvars64). whisper.cpp's Vulkan backend builds
#    a host tool through CMake ExternalProject; that nested configure only finds
#    cl.exe when INCLUDE/LIB/PATH come from vcvars.
#  * Ninja instead of the Visual Studio generator. With the VS generator the
#    nested shader-generator project reports "No CMAKE_C_COMPILER could be
#    found" (CMake 4 + VS 18); Ninja + vcvars works.
#  * CL=/FS. The nested project try-compiles C and C++ concurrently and both
#    cl.exe processes write the same vc140.pdb ("fatal error C1041").
#  * A short CARGO_TARGET_DIR. The nested build path otherwise exceeds
#    CMAKE_OBJECT_PATH_MAX (250 chars) and the compiler check fails.
#
# Signing is intentionally skipped: tauri.conf.json pins a certificate
# thumbprint that only the release machine has. The resulting installer is
# unsigned (SmartScreen will warn).
#
# Usage (from the AutoSubs-App directory):
#   pwsh -File scripts/build-windows-local.ps1
#   pwsh -File scripts/build-windows-local.ps1 -TargetDir 'F:\at' -VulkanSdk 'C:\VulkanSDK\1.4.357.0'

param(
  [string]$TargetDir = 'F:\at',
  [string]$VulkanSdk = 'C:\VulkanSDK\1.4.357.0',
  [string]$VsPath = '',
  [string]$FfmpegZipUrl = 'https://github.com/tmoroney/ffmpeg-binaries/releases/latest/download/ffmpeg-x86_64-pc-windows-msvc.zip'
)

$ErrorActionPreference = 'Stop'
$appDir = Split-Path -Parent $PSScriptRoot          # AutoSubs-App
$repoRoot = Split-Path -Parent $appDir              # repo root
$distDir = Join-Path $repoRoot 'dist'
$binaries = Join-Path $appDir 'src-tauri\binaries'

function Find-VsPath {
  param([string]$Hint)
  if ($Hint -and (Test-Path $Hint)) { return $Hint }
  $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  if (Test-Path $vswhere) {
    $found = & $vswhere -products * -property installationPath | Select-Object -First 1
    if ($found) { return $found }
  }
  throw "Visual Studio Build Tools not found; pass -VsPath"
}

$vs = Find-VsPath -Hint $VsPath
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
if (-not (Test-Path $vcvars)) { throw "vcvars64.bat not found under $vs" }
if (-not (Test-Path $VulkanSdk)) { throw "Vulkan SDK not found at $VulkanSdk; pass -VulkanSdk" }

$llvmBin = Join-Path $vs 'VC\Tools\Llvm\x64\bin'
$ninjaDir = Join-Path $vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja'

New-Item -ItemType Directory -Force -Path $binaries | Out-Null
$sidecar = Join-Path $binaries 'ffmpeg-x86_64-pc-windows-msvc.exe'
if (-not (Test-Path $sidecar)) {
  Write-Host "Downloading the ffmpeg sidecar..."
  $zip = Join-Path $binaries 'ffmpeg.zip'
  Invoke-WebRequest -Uri $FfmpegZipUrl -OutFile $zip -UseBasicParsing
  Expand-Archive -Path $zip -DestinationPath $binaries -Force
  Remove-Item $zip -Force
  Remove-Item (Join-Path $binaries '__MACOSX') -Recurse -Force -ErrorAction SilentlyContinue
  if (-not (Test-Path $sidecar)) { throw "ffmpeg sidecar missing after extraction" }
}

$cfg = Join-Path $env:TEMP 'tauri-nosign.json'
@'
{
  "bundle": {
    "createUpdaterArtifacts": false,
    "windows": {
      "certificateThumbprint": null,
      "signCommand": null
    }
  }
}
'@ | Set-Content -Path $cfg -Encoding ascii

$bat = Join-Path $env:TEMP 'build-autosubs-win.bat'
@(
  '@echo off',
  "call `"$vcvars`" >nul || exit /b 1",
  "set PATH=$ninjaDir;%PATH%",
  "set LIBCLANG_PATH=$llvmBin",
  "set VULKAN_SDK=$VulkanSdk",
  "set PATH=%PATH%;$VulkanSdk\Bin",
  'set CMAKE_GENERATOR=Ninja',
  'set CMAKE_BUILD_PARALLEL_LEVEL=8',
  'set CL=/FS /utf-8',
  "set CARGO_TARGET_DIR=$TargetDir",
  'set RUST_BACKTRACE=1',
  "cd /d $appDir",
  "npx tauri build --bundles nsis --config `"$cfg`" --ci -- --no-default-features --features windows",
  'exit /b %ERRORLEVEL%'
) | Set-Content -Path $bat -Encoding ascii

& cmd /c "`"$bat`""
if ($LASTEXITCODE -ne 0) { throw "tauri build failed with exit code $LASTEXITCODE" }

$release = Join-Path $TargetDir 'release'
$version = (Get-Content (Join-Path $appDir 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json).version
$installer = Get-ChildItem (Join-Path $release 'bundle\nsis') -Filter '*-setup.exe' |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $installer) { throw "NSIS installer not found under $release\bundle\nsis" }

New-Item -ItemType Directory -Force -Path $distDir | Out-Null
Copy-Item $installer.FullName (Join-Path $distDir $installer.Name) -Force
Write-Host "installer: $(Join-Path $distDir $installer.Name)"

# Portable payload: the release folder already holds the exe, the sidecar and
# the bundled resources; config.json is what the app would write on first run.
$portableName = "AutoSubs-$version-portable-x64"
$portable = Join-Path $distDir $portableName
if (Test-Path $portable) { Remove-Item $portable -Recurse -Force }
New-Item -ItemType Directory -Force -Path $portable | Out-Null
Copy-Item (Join-Path $release 'autosubs.exe') $portable
Copy-Item (Join-Path $release 'ffmpeg.exe') $portable
Copy-Item (Join-Path $release 'resources') $portable -Recurse -Force

# Same defaults the app writes on first run (see src-tauri/src/app_config.rs),
# shipped so Microsoft Translator is one edit away.
@'
{
  "_readme": "AutoSubs settings. Set translation.provider to \"google\" or \"microsoft\". For Microsoft fill translation.microsoft.key (Azure Translator key) and translation.microsoft.region, then set provider to \"microsoft\". Changes apply to the next translation, no restart needed.",
  "translation": {
    "provider": "google",
    "fallback": null,
    "concurrency": 2,
    "google": {
      "endpoint": "https://translate.googleapis.com/translate_a/single"
    },
    "microsoft": {
      "endpoint": "https://api.cognitive.microsofttranslator.com",
      "key": "",
      "region": ""
    }
  }
}
'@ | Set-Content -Path (Join-Path $portable 'config.json') -Encoding utf8

$zip = Join-Path $distDir "$portableName.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $portable -DestinationPath $zip -CompressionLevel Fastest
Write-Host "portable:  $zip"
