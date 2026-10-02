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

$name = "docfoo-cli-$version-windows-x64"
$dist = "dist/$name"
Remove-Item -Recurse -Force $dist -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $dist | Out-Null
Copy-Item target/release/docfoo.exe "$dist/"
Copy-Item sidecar/docfoo-agent.exe "$dist/"
Copy-Item install.sh, install.ps1, README.md "$dist/"

# Bundle the MSVC runtime so the archive runs on a clean Windows install:
# docfoo.exe and the downloaded ONNX Runtime import these DLLs, which are not
# part of Windows itself (the UCRT is).
$crtDlls = @("vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll", "msvcp140_1.dll")
foreach ($dll in $crtDlls) {
  $source = Join-Path $env:SystemRoot "System32\$dll"
  if (-not (Test-Path $source)) {
    throw "missing $dll in System32 - install the VC++ 2015-2022 Redistributable first"
  }
  Copy-Item $source (Join-Path $dist $dll)
}
Write-Host "bundled MSVC runtime: $($crtDlls -join ', ')"

# Use libarchive's tar (bundled with Windows 10 1803+) to build the zip:
# PowerShell 5.1's Compress-Archive writes backslash path separators, which
# standard unzip tools reject. `tar -a` picks the zip format from the suffix.
$tar = Join-Path $env:SystemRoot "System32\tar.exe"
if (-not (Test-Path $tar)) { throw "$tar not found; Windows 10 1803+ is required to package" }
& $tar -a -c -f "dist/$name.zip" -C dist $name
if ($LASTEXITCODE -ne 0) { throw "tar failed with exit code $LASTEXITCODE" }
$hash = (Get-FileHash "dist/$name.zip" -Algorithm SHA256).Hash.ToLower()
# LF (not CRLF) so `sha256sum -c` works everywhere.
[System.IO.File]::WriteAllText(
  (Join-Path (Get-Location) "dist/$name.zip.sha256"),
  "$hash  $name.zip`n",
  [System.Text.UTF8Encoding]::new($false)
)
Write-Host "wrote dist/$name.zip"
