# Install x on Windows by building from source (cargo) and copying x.exe.
#
#   iwr https://raw.githubusercontent.com/xsys/x/main/scripts/install.ps1 -OutFile install.ps1; .\install.ps1 -AddToPath
param(
    [string]$Prefix = "$env:LOCALAPPDATA\x",
    [switch]$AddToPath,
    [switch]$Yes
)

$ErrorActionPreference = 'Stop'
$Repo    = 'https://github.com/xsys/x'
$Version = '0.1.0'
$SrcDir  = "$env:TEMP\x-src"

function Info($msg) { Write-Host "==> $msg" }

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
    Info "hint: run with -AddToPath, or add $Prefix to your PATH manually"
}
