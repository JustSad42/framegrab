<#
.SYNOPSIS
  Builds Framegrab and installs it to %LOCALAPPDATA%\Programs\Framegrab with a Start menu shortcut.
  Run it again to update a local install from source (GitHub releases update installed copies on their own).

.EXAMPLE
  .\install.ps1          # build, install, start
  .\install.ps1 -NoStart # install only
#>
param([switch]$NoStart)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

cargo build --release --quiet
if ($LASTEXITCODE -ne 0) { throw "Build failed." }

$dir = Join-Path $env:LOCALAPPDATA "Programs\Framegrab"
$exe = Join-Path $dir "Framegrab.exe"
New-Item -ItemType Directory -Force $dir | Out-Null

# Stop the running copy (and any editor/settings windows) so the file can be replaced.
Get-Process Framegrab -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe } | Stop-Process -Force
Start-Sleep -Milliseconds 500
Copy-Item target\release\framegrab.exe $exe -Force

$lnk = Join-Path ([Environment]::GetFolderPath("Programs")) "Framegrab.lnk"
$sh = New-Object -ComObject WScript.Shell
$s = $sh.CreateShortcut($lnk)
$s.TargetPath = $exe
$s.WorkingDirectory = $dir
$s.Description = "Framegrab screenshots, text grab and screen recording"
$s.Save()

Write-Host "Installed to $exe" -ForegroundColor Green
if (-not $NoStart) {
    Start-Process $exe
}
