<#
  Scarica gli eseguibili esterni (yt-dlp, ffmpeg, spotDL, deno) in
  src-tauri/binaries con i nomi richiesti da Tauri per le "sidecar":
  <nome>-x86_64-pc-windows-msvc.exe

  Uso (PowerShell):  ./scripts/fetch-binaries.ps1
#>
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$triple = 'x86_64-pc-windows-msvc'
$dest = Join-Path $PSScriptRoot '..\src-tauri\binaries'
New-Item -ItemType Directory -Force -Path $dest | Out-Null
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ('audio-ripper-' + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

function Get-File($url, $out) {
    Write-Host "Scarico $url"
    Invoke-WebRequest -Uri $url -OutFile $out -Headers @{ 'User-Agent' = 'audio-ripper-build' }
}

# yt-dlp
Get-File 'https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe' `
    (Join-Path $dest "yt-dlp-$triple.exe")

# ffmpeg (build "essentials": contiene solo ffmpeg, ffplay e ffprobe statici)
$ffZip = Join-Path $tmp 'ffmpeg.zip'
Get-File 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip' $ffZip
Expand-Archive -Path $ffZip -DestinationPath (Join-Path $tmp 'ffmpeg')
$ffExe = Get-ChildItem -Path (Join-Path $tmp 'ffmpeg') -Recurse -Filter 'ffmpeg.exe' | Select-Object -First 1
Copy-Item $ffExe.FullName (Join-Path $dest "ffmpeg-$triple.exe") -Force

# spotDL (eseguibile Windows pubblicato nelle release ufficiali)
$rel = Invoke-RestMethod -Uri 'https://api.github.com/repos/spotDL/spotify-downloader/releases/latest' `
    -Headers @{ 'User-Agent' = 'audio-ripper-build' }
$asset = $rel.assets | Where-Object { $_.name -like '*win*.exe' } | Select-Object -First 1
if (-not $asset) { throw 'Eseguibile Windows di spotDL non trovato nell''ultima release.' }
Get-File $asset.browser_download_url (Join-Path $dest "spotdl-$triple.exe")

# deno: runtime JavaScript che yt-dlp usa per le sfide di YouTube
$denoZip = Join-Path $tmp 'deno.zip'
Get-File 'https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip' $denoZip
Expand-Archive -Path $denoZip -DestinationPath (Join-Path $tmp 'deno')
Copy-Item (Join-Path $tmp 'deno\deno.exe') (Join-Path $dest "deno-$triple.exe") -Force

Remove-Item -Recurse -Force $tmp
Write-Host 'Fatto. Binari in' (Resolve-Path $dest)
