#!/usr/bin/env -S powershell -NoProfile -ExecutionPolicy Bypass -File
# Local mirror of the CI checks (Format, Clippy, Test) from
# .github/workflows/ci.yml. Run this before every commit.
# ASCII only, no BOM: Windows PowerShell 5.1 reads BOM-less files as ANSI.
#
# The first line is a PowerShell comment. It is there so Git Bash can run
# "./scripts/check.ps1" the way it runs check.sh, instead of feeding PowerShell
# source to bash and dying with "syntax error near unexpected token".
# It names -File on purpose: invoked with the path as a bare argument,
# powershell.exe collapses every failure to exit 1, so the code that comes back
# is no longer the code the checks produced.
#
# From PowerShell itself, run it as always:  .\scripts\check.ps1

$Failed = $false

function Invoke-Step {
    param([string]$Name, [scriptblock]$Body)
    Write-Host "==> $Name" -ForegroundColor Cyan
    # $LASTEXITCODE survives the previous step, so a step that never reaches
    # cargo would be judged by the code some *other* step produced - and read as
    # a pass. Clear it first, so "no exit code at all" is this step's own fact.
    # Cargo writes all of its progress to stderr, which moves neither signal, so
    # a green run cannot be turned red by noise.
    $errorsBefore = $Error.Count
    $global:LASTEXITCODE = $null
    & $Body
    $ran = ($null -ne $LASTEXITCODE)
    if (-not $ran -or $LASTEXITCODE -ne 0 -or $Error.Count -gt $errorsBefore) {
        $detail = if ($ran) { "exit $LASTEXITCODE" } else { 'no command ran; is cargo on PATH?' }
        Write-Host "$Name FAILED ($detail)" -ForegroundColor Red
        $script:Failed = $true
    }
}

Invoke-Step "Format" { cargo fmt --all -- --check }
Invoke-Step "Clippy" { cargo clippy --all-targets --all-features -- -D warnings }
Invoke-Step "Test"   { cargo test --workspace }

if ($Failed) {
    exit 1
}
Write-Host "all checks passed" -ForegroundColor Green
