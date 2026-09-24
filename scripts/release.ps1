# Build a Windows x64 release zip: dist/docfoo-<version>-windows-x64.zip
# plus its .sha256 sidecar.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$versionLine = Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1
if (-not $versionLine) { throw "could not read the workspace version" }
$version = $versionLine.Matches[0].Groups[1].Value

Write-Host "building docfoo $version (windows-x64)…"
cargo build --release
Push-Location sidecar
if (-not (Test-Path node_modules)) { bun install }
./build.ps1 docfoo-agent.exe
Pop-Location

$name = "docfoo-$version-windows-x64"
$dist = "dist/$name"
Remove-Item -Recurse -Force $dist -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $dist | Out-Null
Copy-Item target/release/docfoo.exe "$dist/"
Copy-Item sidecar/docfoo-agent.exe "$dist/"
Copy-Item install.sh, README.md "$dist/"

Compress-Archive -Path $dist -DestinationPath "dist/$name.zip" -Force
$hash = (Get-FileHash "dist/$name.zip" -Algorithm SHA256).Hash.ToLower()
"$hash  $name.zip" | Out-File "dist/$name.zip.sha256" -Encoding ascii
Write-Host "wrote dist/$name.zip"
