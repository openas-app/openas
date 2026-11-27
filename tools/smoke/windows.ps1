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

function Check($cond, $what) { if (-not $cond) { throw "FAILED: $what" } else { Write-Output "ok: $what" } }

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

# The load check, with the real runtime.
$p = Start-Process $runner -ArgumentList "--check", "$T\root\apps\hello", "--report", "$out\check.json" -PassThru
Check ($p.WaitForExit(60000)) "load check finished"
$report = Get-Content "$out\check.json" -Raw
Write-Output $report
Check ($report -match "example.com") "offline app's fetch was blocked"

# The builder's eyes: a click, a notification, the screenshot.
Set-Content "$T\s.js" "await ui.click('#b'); await app.permissions.request(['notifications']); await app.notify('hi'); return document.getElementById('n').textContent;"
$p = Start-Process $runner -ArgumentList "--snap", "$T\root\apps\hello\app", "$out\shot.png", "900", "600", "$T\s.js", "light" -PassThru
Check ($p.WaitForExit(90000)) "snap finished"
$snap = Get-Content "$out\shot.png.json" -Raw
Write-Output $snap
$s = $snap | ConvertFrom-Json
Check ($s.result -eq "1 clicks") "script clicked ($($s.result))"
Check ($s.desktop.notifications -contains "hi") "simulated notification"
Check ((Get-Item "$out\shot.png").Length -gt 2000) "screenshot written"

# Share and import (a zip both ways).
$f = (& $oa share hello | Select-Object -Last 1)
Check (Test-Path $f) "shared file $f"
& $oa import $f
Check (Test-Path "$T\Apps\Hello 2\Hello 2.exe") "imported as Hello 2"
& $oa reinstall
Check ($LASTEXITCODE -eq 0) "reinstall"
Write-Output "smoke passed"
