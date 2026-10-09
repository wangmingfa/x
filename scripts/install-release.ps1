# Install x on Windows from GitHub Releases - download the Inno Setup
# installer, run it silently, add the install directory to the user PATH.
#
#   powershell -c "irm https://raw.githubusercontent.com/wangmingfa/x/main/scripts/install-release.ps1|iex"
#
# (no local file left behind: the script is fetched and executed in memory,
# release mode and the PATH step are the defaults)
param(
    # Release tag to install; empty means the latest release.
    [string]$Version = '',
    [switch]$AddToPath = $true
)

$ErrorActionPreference = 'Stop'
$repo = 'wangmingfa/x'

function Info($msg) { Write-Host "==> $msg" }

if (-not $Version) {
    # /releases/latest only answers for stable releases - a repo whose newest
    # release is marked prerelease gets a 404. List releases and take the
    # first entry: GitHub returns them newest-first, prereleases included.
    Info "resolving the latest release of $repo"
    $rel = Invoke-RestMethod "https://api.github.com/repos/$repo/releases"
    $Version = $rel[0].tag_name
}
$asset = "x-$Version-windows-x86_64-setup.exe"
$url = "https://github.com/$repo/releases/download/$Version/$asset"
$setup = Join-Path $env:TEMP $asset

Info "downloading $url"
Invoke-WebRequest -Uri $url -OutFile $setup
if (-not (Test-Path $setup) -or (Get-Item $setup).Length -eq 0) {
    Write-Error "download failed: $setup is missing or empty"
    exit 1
}

Info "running the installer (silently)"
# Inno Setup silent flags. The installer is a per-user build (LocalAppData, no
# admin), so we pin the directory explicitly to match the path we verify below
# and avoid any fallback to a different drive layout.
$dir = "$env:LOCALAPPDATA\x"
Start-Process -FilePath $setup -ArgumentList '/VERYSILENT', '/NORESTART', '/SUPPRESSMSGBOXES', "/DIR=$dir" -Wait

$x = "$env:LOCALAPPDATA\x\x.exe"
if (-not (Test-Path $x)) {
    Write-Error "installer did not produce $x"
    exit 1
}
Info "installed: $(& $x --version 2>$null)"

if ($AddToPath) {
    $dir = Split-Path $x
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath -notlike "*$dir*") {
        [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User')
        Info "added $dir to user PATH (restart the terminal to apply)"
    }
} else {
    Info "hint: run with -AddToPath, or add $(Split-Path $x) to your PATH manually"
}
