# Builds OpenAs from source and installs it for you (no admin rights needed).
# Needs Rust (https://rustup.rs) with the Visual Studio C++ build tools it
# asks for, and the WebView2 runtime (part of Windows 11).
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  Write-Output "Rust is missing. Install it from https://rustup.rs, then run this again."; exit 1
}
$wv = "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
$wvUser = "HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
if (-not ((Test-Path $wv) -or (Test-Path $wvUser))) {
  Write-Output "The WebView2 runtime is missing. Get it from https://developer.microsoft.com/microsoft-edge/webview2/, then run this again."; exit 1
}

Write-Output "Building OpenAs (the first build takes a few minutes)..."
cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& runner\scripts\bundle_windows.ps1 | Out-Null

& target\release\openas.exe setup
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Output ""
Write-Output "OpenAs is in your Start menu and opening now. Choose a model, then describe an app."
