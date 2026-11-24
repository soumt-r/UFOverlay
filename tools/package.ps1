# Builds the release exe and zips it with the files every release must carry.
# usage: pwsh tools/package.ps1      → dist/UFOverlay-<version>.zip
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$stage = Join-Path $root "dist/UFOverlay-$version"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target/release/ufoverlay.exe, LICENSE, THIRD_PARTY_NOTICES.md, README.md, README.ko.md $stage

$zip = Join-Path $root "dist/UFOverlay-$version.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path "$stage/*" -DestinationPath $zip
Write-Host "packaged $zip ($([math]::Round((Get-Item $zip).Length / 1KB)) KB)"
