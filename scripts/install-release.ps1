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
    Info "resolving the latest release of $repo"
    $rel = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest"
    $Version = $rel.tag_name
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
# Inno Setup silent flags; the installer places x.exe on disk itself.
Start-Process -FilePath $setup -ArgumentList '/VERYSILENT', '/NORESTART' -Wait

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
