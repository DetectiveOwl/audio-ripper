#!/usr/bin/env bash
# setup.sh — installazione di audio-ripper su Ubuntu (Oracle Cloud Free Tier, ARM Ampere).
# Eseguire dalla cartella del progetto: cd ~/audio-ripper && bash setup.sh
set -euo pipefail

APP_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$APP_DIR"

echo "==> [1/5] Pacchetti di sistema (ffmpeg, python3, venv, deno)…"
sudo apt-get update
sudo apt-get install -y ffmpeg python3 python3-venv python3-pip curl unzip

# Deno: runtime JS richiesto da yt-dlp per le challenge di YouTube (EJS)
if ! command -v deno >/dev/null 2>&1; then
  curl -fsSL https://deno.land/install.sh | sh -s -- -y
  sudo ln -sf "$HOME/.deno/bin/deno" /usr/local/bin/deno
fi

echo "==> [2/5] Ambiente virtuale Python…"
if [ ! -d venv ]; then
  python3 -m venv venv
fi
./venv/bin/pip install --upgrade pip
./venv/bin/pip install -r requirements.txt

echo "==> [3/5] Servizio systemd…"
sudo cp "$APP_DIR/audio-ripper.service" /etc/systemd/system/audio-ripper.service
sudo systemctl daemon-reload
sudo systemctl enable --now audio-ripper

echo "==> [4/5] Tailscale (accesso privato)…"
if ! command -v tailscale >/dev/null 2>&1; then
  curl -fsSL https://tailscale.com/install.sh | sh
fi

echo "==> [5/5] Fatto."
cat <<'EOF'

================================================================
 audio-ripper installato.

 1) Collega la VM alla tua rete Tailscale (apre un link di login):
       sudo tailscale up

 2) Il server ascolta SOLO su 127.0.0.1:8000. Per raggiungerlo
    dagli altri tuoi dispositivi sulla tailnet:
       sudo tailscale serve --bg 8000
    poi apri l'URL https://<nome-macchina>.<tailnet>.ts.net

 3) Stato del servizio:
       systemctl status audio-ripper
       journalctl -u audio-ripper -f

 4) (Opzionale) cookies.txt nella cartella del progetto per i
    contenuti che richiedono login — vedi README.md.
================================================================
EOF
