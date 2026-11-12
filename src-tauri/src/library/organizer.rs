//! Library layout: renders beets-style path templates (`$albumartist/$album/
//! $track $title`, with `${field}` and `$$` as in beets) and moves files into
//! place.

use deunicode::deunicode_char;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};
use crate::state::{PathTemplates, TrackMetadata};
use crate::text::lead_artist;

/// Fields a template can use, named as in beets.
pub const TEMPLATE_FIELDS: [&str; 7] = ["albumartist", "artist", "album", "year", "track", "disc", "title"];
const MAX_COMPONENT_BYTES: usize = 200;

/// A parsed template segment: literal text and `$field`s.
#[derive(Debug, PartialEq)]
enum Piece<'a> {
    Text(&'a str),
    Field(&'a str),
}

/// Splits one folder/file segment of a template into pieces.
fn parse(segment: &str) -> Result<Vec<Piece<'_>>, String> {
    let mut pieces = Vec::new();
    let mut rest = segment;
    while let Some(dollar) = rest.find('$') {
        if dollar > 0 {
            pieces.push(Piece::Text(&rest[..dollar]));
        }
        let after = &rest[dollar + 1..];
        if let Some(escaped) = after.strip_prefix('$') {
            pieces.push(Piece::Text("$"));
            rest = escaped;
        } else if let Some(braced) = after.strip_prefix('{') {
            let close = braced.find('}').ok_or("has an unclosed ${")?;
            pieces.push(Piece::Field(&braced[..close]));
            rest = &braced[close + 1..];
        } else {
            let len = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(after.len());
            if len == 0 {
                return Err("has a $ without a field name (write $$ for a dollar sign)".into());
            }
            pieces.push(Piece::Field(&after[..len]));
            rest = &after[len..];
        }
    }
    if !rest.is_empty() {
        pieces.push(Piece::Text(rest));
    }
    Ok(pieces)
}

/// Checks a template before it's saved, so bad templates fail in Settings
/// rather than at the end of a download.
pub fn validate_template(template: &str) -> AppResult<()> {
    let invalid = |msg: &str| Err(AppError::Validation(format!("Path template {msg}")));
    if template.trim().is_empty() {
        return invalid("can't be empty");
    }
    if template.starts_with('/') || template.split('/').any(|seg| seg.trim() == "..") {
        return invalid("must be a relative path inside the music folder");
    }
    let mut has_title = false;
    for segment in template.split('/') {
        let pieces = match parse(segment) {
            Ok(pieces) => pieces,
            Err(msg) => return invalid(&msg),
        };
        for piece in pieces {
            if let Piece::Field(name) = piece {
                if !TEMPLATE_FIELDS.contains(&name) {
                    return invalid(&format!("has unknown field ${name}"));
                }
                has_title |= name == "title";
            }
        }
    }
    if !has_title {
        return invalid("must include $title");
    }
    Ok(())
}

/// Converts a template from the app's original `{field}` syntax,
/// writing `$field` where that's unambiguous and `${field}` elsewhere.
pub fn convert_legacy_template(template: &str) -> String {
    let mut out = template.replace('$', "$$");
    for field in TEMPLATE_FIELDS {
        let legacy = format!("{{{field}}}");
        let mut from = 0;
        while let Some(found) = out[from..].find(&legacy) {
            let start = from + found;
            let rest = &out[start + legacy.len()..];
            let glued = rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
            let field = if glued { format!("${{{field}}}") } else { format!("${field}") };
            out.replace_range(start..start + legacy.len(), &field);
            from = start + field.len();
        }
    }
    out
}

/// A track's path relative to the music folder (without extension), from
/// the template beets would use for it: albums, compilations or singletons.
/// Each value is sanitized separately so a "/" in a title can't create folders.
pub fn render(paths: &PathTemplates, meta: &TrackMetadata) -> PathBuf {
    let value = |name: &str| -> String {
        let raw = match name {
            "albumartist" => {
                let artist = meta.primary_album_artist.as_deref().unwrap_or(lead_artist(&meta.album_artist));
                or_unknown(artist, "Unknown Artist")
            }
            "artist" => or_unknown(&meta.artist, "Unknown Artist"),
            "album" => or_unknown(&meta.album, "Unknown Album"),
            "title" => or_unknown(&meta.title, "Unknown Title"),
            "year" => meta.year().unwrap_or_default().to_string(),
            "track" => meta.track_number.map(|n| format!("{n:02}")).unwrap_or_default(),
            "disc" => meta.disc_number.map(|n| format!("{n:02}")).unwrap_or_default(),
            _ => String::new(),
        };
        sanitize(&raw)
    };

    let template = paths.for_track(meta);
    let last = template.split('/').count() - 1;
    template
        .split('/')
        .enumerate()
        .map(|(at, segment)| {
            let rendered: String = parse(segment)
                .unwrap_or_default()
                .into_iter()
                .map(|piece| match piece {
                    Piece::Text(text) => text.to_string(),
                    Piece::Field(name) => value(name),
                })
                .collect();
            let cleaned = clean_component(&rendered);
            // The file's own name can't be nothing: a title of only
            // punctuation ("?", "...") cleans away to that, and the file
            // would take its folder's name, beside the folder.
            if cleaned.is_empty() && at == last { "Unknown Title".to_owned() } else { cleaned }
        })
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn or_unknown(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() { fallback } else { value }.to_string()
}

/// Makes a value safe and easy to type as a file name: characters unsafe on
/// macOS/Windows are replaced, and accented Latin letters and typographic
/// punctuation become plain ASCII ("Noït" → "Noit", "T‐Pain" → "T-Pain").
/// Other scripts (Japanese, Cyrillic, …) are kept as they are.
fn sanitize(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.replace(": ", " - ").chars() {
        match c {
            '/' | '\\' | ':' => out.push('-'),
            '<' | '>' | '"' | '?' | '*' | '|' => {}
            c if c.is_control() => {}
            c if is_latin_or_punctuation(c) => out.push_str(deunicode_char(c).unwrap_or("")),
            c => out.push(c),
        }
    }
    out
}

/// Latin-1, Latin Extended and general punctuation: the ranges where ASCII
/// folding keeps a name readable.
fn is_latin_or_punctuation(c: char) -> bool {
    matches!(c as u32, 0x80..=0x24F | 0x1E00..=0x1EFF | 0x2000..=0x206F)
}

/// Drops brackets left empty by a missing value (e.g. `({year})`), collapses
/// whitespace, strips edge dots/dashes/spaces and caps the component length.
fn clean_component(component: &str) -> String {
    let unbracketed = component.replace("()", "").replace("[]", "");
    let collapsed = unbracketed.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut trimmed = collapsed.trim_matches(|c: char| c == ' ' || c == '.' || c == '-').to_string();
    if trimmed.len() > MAX_COMPONENT_BYTES {
        let mut end = MAX_COMPONENT_BYTES;
        while !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        trimmed.truncate(end);
        // Windows drops a trailing dot or space from a name, so the folder
        // wouldn't be where this says it is.
        trimmed.truncate(trimmed.trim_end_matches([' ', '.']).len());
    }
    if cfg!(windows) && is_reserved_on_windows(&trimmed) {
        trimmed.push('_');
    }
    trimmed
}

/// Names Windows keeps for devices and won't make a file or folder with,
/// whatever follows the first dot: "CON", "aux.flac", "COM1"…
fn is_reserved_on_windows(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.ends_with(|c: char| c.is_ascii_digit() && c != '0'))
}

/// Absolute library path for a rendered template path plus extension. The
/// extension is appended rather than set, since titles may contain dots.
pub fn library_path(music_dir: &Path, relative: &Path, ext: &str) -> PathBuf {
    let mut path = music_dir.join(relative).into_os_string();
    path.push(format!(".{ext}"));
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind of track filed by one template.
    fn only(template: &str) -> PathTemplates {
        PathTemplates { default: template.into(), comp: template.into(), singleton: template.into() }
    }

    fn meta() -> TrackMetadata {
        TrackMetadata {
            title: "Weird Fishes/Arpeggi".into(),
            artist: "Radiohead".into(),
            album: "In Rainbows".into(),
            album_artist: "Radiohead".into(),
            track_number: Some(4),
            date: Some("2007-10-10".into()),
            ..Default::default()
        }
    }

    #[test]
    fn renders_default_template() {
        assert_eq!(
            render(&PathTemplates::default(), &meta()),
            PathBuf::from("Radiohead/In Rainbows/04 Weird Fishes-Arpeggi")
        );
    }

    #[test]
    fn appends_extension_to_dotted_titles() {
        assert_eq!(
            library_path(Path::new("/m"), Path::new("A/B/01 Mr. Brightside"), "flac"),
            PathBuf::from("/m/A/B/01 Mr. Brightside.flac")
        );
    }

    #[test]
    fn renders_optional_fields_cleanly() {
        let mut m = meta();
        m.track_number = None;
        m.album = String::new();
        assert_eq!(
            render(&only("$albumartist/$album ($year)/$track - $title"), &m),
            PathBuf::from("Radiohead/Unknown Album (2007)/Weird Fishes-Arpeggi")
        );
        m.date = None;
        assert_eq!(
            render(&only("$albumartist/$album ($year)/$title"), &m),
            PathBuf::from("Radiohead/Unknown Album/Weird Fishes-Arpeggi")
        );
    }

    #[test]
    fn sanitizes_unsafe_characters() {
        let mut m = meta();
        m.title = "What's Going On?: Live*".into();
        assert_eq!(render(&only("$title"), &m), PathBuf::from("What's Going On - Live"));
    }

    #[test]
    fn folds_latin_text_to_ascii() {
        let mut m = meta();
        m.album_artist = "Flo Rida feat. T‐Pain".into();
        m.album = "Extinction Level Event: The Final World Front".into();
        m.title = "What’s Golden — Noït".into();
        assert_eq!(
            render(&PathTemplates::default(), &m),
            PathBuf::from("Flo Rida/Extinction Level Event - The Final World Front/04 What's Golden -- Noit")
        );
        m.title = "東京".into();
        assert_eq!(render(&only("$title"), &m), PathBuf::from("東京"));
    }

    #[test]
    fn prefers_primary_album_artist_for_folders() {
        let mut m = meta();
        m.album_artist = "Fred again.., Skrillex & Four Tet".into();
        m.primary_album_artist = Some("Fred again..".into());
        assert_eq!(render(&only("$albumartist/$title"), &m), PathBuf::from("Fred again/Weird Fishes-Arpeggi"));
    }

    #[test]
    fn validates_templates() {
        let defaults = PathTemplates::default();
        for template in [&defaults.default, &defaults.comp, &defaults.singleton] {
            assert!(validate_template(template).is_ok(), "{template}");
        }
        assert!(validate_template("${albumartist}/$$5 $title").is_ok());
        assert!(validate_template("$artist/$album").is_err());
        assert!(validate_template("../$title").is_err());
        assert!(validate_template("/abs/$title").is_err());
        assert!(validate_template("$bogus/$title").is_err());
        assert!(validate_template("${title").is_err());
        assert!(validate_template("$ $title").is_err());
    }

    #[test]
    fn converts_legacy_templates() {
        assert_eq!(convert_legacy_template("{albumartist}/{album}/{track} {title}"), "$albumartist/$album/$track $title");
        assert_eq!(convert_legacy_template("{track}_x {title} $5"), "${track}_x $title $$5");
    }

    #[test]
    fn uses_the_compilation_and_singleton_templates() {
        let mut m = meta();
        m.compilation = true;
        m.album_artist = "Various Artists".into();
        assert_eq!(render(&PathTemplates::default(), &m), PathBuf::from("Compilations/In Rainbows/04 Weird Fishes-Arpeggi"));
        m.album = String::new();
        assert_eq!(render(&PathTemplates::default(), &m), PathBuf::from("Non-Album/Radiohead/Weird Fishes-Arpeggi"));
    }
}
