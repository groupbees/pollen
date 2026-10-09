# Install or upgrade pollen from its GitHub release — Windows.
#
#   install.ps1 [-Version v0.4.0] [-InstallDir <dir>]
#
# Version defaults to the latest release, InstallDir to
# %LOCALAPPDATA%\Programs\pollen, which is added to the user PATH once. The
# archive is checked against the release's SHA256SUMS before anything is
# installed.
param(
    [string]$Version = "",
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Programs\pollen")
)
$ErrorActionPreference = "Stop"
# The download progress bar slows Invoke-WebRequest to a crawl on Windows PowerShell 5.1.
$ProgressPreference = "SilentlyContinue"
# Windows PowerShell 5.1 may still offer TLS 1.0, which GitHub refuses.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$repo = "groupbees/pollen"

if (-not $Version) {
    $Version = (Invoke-RestMethod -UseBasicParsing "https://api.github.com/repos/$repo/releases/latest").tag_name
}
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "aarch64" } else { "x86_64" }
$archive = "pollen-$arch-pc-windows-msvc-$Version.zip"
$base = "https://github.com/$repo/releases/download/$Version"
$work = Join-Path ([System.IO.Path]::GetTempPath()) "pollen-$Version-$([guid]::NewGuid())"
New-Item -ItemType Directory -Force $work | Out-Null

try {
    Invoke-WebRequest -UseBasicParsing "$base/$archive" -OutFile (Join-Path $work $archive)
    Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS" -OutFile (Join-Path $work "SHA256SUMS")

    $line = Get-Content (Join-Path $work "SHA256SUMS") | Where-Object { $_ -match [regex]::Escape($archive) }
    if (-not $line) { throw "$archive is not listed in SHA256SUMS" }
    $expected = ($line -split '\s+')[0]
    $actual = (Get-FileHash (Join-Path $work $archive) -Algorithm SHA256).Hash
    if ($actual -ne $expected.ToUpper()) { throw "checksum mismatch for $archive" }

    Expand-Archive (Join-Path $work $archive) -DestinationPath $work -Force
    New-Item -ItemType Directory -Force $InstallDir | Out-Null
    # Two-argument Join-Path only: Windows PowerShell 5.1 has no -AdditionalChildPath.
    $extracted = Join-Path $work ($archive -replace '\.zip$', '')
    Copy-Item (Join-Path $extracted "pollen.exe") $InstallDir -Force
    Write-Output "installed pollen $Version in $InstallDir"
}
finally {
    Remove-Item -Recurse -Force $work
}

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not (($userPath -split ';') -contains $InstallDir)) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
    Write-Output "added $InstallDir to the user PATH — open a new terminal."
}
