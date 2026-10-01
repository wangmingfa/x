# Local mirror of the CI checks (Format, Clippy, Test) from
# .github/workflows/ci.yml. Run this before every commit.
# ASCII only, no BOM: Windows PowerShell 5.1 reads BOM-less files as ANSI.

$Failed = $false

function Invoke-Step {
    param([string]$Name, [scriptblock]$Body)
    Write-Host "==> $Name" -ForegroundColor Cyan
    & $Body
    if ($LASTEXITCODE -ne 0) {
        Write-Host "$Name FAILED (exit $LASTEXITCODE)" -ForegroundColor Red
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
