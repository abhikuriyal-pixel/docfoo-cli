# install.ps1 - download, verify and install docfoo for Windows x64.
#
#   irm https://raw.githubusercontent.com/abhikuriyal-pixel/docfoo-cli/master/install.ps1 | iex
#   powershell -ExecutionPolicy Bypass -File install.ps1 [-Version 0.2.2] [-Prefix DIR] [-SkipSetup]
#
# The script is a bootstrap only: it downloads the latest release archive,
# verifies its SHA-256, extracts it under -Prefix, adds that directory to the
# user PATH, and then runs `docfoo setup`, which installs ONNX Runtime, PDFium
# and the PP-DocLayoutV3 model (pinned + checksummed). No Rust, Bun or Git Bash
# is required.
param(
    [string]$Version = "latest",
    [string]$Prefix = (Join-Path $HOME ".docfoo\bin"),
    [switch]$SkipSetup
)

$ErrorActionPreference = "Stop"
# PS 5.1 needs this on older Windows builds, and IWR progress is very slow.
[Net.ServicePointManager]::SecurityProtocol = `
    [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$ProgressPreference = "SilentlyContinue"

$Repo = "abhikuriyal-pixel/docfoo-cli"
if ($env:DOCFOO_REPO) { $Repo = $env:DOCFOO_REPO }
if ($env:DOCFOO_VERSION) { $Version = $env:DOCFOO_VERSION }
if ($env:DOCFOO_PREFIX) { $Prefix = $env:DOCFOO_PREFIX }

if ($Version -eq "latest") {
    Write-Host "resolving the latest release from $Repo..."
    $release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest" `
        -Headers @{ "User-Agent" = "DocFoo-Installer" }
    $Tag = $release.tag_name
} else {
    $Tag = $Version
}
$Tag = $Tag.TrimStart("v")
$Name = "docfoo-cli-$Tag-windows-x64"
$Base = "https://github.com/$Repo/releases/download/v$Tag"

$Tmp = Join-Path ([IO.Path]::GetTempPath()) ("docfoo-install-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $Tmp | Out-Null
try {
    $Zip = Join-Path $Tmp "$Name.zip"
    Write-Host "downloading $Name.zip"
    Invoke-WebRequest -Uri "$Base/$Name.zip" -OutFile $Zip -UseBasicParsing

    $Expected = (Invoke-WebRequest -Uri "$Base/$Name.zip.sha256" -UseBasicParsing).Content.Trim().Split(" ")[0].ToLower()
    $Actual = (Get-FileHash $Zip -Algorithm SHA256).Hash.ToLower()
    if ($Actual -ne $Expected) {
        throw "checksum mismatch for $Name.zip (expected $Expected, got $Actual)"
    }
    Write-Host "sha256 verified: $Actual"

    Expand-Archive -Path $Zip -DestinationPath $Tmp -Force
    New-Item -ItemType Directory -Path $Prefix -Force | Out-Null
    Copy-Item -Path (Join-Path $Tmp "$Name\*") -Destination $Prefix -Force
    Write-Host "installed to $Prefix"
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}

# Add the install directory to the user PATH, keeping the existing value.
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
$UserEntries = if ([string]::IsNullOrWhiteSpace($UserPath)) { @() } else { $UserPath -split ";" }
if ($UserEntries -notcontains $Prefix) {
    $NewPath = if ([string]::IsNullOrWhiteSpace($UserPath)) { $Prefix } else { "$UserPath;$Prefix" }
    [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    Write-Host "added $Prefix to the user PATH (new terminals only)"
}
if (($env:Path -split ";") -notcontains $Prefix) {
    $env:Path = "$env:Path;$Prefix"
}

if (-not $SkipSetup) {
    $Workspace = Join-Path $HOME ".docfoo"
    Write-Host "running: docfoo --workspace `"$Workspace`" setup"
    & (Join-Path $Prefix "docfoo.exe") --workspace $Workspace setup
    if ($LASTEXITCODE -ne 0) {
        throw "docfoo setup failed with exit code $LASTEXITCODE"
    }
}

Write-Host ""
Write-Host "docfoo $Tag installed. Run 'docfoo version' in a new terminal."
