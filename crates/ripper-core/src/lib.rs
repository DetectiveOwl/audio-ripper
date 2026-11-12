//! Audio Ripper logic, independent of the graphical interface.
//!
//! The crate knows nothing about Tauri: it takes a URL, a format and a
//! destination folder, runs yt-dlp or spotDL as child processes and reports
//! progress through a callback. This makes it testable without a window.

pub mod jobobject;
pub mod parse;
pub mod runner;

use url::Url;

/// Output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mp3,
    Wav,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mp3" => Some(Format::Mp3),
            "wav" => Some(Format::Wav),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Format::Mp3 => "mp3",
            Format::Wav => "wav",
        }
    }
}

/// Which program performs the download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// YouTube and SoundCloud: yt-dlp.
    Direct,
    /// Spotify links: spotDL (metadata from Spotify, audio from YouTube).
    Spotify,
}

const ALLOWED_HOSTS: [&str; 6] = [
    "youtube.com",
    "youtu.be",
    "music.youtube.com",
    "soundcloud.com",
    "on.soundcloud.com",
    "open.spotify.com",
];

/// Checks that the URL is http(s) and that the host is on the allowlist.
/// Returns the normalized URL and the source to use.
pub fn validate_url(raw: &str) -> Result<(String, Source), String> {
    const ERR: &str =
        "URL not allowed. Supported sources: YouTube, SoundCloud, Spotify.";
    let parsed = Url::parse(raw.trim()).map_err(|_| ERR.to_string())?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(ERR.to_string());
    }
    let mut host = parsed.host_str().ok_or_else(|| ERR.to_string())?.to_ascii_lowercase();
    for prefix in ["www.", "m."] {
        if let Some(rest) = host.strip_prefix(prefix) {
            host = rest.to_string();
        }
    }
    if !ALLOWED_HOSTS.contains(&host.as_str()) {
        return Err(ERR.to_string());
    }
    let source = if host == "open.spotify.com" { Source::Spotify } else { Source::Direct };
    Ok((parsed.to_string(), source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_known_hosts() {
        assert_eq!(validate_url("https://www.youtube.com/watch?v=abc").unwrap().1, Source::Direct);
        assert_eq!(validate_url("https://m.youtube.com/watch?v=abc").unwrap().1, Source::Direct);
        assert_eq!(validate_url("https://youtu.be/abc").unwrap().1, Source::Direct);
        assert_eq!(validate_url("https://soundcloud.com/a/b").unwrap().1, Source::Direct);
        assert_eq!(validate_url("  https://open.spotify.com/track/xyz  ").unwrap().1, Source::Spotify);
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            "",
            "ftp://youtube.com/x",
            "file:///C:/Windows/win.ini",
            "https://evil.com/youtube.com",
            "https://youtube.com.evil.com/x",
            "--exec=calc",
            "https://example.org",
        ] {
            assert!(validate_url(bad).is_err(), "{bad} should have been rejected");
        }
    }

    #[test]
    fn format_parse() {
        assert_eq!(Format::parse(" MP3 "), Some(Format::Mp3));
        assert_eq!(Format::parse("wav"), Some(Format::Wav));
        assert_eq!(Format::parse("flac"), None);
    }
}
