$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

function Invoke-Gate {
    param([string]$Program, [string[]]$Arguments)
    Write-Host "`n> $Program $($Arguments -join ' ')"
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Gate failed (exit $LASTEXITCODE): $Program" }
}

Invoke-Gate rustc @('--version')
Invoke-Gate node @('--version')
Invoke-Gate python @('--version')
Invoke-Gate cargo @('llvm-cov', '--version')
Invoke-Gate cargo @('fmt', '--all', '--', '--check')
Invoke-Gate cargo @('check', '--offline', '--all-targets')
Invoke-Gate cargo @('clippy', '--offline', '--all-targets', '--', '-D', 'warnings')
Invoke-Gate cargo @('test', '--offline', '--all-targets')
Invoke-Gate cargo @('build', '--offline', '--bin', 'win-mouse-fix')
Invoke-Gate npx.cmd @('--no-install', 'playwright', 'test')
Invoke-Gate python @('tools/drag_smoke.py')
Invoke-Gate python @('tools/drag_mutation.py')
Write-Host 'Coverage report is diagnostic; no 100% changed-line coverage claim.'
Invoke-Gate cargo @('llvm-cov', 'test', '--offline', '--bin', 'win-mouse-fix', '--json', '--output-path', 'target/drag-coverage.json', 'remote_input::tests::')
Invoke-Gate python @('tools/drag_smoke.py', '--source-state')
Write-Host 'PASS drag lifecycle gauntlet'
