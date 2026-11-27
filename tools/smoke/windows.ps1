# End to end on Windows without a model: install a hand-written app, check
# it loads, take the builder's screenshot, share and import it. A scratch
# setup (OPENAS_ROOT / OPENAS_INSTALL_DIR) keeps it apart from a real one.
$ErrorActionPreference = "Stop"
$repo = Resolve-Path "$PSScriptRoot\..\.."
$T = Join-Path $env:RUNNER_TEMP "oa-smoke"
if (Test-Path $T) { Remove-Item -Recurse -Force $T }
New-Item -ItemType Directory $T | Out-Null
$env:OPENAS_ROOT = "$T\root"
$env:OPENAS_INSTALL_DIR = "$T\Apps"
$env:OPENAS_ID_PREFIX = "app.openas.ci."
$env:OPENAS_QUIET_WINDOW = "1"
$oa = Join-Path $repo "target\release\openas.exe"
$runner = Join-Path $repo "target\Runner\Runner.exe"
$out = Join-Path $repo "target\smoke"
New-Item -ItemType Directory -Force $out | Out-Null

function Check($cond, $what) { if (-not $cond) { throw "FAILED: $what" } else { Write-Host "ok: $what" } }

function LoadCheck($folder, $name) {
  $p = Start-Process $runner -ArgumentList "--check", $folder, "--report", "$out\$name.json" -PassThru -RedirectStandardError "$out\$name.err.txt" -RedirectStandardOutput "$out\$name.out.txt"
  Check ($p.WaitForExit(90000)) "load check finished ($name)"
  $r = Get-Content "$out\$name.json" -Raw
  Write-Host $r
  Get-Content "$out\$name.err.txt" -ErrorAction SilentlyContinue | Select-Object -Last 20 | ForEach-Object { Write-Host "  stderr: $_" }
  return $r
}

# The load check on the hand-written app as it is, before anything is installed.
$plain = "$T\plain"
Copy-Item -Recurse "$repo\tools\smoke\hello" $plain
$r = LoadCheck $plain "check-plain"
Check ($r -match "example.com") "offline app's fetch was blocked (plain folder)"

# A draft, as the builder leaves one.
$d = "$T\root\apps\.draft-1"
New-Item -ItemType Directory -Force "$d\app" | Out-Null
Copy-Item "$repo\tools\smoke\hello\app\index.html" "$d\app\index.html"
Set-Content "$d\icon.svg" '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024"><rect width="1024" height="1024" fill="#2a6"/><circle cx="512" cy="512" r="200" fill="#fff"/></svg>'
Set-Content "$d\draft.json" '{"provider":"test","model":"none"}'
Set-Content "$d\prompt.txt" 'hello'

& $oa keep $d "Hello"
Check ($LASTEXITCODE -eq 0) "openas keep"
$app = "$T\Apps\Hello"
Check (Test-Path "$app\Hello.exe") "installed Hello.exe"
Check (Test-Path "$app\openas-app.json") "identity file"
Check (Test-Path "$app\app.ico") "icon"
$info = Get-Content "$T\root\apps\hello\app.json" | ConvertFrom-Json
Check ($info.cdhash.Length -eq 64) "seal recorded ($($info.cdhash))"
Get-Content "$app\openas-app.json"
& $oa list

# keep opened the installed app; it should be running as itself.
Start-Sleep 3
$running = Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq "$app\Hello.exe" }
Write-Output "Hello.exe running: $(@($running).Count)"
Get-Process msedgewebview2 -ErrorAction SilentlyContinue | Measure-Object | ForEach-Object { "web view processes: $($_.Count)" }

# The load check on the installed app's folder, while the app is open.
$r = LoadCheck "$T\root\apps\hello" "check-kept-open"
if (-not ($r -match "example.com")) {
  Write-Output "check failed with the app open; closing it and trying again"
  $running | Stop-Process -Force
  Start-Sleep 2
  $r = LoadCheck "$T\root\apps\hello" "check-kept-closed"
}
Check ($r -match "example.com") "offline app's fetch was blocked (installed app's folder)"
$running | Stop-Process -Force -ErrorAction SilentlyContinue

# The builder's eyes: a click, a notification, the screenshot.
Set-Content "$T\s.js" "await ui.click('#b'); await app.permissions.request(['notifications']); await app.notify('hi'); return document.getElementById('n').textContent;"
$p = Start-Process $runner -ArgumentList "--snap", "$T\root\apps\hello\app", "$out\shot.png", "900", "600", "$T\s.js", "light" -PassThru
Check ($p.WaitForExit(120000)) "snap finished"
$snap = Get-Content "$out\shot.png.json" -Raw
Write-Output $snap
$s = $snap | ConvertFrom-Json
Check ($s.result -eq "1 clicks") "script clicked ($($s.result))"
Check ($s.desktop.notifications -contains "hi") "simulated notification"
Check ((Get-Item "$out\shot.png").Length -gt 2000) "screenshot written"

# Share and import (a zip both ways).
$f = (& $oa share hello $T | Select-Object -Last 1)
Check (Test-Path $f) "shared file $f"
& $oa import $f
Check (Test-Path "$T\Apps\Hello 2\Hello 2.exe") "imported as Hello 2"
& $oa reinstall
Check ($LASTEXITCODE -eq 0) "reinstall"
Write-Output "smoke passed"
