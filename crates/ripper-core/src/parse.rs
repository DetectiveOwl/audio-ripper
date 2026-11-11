//! Parsing riga per riga dell'output di yt-dlp e spotDL.

use std::path::Path;

#[derive(Debug, PartialEq)]
pub enum YtLine {
    /// Percentuale di download (0-100).
    Progress(u8),
    /// File temporaneo in scrittura.
    Destination(String),
    /// Inizio della conversione audio.
    Converting,
    /// File audio finale.
    Finished(String),
    /// Messaggio di errore di yt-dlp.
    Error(String),
    Other,
}

pub fn parse_ytdlp_line(line: &str) -> YtLine {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("[download]") {
        let rest = rest.trim_start();
        if let Some(dest) = rest.strip_prefix("Destination:") {
            return YtLine::Destination(dest.trim().to_string());
        }
        if let Some(idx) = rest.find('%') {
            if let Ok(p) = rest[..idx].trim().parse::<f32>() {
                return YtLine::Progress(p.clamp(0.0, 100.0) as u8);
            }
        }
        return YtLine::Other;
    }
    if let Some(rest) = line.strip_prefix("[ExtractAudio]") {
        let rest = rest.trim_start();
        if let Some(dest) = rest.strip_prefix("Destination:") {
            return YtLine::Finished(dest.trim().to_string());
        }
        // "Not converting audio X; file is already in target format mp3"
        if let Some(after) = rest.strip_prefix("Not converting audio ") {
            if let Some(end) = after.find("; file is already") {
                return YtLine::Finished(after[..end].trim().to_string());
            }
        }
        return YtLine::Converting;
    }
    if let Some(err) = line.strip_prefix("ERROR:") {
        return YtLine::Error(err.trim().to_string());
    }
    YtLine::Other
}

#[derive(Debug, PartialEq)]
pub enum SpotLine {
    /// "Found 12 songs in ..."
    Found(u32),
    /// `Downloaded "Artista - Titolo": https://...`
    Downloaded(String),
    Other,
}

pub fn parse_spotdl_line(line: &str) -> SpotLine {
    if let Some(start) = line.find("Downloaded \"") {
        let rest = &line[start + "Downloaded \"".len()..];
        if let Some(end) = rest.find('"') {
            return SpotLine::Downloaded(rest[..end].to_string());
        }
    }
    if let Some(start) = line.find("Found ") {
        let rest = &line[start + "Found ".len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && rest[digits.len()..].trim_start().starts_with("song") {
            if let Ok(n) = digits.parse() {
                return SpotLine::Found(n);
            }
        }
    }
    SpotLine::Other
}

/// Nome del file senza cartella né estensione, da usare come titolo.
pub fn title_from_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    Path::new(&normalized)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ytdlp_progress() {
        assert_eq!(parse_ytdlp_line("[download]  42.3% of 3.41MiB at 1.2MiB/s ETA 00:02"), YtLine::Progress(42));
        assert_eq!(parse_ytdlp_line("[download] 100% of 3.41MiB in 00:00:03"), YtLine::Progress(100));
        assert_eq!(parse_ytdlp_line("[download]   0.0% of ~ 3.41MiB"), YtLine::Progress(0));
    }

    #[test]
    fn ytdlp_destination_and_finish() {
        assert_eq!(
            parse_ytdlp_line(r"[download] Destination: C:\Music\Brano.webm"),
            YtLine::Destination(r"C:\Music\Brano.webm".into())
        );
        assert_eq!(
            parse_ytdlp_line(r"[ExtractAudio] Destination: C:\Music\Brano.mp3"),
            YtLine::Finished(r"C:\Music\Brano.mp3".into())
        );
        assert_eq!(
            parse_ytdlp_line("[ExtractAudio] Not converting audio /m/Brano.mp3; file is already in target format mp3"),
            YtLine::Finished("/m/Brano.mp3".into())
        );
        assert_eq!(parse_ytdlp_line("[ExtractAudio] something else"), YtLine::Converting);
    }

    #[test]
    fn ytdlp_error_and_noise() {
        assert_eq!(
            parse_ytdlp_line("ERROR: [youtube] abc: Video unavailable"),
            YtLine::Error("[youtube] abc: Video unavailable".into())
        );
        assert_eq!(parse_ytdlp_line("[youtube] Extracting URL: https://youtu.be/abc"), YtLine::Other);
        assert_eq!(parse_ytdlp_line(""), YtLine::Other);
    }

    #[test]
    fn spotdl_lines() {
        assert_eq!(parse_spotdl_line("Found 12 songs in Mia Playlist (Playlist)"), SpotLine::Found(12));
        assert_eq!(parse_spotdl_line("Found 1 song in X (Song)"), SpotLine::Found(1));
        assert_eq!(
            parse_spotdl_line("Downloaded \"Daft Punk - Around the World\": https://youtu.be/x"),
            SpotLine::Downloaded("Daft Punk - Around the World".into())
        );
        assert_eq!(parse_spotdl_line("Processing query: https://open.spotify.com/..."), SpotLine::Other);
    }

    #[test]
    fn title_extraction() {
        assert_eq!(title_from_path(r"C:\Music\Mio Brano.mp3"), "Mio Brano");
        assert_eq!(title_from_path("/tmp/a.b.mp3"), "a.b");
    }
}
