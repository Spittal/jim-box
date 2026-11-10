//! Text normalization shared by metadata matching and Soulseek ranking.

use deunicode::deunicode;

/// Parenthetical/bracketed annotations that don't change which recording a title refers to.
const NOISE_MARKERS: [&str; 8] = [
    "feat", "ft.", "with ", "remaster", "explicit", "mono", "stereo", "bonus",
];

/// ASCII-folded, lowercased, punctuation replaced by single spaces.
pub fn normalize(s: &str) -> String {
    let folded = deunicode(s).to_lowercase();
    let spaced: String = folded
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalized words, dropping "and" since titles alternate between "and" and "&".
pub fn tokens(s: &str) -> Vec<String> {
    normalize(s)
        .split(' ')
        .filter(|t| !t.is_empty() && *t != "and")
        .map(str::to_string)
        .collect()
}

/// Removes annotations like "(feat. X)" or "[Remastered 2011]" from a title.
pub fn strip_noise(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut rest = title;
    while let Some(start) = rest.find(['(', '[']) {
        let close = if rest[start..].starts_with('(') { ')' } else { ']' };
        let Some(len) = rest[start..].find(close) else { break };
        let inner = rest[start + 1..start + len].to_lowercase();
        out.push_str(&rest[..start]);
        if !NOISE_MARKERS.iter().any(|m| inner.contains(m)) {
            out.push_str(&rest[start..=start + len]);
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);

    // "Song - Remastered 2009" style suffixes.
    if let Some((head, tail)) = out.rsplit_once(" - ") {
        if NOISE_MARKERS.iter().any(|m| tail.to_lowercase().contains(m)) {
            return head.trim().to_string();
        }
    }
    // Unbracketed "Song feat. Someone".
    let out = cut_at_first(&out, &FEATURING);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

const FEATURING: [&str; 5] = [" feat. ", " feat ", " ft. ", " ft ", " featuring "];

/// The lead artist of a credit like "Justin Bieber feat. Nicki Minaj" or "A & B".
/// Too aggressive for display ("Simon & Garfunkel" → "Simon"); used for search.
pub fn primary_artist(artist: &str) -> &str {
    cut_at_first(artist, &[&FEATURING[..], &[" & ", " x ", ", "][..]].concat())
}

/// A credit without its featured artists: "Flo Rida feat. T-Pain" → "Flo Rida".
/// Keeps duos like "Simon & Garfunkel" intact.
pub fn lead_artist(artist: &str) -> &str {
    cut_at_first(artist, &FEATURING)
}

fn cut_at_first<'a>(s: &'a str, separators: &[&str]) -> &'a str {
    // ASCII lowercasing keeps byte offsets valid for slicing `s`.
    let lower = s.to_ascii_lowercase();
    let cut = separators.iter().filter_map(|sep| lower.find(sep)).min().unwrap_or(s.len());
    s[..cut].trim()
}

/// Words marking a title annotation as a version rather than part of the name,
/// e.g. "(Four Tet Remix)" but not "(Drop It to the Floor)".
const VERSION_MARKERS: [&str; 20] = [
    "remix", "remixed", "mix", "edit", "rerub", "dub", "version", "live", "instrumental", "acoustic",
    "rework", "vip", "refix", "flip", "demo", "extended", "radio", "reprise", "session", "sesh",
];

/// Words of annotations that only name the default cut of a track, e.g.
/// "(Radio Edit)" or "(Original Mix)". Files usually omit these.
const PLAIN_VERSION_WORDS: [&str; 10] =
    ["radio", "edit", "single", "album", "version", "original", "mix", "clean", "short", "the"];

/// Splits a title into its name and version annotation, after dropping noise:
/// "Money Folder (Four Tet Remix)" → ("Money Folder", Some("Four Tet Remix")).
pub fn split_version(title: &str) -> (String, Option<String>) {
    let clean = strip_noise(title);
    let is_version = |s: &str| tokens(s).iter().any(|t| VERSION_MARKERS.contains(&t.as_str()));

    if let Some(open) = clean.rfind(['(', '[']).filter(|_| clean.ends_with([')', ']'])) {
        let (name, inner) = (clean[..open].trim(), &clean[open + 1..clean.len() - 1]);
        if !name.is_empty() && is_version(inner) {
            return (name.to_string(), Some(inner.trim().to_string()));
        }
    }
    if let Some((name, tail)) = clean.rsplit_once(" - ") {
        if !name.trim().is_empty() && is_version(tail) {
            return (name.trim().to_string(), Some(tail.trim().to_string()));
        }
    }
    (clean, None)
}

/// The words that identify a version, or none for the default cut: "Four Tet
/// Remix" → ["four", "tet", "remix"], "Radio Edit" → [].
pub fn version_words(title: &str) -> Vec<String> {
    let words = split_version(title).1.map(|v| tokens(&v)).unwrap_or_default();
    if words.iter().all(|w| PLAIN_VERSION_WORDS.contains(&w.as_str())) {
        return Vec::new();
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_primary_artist() {
        assert_eq!(primary_artist("Justin Bieber feat. Nicki Minaj"), "Justin Bieber");
        assert_eq!(primary_artist("Jamie Foxx feat. T‐Pain"), "Jamie Foxx");
        assert_eq!(primary_artist("Simon & Garfunkel"), "Simon");
        assert_eq!(primary_artist("Kendrick Lamar"), "Kendrick Lamar");
    }

    #[test]
    fn finds_lead_artist() {
        assert_eq!(lead_artist("Flo Rida feat. T‐Pain"), "Flo Rida");
        assert_eq!(lead_artist("Simon & Garfunkel"), "Simon & Garfunkel");
    }

    #[test]
    fn splits_versions_from_titles() {
        assert_eq!(
            split_version("Money Folder (Four Tet Remix)"),
            ("Money Folder".into(), Some("Four Tet Remix".into()))
        );
        assert_eq!(
            split_version("Blame It (feat. T-Pain) (Club Mix)"),
            ("Blame It".into(), Some("Club Mix".into()))
        );
        assert_eq!(split_version("Bayside - Radio Edit").0, "Bayside");
        assert_eq!(split_version("Hey Baby (Drop It to the Floor)").1, None);
        assert_eq!(split_version("(wave to anchor)").0, "(wave to anchor)");
    }

    #[test]
    fn plain_versions_have_no_version_words() {
        assert!(version_words("Bayside (Radio Edit)").is_empty());
        assert!(version_words("Long Train Runnin'").is_empty());
        assert_eq!(version_words("Long Train Runnin’ (5.1 mix)"), vec!["5", "1", "mix"]);
        assert_eq!(version_words("Rose rouge (Nightmares on Wax ReRub)"), vec!["nightmares", "on", "wax", "rerub"]);
    }

    #[test]
    fn normalizes_unicode_and_punctuation() {
        assert_eq!(normalize("Björk — Hyperballad!"), "bjork hyperballad");
        assert_eq!(normalize("  AC/DC  "), "ac dc");
    }

    #[test]
    fn tokens_ignore_and() {
        assert_eq!(tokens("Rock & Roll"), tokens("Rock and Roll"));
    }

    #[test]
    fn strips_noise_annotations() {
        assert_eq!(strip_noise("Get Lucky (feat. Pharrell Williams)"), "Get Lucky");
        assert_eq!(strip_noise("Come Together - Remastered 2009"), "Come Together");
        assert_eq!(strip_noise("Song [Remastered]"), "Song");
        assert_eq!(strip_noise("One More Time (Radio Edit)"), "One More Time (Radio Edit)");
        assert_eq!(strip_noise("Weird Fishes/Arpeggi"), "Weird Fishes/Arpeggi");
        assert_eq!(strip_noise("Phantasm feat. Saori Koseki"), "Phantasm");
    }
}
