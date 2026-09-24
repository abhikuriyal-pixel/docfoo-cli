# Build the DocFoo CLI sidecar with bun (Windows).
# Usage: .\build.ps1 [output]   (default: docfoo-agent.exe in this directory)
param([string]$Out = "docfoo-agent.exe")
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if (-not (Get-Command bun -ErrorAction SilentlyContinue)) {
    Write-Error "bun is required. Install it with: npm i -g bun"
}

if (-not (Test-Path node_modules)) {
    bun install
}
node scripts/dedupe-pi-ai.mjs

$args = @("build", "--compile", "./main.ts", "--outfile", $Out)
if ($env:DOCFOO_SIDECAR_TARGET) {
    $args += @("--target", $env:DOCFOO_SIDECAR_TARGET)
}
Write-Host "Building sidecar: $Out"
& bun @args
Write-Host "Done."
