<#
  Downloads the external executables (yt-dlp, ffmpeg, spotDL, deno) into
  src-tauri/binaries, using the names Tauri expects for sidecars:
  <name>-x86_64-pc-windows-msvc.exe

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

# yt-dlp
Get-File 'https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe' `
    (Join-Path $dest "yt-dlp-$triple.exe")

# ffmpeg ("essentials" build: only static ffmpeg, ffplay and ffprobe)
$ffZip = Join-Path $tmp 'ffmpeg.zip'
Get-File 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip' $ffZip
Expand-Archive -Path $ffZip -DestinationPath (Join-Path $tmp 'ffmpeg')
$ffExe = Get-ChildItem -Path (Join-Path $tmp 'ffmpeg') -Recurse -Filter 'ffmpeg.exe' | Select-Object -First 1
Copy-Item $ffExe.FullName (Join-Path $dest "ffmpeg-$triple.exe") -Force

# spotDL (Windows executable published in the official releases)
$rel = Invoke-RestMethod -Uri 'https://api.github.com/repos/spotDL/spotify-downloader/releases/latest' `
    -Headers @{ 'User-Agent' = 'audio-ripper-build' }
$asset = $rel.assets | Where-Object { $_.name -like '*win*.exe' } | Select-Object -First 1
if (-not $asset) { throw 'spotDL Windows executable not found in the latest release.' }
Get-File $asset.browser_download_url (Join-Path $dest "spotdl-$triple.exe")

# deno: JavaScript runtime that yt-dlp uses for YouTube challenges
$denoZip = Join-Path $tmp 'deno.zip'
Get-File 'https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip' $denoZip
Expand-Archive -Path $denoZip -DestinationPath (Join-Path $tmp 'deno')
Copy-Item (Join-Path $tmp 'deno\deno.exe') (Join-Path $dest "deno-$triple.exe") -Force

Remove-Item -Recurse -Force $tmp
Write-Host 'Done. Binaries in' (Resolve-Path $dest)
