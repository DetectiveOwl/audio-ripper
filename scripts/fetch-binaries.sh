#!/usr/bin/env bash
# Downloads the external executables (yt-dlp, ffmpeg, spotDL, deno) for macOS into
# src-tauri/binaries, using the names Tauri expects for sidecars:
#   <name>-<target-triple>
#
# Usage:  ./scripts/fetch-binaries.sh [aarch64-apple-darwin | x86_64-apple-darwin]
# Without an argument it uses the architecture of the current Mac.
# Set GITHUB_TOKEN to avoid GitHub API rate limits (the CI does).
set -euo pipefail

triple="${1:-}"
if [ -z "$triple" ]; then
  case "$(uname -m)" in
    arm64) triple="aarch64-apple-darwin" ;;
    x86_64) triple="x86_64-apple-darwin" ;;
    *) echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
  esac
fi
case "$triple" in
  aarch64-apple-darwin) deno_arch="aarch64"; ff_arch="arm64"; want_arm=1 ;;
  x86_64-apple-darwin)  deno_arch="x86_64";  ff_arch="x64";   want_arm=0 ;;
  *) echo "Unsupported target: $triple" >&2; exit 1 ;;
esac

dest="$(cd "$(dirname "$0")/.." && pwd)/src-tauri/binaries"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$dest"

auth=()
if [ -n "${GITHUB_TOKEN:-}" ]; then auth=(-H "Authorization: Bearer $GITHUB_TOKEN"); fi

download() { # url output
  echo "Downloading $1"
  curl -fsSL --retry 3 "${auth[@]}" -o "$2" "$1"
}

sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }

# Stops the build if the file does not have the expected SHA-256.
check_sha256() { # file text-containing-the-hash
  local want got
  want="$(printf '%s' "$2" | grep -oE '[A-Fa-f0-9]{64}' | head -1 | tr 'A-F' 'a-f')"
  if [ -z "$want" ]; then echo "No checksum found for $1" >&2; exit 1; fi
  got="$(sha256_of "$1")"
  if [ "$got" != "$want" ]; then echo "Checksum mismatch for $1: expected $want, got $got" >&2; exit 1; fi
  echo "  sha256 ok ($got)"
}

# Prints "sha256:..." published by GitHub for an asset URL of the latest release, if any
# (GitHub has these only for assets uploaded since mid 2025).
asset_digest() { # owner/repo asset-url
  curl -fsSL --retry 3 "${auth[@]}" "https://api.github.com/repos/$1/releases/latest" \
    | python3 -c 'import json,sys; u=sys.argv[1]; print(next((a.get("digest") or "" for a in json.load(sys.stdin)["assets"] if a["browser_download_url"]==u), ""))' "$2"
}

# Prints the download URLs of the latest release of a GitHub repo.
release_urls() { # owner/repo
  curl -fsSL --retry 3 "${auth[@]}" "https://api.github.com/repos/$1/releases/latest" \
    | grep -o '"browser_download_url": *"[^"]*"' | sed 's/.*"\(http[^"]*\)"/\1/'
}

# yt-dlp: universal binary (Apple Silicon and Intel), checked against the release's SHA2-256SUMS
yt_base="https://github.com/yt-dlp/yt-dlp/releases/latest/download"
download "$yt_base/yt-dlp_macos" "$dest/yt-dlp-$triple"
check_sha256 "$dest/yt-dlp-$triple" "$(curl -fsSL --retry 3 "$yt_base/SHA2-256SUMS" | grep -E '[[:space:]]\*?yt-dlp_macos$')"

# ffmpeg: static build, one per architecture
ff_url="$(release_urls eugeneware/ffmpeg-static | grep -E "/ffmpeg-darwin-$ff_arch\$" | head -1 || true)"
if [ -z "$ff_url" ]; then echo "ffmpeg for macOS ($ff_arch) not found in the latest release." >&2; exit 1; fi
download "$ff_url" "$dest/ffmpeg-$triple"
ff_digest="$(asset_digest eugeneware/ffmpeg-static "$ff_url" || true)"
if [ -n "$ff_digest" ]; then check_sha256 "$dest/ffmpeg-$triple" "$ff_digest"; else echo "  (no published checksum for this ffmpeg build)"; fi

# spotDL: the macOS executable published in the official releases
spot_all="$(release_urls spotDL/spotify-downloader | grep -iE 'darwin|macos' || true)"
if [ -z "$spot_all" ]; then echo "spotDL macOS executable not found in the latest release." >&2; exit 1; fi
echo "spotDL macOS assets:"; echo "$spot_all"
if [ "$want_arm" = 1 ]; then
  spot_url="$(echo "$spot_all" | grep -i 'arm64' | head -1 || true)"
else
  spot_url="$(echo "$spot_all" | grep -iv 'arm64' | head -1 || true)"
fi
if [ -z "$spot_url" ]; then spot_url="$(echo "$spot_all" | head -1)"; fi
download "$spot_url" "$dest/spotdl-$triple"
spot_digest="$(asset_digest spotDL/spotify-downloader "$spot_url" || true)"
if [ -n "$spot_digest" ]; then check_sha256 "$dest/spotdl-$triple" "$spot_digest"; else echo "  (no published checksum for this spotDL release)"; fi

# deno: JavaScript runtime that yt-dlp uses for YouTube challenges
deno_url="https://github.com/denoland/deno/releases/latest/download/deno-$deno_arch-apple-darwin.zip"
download "$deno_url" "$tmp/deno.zip"
check_sha256 "$tmp/deno.zip" "$(curl -fsSL --retry 3 "$deno_url.sha256sum")"
unzip -q -o "$tmp/deno.zip" -d "$tmp/deno"
cp "$tmp/deno/deno" "$dest/deno-$triple"

chmod +x "$dest"/*-"$triple"
echo "Done. Binaries in $dest"
