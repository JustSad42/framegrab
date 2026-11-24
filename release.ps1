<#
.SYNOPSIS
  Publishes a new Framegrab version. Installed copies update themselves while idle.

.EXAMPLE
  .\release.ps1 0.2.0 "Faster scrolling capture"

  1. Sets the version in Cargo.toml
  2. Checks that it builds and the tests pass
  3. Commits, tags v0.2.0 and pushes; the "Release" workflow builds Framegrab.exe + .sha256
#>
param(
    [Parameter(Mandatory)][string]$Version,
    [string]$Notes = "",
    [switch]$Wait
)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version must look like 1.2.3" }
$tag = "v$Version"
if (git tag --list $tag) { throw "$tag already exists. Pick a higher version." }
if ((git config user.email) -notmatch 'noreply') { throw "Set the repo's git email to the GitHub no-reply address first." }

$toml = Get-Content Cargo.toml -Raw
$current = [regex]::Match($toml, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if ([version]$Version -lt [version]$current) { throw "New version $Version must not be lower than $current." }
$toml = [regex]::Replace($toml, '(?m)^version\s*=\s*"[^"]+"', "version = `"$Version`"", 1)
Set-Content Cargo.toml $toml -NoNewline

cargo test --release --quiet
if ($LASTEXITCODE -ne 0) { throw "Tests failed - nothing was published." }
cargo build --release --quiet
if ($LASTEXITCODE -ne 0) { throw "Build failed - nothing was published." }

if (-not $Notes) { $Notes = "Framegrab $Version" }
git add -A
git commit -m "Release $tag" -m $Notes
git tag -a $tag -m $Notes
git push
git push origin $tag

Write-Host "Pushed $tag. GitHub is building the release:" -ForegroundColor Green
Write-Host "  https://github.com/JustSad42/framegrab/actions"
if ($Wait) {
    Start-Sleep -Seconds 5
    gh run watch (gh run list --workflow release.yml --limit 1 --json databaseId -q ".[0].databaseId") --exit-status
    gh release view $tag
}
