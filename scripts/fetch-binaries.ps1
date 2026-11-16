<#
  Downloads the external executables (yt-dlp, ffmpeg, spotDL, deno) into
  src-tauri/binaries, using the names Tauri expects for sidecars:
  <name>-x86_64-pc-windows-msvc.exe
  Every download is checked against the SHA-256 its publisher provides, and
  the build stops on a mismatch. The app keeps yt-dlp and spotDL up to date
  on its own afterwards (see crates/ripper-core/src/update.rs).

  Usage (PowerShell):  ./scripts/fetch-binaries.ps1
#>
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$triple = 'x86_64-pc-windows-msvc'
$dest = Join-Path $PSScriptRoot '..\src-tauri\binaries'
New-Item -ItemType Directory -Force -Path $dest | Out-Null
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ('audio-ripper-' + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

function Get-File($url, $out) {
    Write-Host "Downloading $url"
    Invoke-WebRequest -Uri $url -OutFile $out -Headers @{ 'User-Agent' = 'audio-ripper-build' }
}

# Downloads a checksum file and returns its text (servers often send it as
# binary, which PowerShell hands back as a byte array).
function Get-Text($url) {
    $content = (Invoke-WebRequest -Uri $url -Headers @{ 'User-Agent' = 'audio-ripper-build' } -UseBasicParsing).Content
    if ($content -is [byte[]]) { return [System.Text.Encoding]::UTF8.GetString($content) }
    return [string]$content
}

# Stops the build if the file's SHA-256 is not the expected one.
function Assert-Sha256($file, $expected) {
    if (-not ($expected -match '[A-Fa-f0-9]{64}')) { throw "No checksum found for $file" }
    $want = $Matches[0].ToLower()
    $got = (Get-FileHash -Algorithm SHA256 -Path $file).Hash.ToLower()
    if ($got -ne $want) { throw "Checksum mismatch for ${file}: expected $want, got $got" }
    Write-Host "  sha256 ok ($got)"
}

# yt-dlp (checked against the SHA2-256SUMS file of the same release)
$ytBase = 'https://github.com/yt-dlp/yt-dlp/releases/latest/download'
$ytOut = Join-Path $dest "yt-dlp-$triple.exe"
Get-File "$ytBase/yt-dlp.exe" $ytOut
$ytSums = Get-Text "$ytBase/SHA2-256SUMS"
Assert-Sha256 $ytOut (($ytSums -split "`n") | Where-Object { $_ -match '\s\*?yt-dlp\.exe\s*$' } | Select-Object -First 1)

# ffmpeg ("essentials" build: only static ffmpeg, ffplay and ffprobe)
$ffZip = Join-Path $tmp 'ffmpeg.zip'
Get-File 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip' $ffZip
Assert-Sha256 $ffZip (Get-Text 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip.sha256')
Expand-Archive -Path $ffZip -DestinationPath (Join-Path $tmp 'ffmpeg')
$ffExe = Get-ChildItem -Path (Join-Path $tmp 'ffmpeg') -Recurse -Filter 'ffmpeg.exe' | Select-Object -First 1
Copy-Item $ffExe.FullName (Join-Path $dest "ffmpeg-$triple.exe") -Force

# spotDL (Windows executable published in the official releases)
$rel = Invoke-RestMethod -Uri 'https://api.github.com/repos/spotDL/spotify-downloader/releases/latest' `
    -Headers @{ 'User-Agent' = 'audio-ripper-build' }
$asset = $rel.assets | Where-Object { $_.name -like '*win*.exe' } | Select-Object -First 1
if (-not $asset) { throw 'spotDL Windows executable not found in the latest release.' }
$spotOut = Join-Path $dest "spotdl-$triple.exe"
Get-File $asset.browser_download_url $spotOut
# GitHub publishes a digest only for assets uploaded since mid 2025
if ($asset.digest) { Assert-Sha256 $spotOut $asset.digest } else { Write-Host '  (no published checksum for this spotDL release)' }

# deno: JavaScript runtime that yt-dlp uses for YouTube challenges
$denoZip = Join-Path $tmp 'deno.zip'
$denoUrl = 'https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip'
Get-File $denoUrl $denoZip
Assert-Sha256 $denoZip (Get-Text "$denoUrl.sha256sum")
Expand-Archive -Path $denoZip -DestinationPath (Join-Path $tmp 'deno')
Copy-Item (Join-Path $tmp 'deno\deno.exe') (Join-Path $dest "deno-$triple.exe") -Force

Remove-Item -Recurse -Force $tmp
Write-Host 'Versions:'
Write-Host '  yt-dlp' (& $ytOut --version)
Write-Host '  spotDL' $rel.tag_name
Write-Host 'Done. Binaries in' (Resolve-Path $dest)
