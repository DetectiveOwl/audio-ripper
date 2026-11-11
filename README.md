# Audio Ripper

App **Windows** per estrarre l'audio da **YouTube**, **SoundCloud** e link **Spotify** in
**MP3 320 kbps** o **WAV**, con l'interfaccia a rack analogico di prima.

Scritta in **Rust** con **Tauri 2**. Non c'è nessun server: l'app apre una finestra
(WebView2, già presente in Windows 11), lancia yt-dlp, spotDL e ffmpeg come processi
figli e salva i file direttamente nella cartella che scegli. Quando chiudi la finestra
non resta niente in esecuzione.

## Come funziona

La parte Rust sta in due crate. `crates/ripper-core` contiene la logica e non conosce la
grafica: valida l'URL contro una lista di host ammessi, avvia yt-dlp (o spotDL per i link
Spotify), legge l'output riga per riga per ricavare avanzamento e titolo, e supporta
l'annullamento. `src-tauri` è il guscio dell'app: finestra, scelta cartella, comandi
chiamati dall'interfaccia ed eventi di avanzamento verso di lei. L'interfaccia è in `ui/`
(HTML, CSS e JS senza build).

Per la chiusura pulita l'app, all'avvio, si inserisce in un **Windows Job Object** con
`KILL_ON_JOB_CLOSE`: se la finestra si chiude, anche nel mezzo di un download, Windows
termina da solo tutti i processi figli (yt-dlp, ffmpeg, spotDL). Il pulsante **Annulla**
ferma il download in corso e i suoi discendenti.

I file vanno nella cartella di destinazione (di default **Musica**, cambiabile dall'app e
ricordata). Non ci sono più copie temporanee né pulizia a tempo.

## Scaricare l'app

Su GitHub, scheda **Actions**, workflow **Build Windows**, **Run workflow**. A fine build
trovi due artefatti: l'**installer** (`.exe`, installazione per utente senza permessi di
amministratore) e la **versione portatile** (`.zip`, scompatti e avvii `audio-ripper.exe`).
In entrambi yt-dlp, ffmpeg, spotDL e deno sono già inclusi.

## Compilare in locale (Windows)

Servono Rust (`rustup`), Node.js e PowerShell.

```powershell
./scripts/fetch-binaries.ps1          # scarica yt-dlp, ffmpeg, spotDL, deno
npx @tauri-apps/cli@2 dev             # finestra in modalità sviluppo
npx @tauri-apps/cli@2 build           # installer in target/release/bundle/nsis
```

I test della logica girano anche senza Windows: `cargo test -p ripper-core`.

## Note operative

- Alcuni contenuti YouTube richiedono un `cookies.txt` in formato Netscape. Mettilo in
  `%APPDATA%\dev.audioripper.app\cookies.txt` e verrà usato da yt-dlp e spotDL.
- yt-dlp e spotDL seguono bersagli mobili: per aggiornarli basta rilanciare il build, che
  scarica sempre le ultime versioni.
- Il formato WAV non migliora una sorgente già compressa: è solo un contenitore senza
  ulteriore perdita.
