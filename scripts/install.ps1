# Install x on Windows by building from source (cargo) and copying x.exe, or,
# with -FromRelease, by downloading the release installer - no Rust toolchain.
#
#   powershell -c "irm https://raw.githubusercontent.com/wangmingfa/x/main/scripts/install.ps1|iex"
#   (AddToPath is on by default; turn it off with -AddToPath:$false)
param(
    [string]$Prefix = "$env:LOCALAPPDATA\x",
    [switch]$AddToPath = $true,
    [switch]$Yes,
    # Download the x-*-windows-x86_64-setup.exe from GitHub Releases and run it
    # silently instead of building from source. -Prefix is ignored: the
    # installer decides where x.exe goes.
    [switch]$FromRelease,
    # Release tag to install; empty means the latest release.
    [string]$Version = ''
)

$ErrorActionPreference = 'Stop'
$Repo    = 'https://github.com/wangmingfa/x'
$Version = '0.1.0'
$SrcDir  = "$env:TEMP\x-src"

function Info($msg) { Write-Host "==> $msg" }

if ($FromRelease) {
    $repo = 'wangmingfa/x'
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
        Info "hint: add $(Split-Path $x) to your PATH manually (-AddToPath is on by default)"
    }
    exit 0
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo (Rust toolchain) is required. Install from https://rustup.rs"
    exit 1
}

Info "cloning/updating $Repo @ $Version"
if (Test-Path "$SrcDir\.git") {
    git -C $SrcDir fetch --tags --quiet
} else {
    git clone --quiet $Repo $SrcDir
}
git -C $SrcDir checkout --quiet $Version 2>$null
if ($LASTEXITCODE -ne 0) { git -C $SrcDir checkout --quiet main }

Info "building release binary (this can take a few minutes)"
Push-Location $SrcDir
try { cargo build --release --locked -p x-app } finally { Pop-Location }

$BinSrc = "$SrcDir\target\release\x.exe"
if (-not (Test-Path $BinSrc)) {
    Write-Error "build did not produce $BinSrc"
    exit 1
}

New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
Copy-Item $BinSrc -Destination "$Prefix\x.exe" -Force
Info "installed to $Prefix\x.exe"

$installed = & "$Prefix\x.exe" --version 2>$null
Info "installed: $installed"

if ($AddToPath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath -notlike "*$Prefix*") {
        [Environment]::SetEnvironmentVariable('Path', "$userPath;$Prefix", 'User')
        Info "added $Prefix to user PATH (restart the terminal to apply)"
    }
} else {
    Info "hint: add $Prefix to your PATH manually (or -AddToPath:$false to skip this message)"
}
