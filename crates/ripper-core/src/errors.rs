//! Turns the raw error lines of yt-dlp and spotDL into short messages a person
//! can act on. Anything not recognized returns `None`, and the caller shows the
//! raw line instead.

/// Rules are checked in order: the first one whose needles appear in the
/// (lowercased) error wins, so put the more specific cases first.
const LOGIN_HINT: &str = "pick your browser under Login cookies";

fn rules() -> Vec<(&'static [&'static str], String)> {
    vec![
        (&["private video", "this video is private"], "This video is private.".into()),
        (
            &["sign in to confirm your age", "age-restricted", "inappropriate for some users"],
            format!("Age-restricted video: {LOGIN_HINT}."),
        ),
        (
            &["not a bot", "sign in to confirm"],
            format!("YouTube asks for a login: {LOGIN_HINT}."),
        ),
        (
            &["members-only", "join this channel", "members only", "requires payment"],
            format!("Members-only or paid content: {LOGIN_HINT} (logged in)."),
        ),
        (
            &["available in your country", "geo restrict", "geo-restrict", "blocked it in your country"],
            "Not available in your country.".into(),
        ),
        (&["copyright"], "Blocked for copyright reasons.".into()),
        (
            &["premieres in", "live event will begin", "this live event"],
            "Live stream or premiere that has not started yet.".into(),
        ),
        (
            &["video unavailable", "no longer available", "has been removed", "been terminated"],
            "Video unavailable: it was removed or made private.".into(),
        ),
        (&["unsupported url"], "This link is not supported.".into()),
        (
            &["http error 429", "too many requests", "rate limit", "rate-limit"],
            "Too many requests: wait a few minutes and try again.".into(),
        ),
        (
            &["ffmpeg not found", "ffprobe and ffmpeg not found", "ffmpeg is not installed"],
            "Missing component (ffmpeg). Please reinstall the app.".into(),
        ),
        (
            &["unable to extract", "nsig extraction failed", "signature extraction failed", "please report this issue"],
            "The site changed and yt-dlp can no longer read it: click \"Check for updates\" below.".into(),
        ),
        (
            &["requested format is not available", "no video formats found"],
            "No audio found for this link. Try \"Check for updates\" below.".into(),
        ),
        (
            &["http error 403", "403: forbidden"],
            "Access denied (403): try \"Check for updates\" or the Login cookies.".into(),
        ),
        (
            &["no space left", "not enough space", "disk full", "permission denied", "access is denied"],
            "Cannot write to the destination folder (full or no permission).".into(),
        ),
        (
            &[
                "getaddrinfo failed",
                "name or service not known",
                "temporary failure in name resolution",
                "network is unreachable",
                "nodename nor servname",
                "failed to resolve",
                "unable to download webpage",
                "timed out",
                "connection reset",
                "connection refused",
                "urlopen error",
            ],
            "Network error: check your internet connection.".into(),
        ),
        (
            &["no results found", "lookuperror", "could not find"],
            "No matching audio found on YouTube for this track.".into(),
        ),
        (
            &["spotifyerror", "spotify api", "invalid client"],
            "Spotify could not read this link (private, removed or limited).".into(),
        ),
    ]
}

/// Friendly version of an error message, if we know the case.
pub fn explain(raw: &str) -> Option<String> {
    let lower = raw.to_lowercase();
    rules()
        .into_iter()
        .find(|(needles, _)| needles.iter().any(|n| lower.contains(n)))
        .map(|(_, msg)| msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_cases() {
        let cases = [
            ("[youtube] abc: Private video. Sign in if you've been granted access to this video", "private"),
            ("[youtube] abc: Sign in to confirm your age. This video may be inappropriate for some users.", "Age-restricted"),
            ("[youtube] abc: Sign in to confirm you’re not a bot. Use --cookies-from-browser", "login"),
            ("[youtube] abc: Video unavailable. This video is not available", "unavailable"),
            ("[youtube] abc: The uploader has not made this video available in your country", "country"),
            ("[youtube] abc: This video is no longer available because the YouTube account has been terminated", "unavailable"),
            ("HTTP Error 429: Too Many Requests", "Too many"),
            ("Unsupported URL: https://example.com", "not supported"),
            ("ERROR: ffprobe and ffmpeg not found. Please install", "ffmpeg"),
            ("unable to download video data: <urlopen error [Errno 11001] getaddrinfo failed>", "Network"),
            ("[youtube] abc: nsig extraction failed: Some formats may be missing", "Check for updates"),
            ("OSError: [Errno 28] No space left on device", "destination folder"),
            ("LookupError: No results found for song: Foo - Bar", "YouTube"),
        ];
        for (raw, expect) in cases {
            let got = explain(raw).unwrap_or_else(|| panic!("no explanation for: {raw}"));
            assert!(
                got.to_lowercase().contains(&expect.to_lowercase()),
                "for {raw:?}: got {got:?}, wanted something with {expect:?}"
            );
        }
    }

    #[test]
    fn age_gate_wins_over_the_generic_login_rule() {
        let m = explain("Sign in to confirm your age").unwrap();
        assert!(m.starts_with("Age-restricted"));
    }

    #[test]
    fn unknown_is_none() {
        assert_eq!(explain("something weird happened"), None);
        assert_eq!(explain(""), None);
    }
}
