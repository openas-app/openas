# Makes the prebuilt Runner folder that installs copy: target\Runner\Runner.exe.
$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot\..\.."
$R = Join-Path $root "target\Runner"
if (Test-Path $R) { Remove-Item -Recurse -Force $R }
New-Item -ItemType Directory $R | Out-Null
Copy-Item (Join-Path $root "target\release\runner.exe") (Join-Path $R "Runner.exe")
Write-Output $R
