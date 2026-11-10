//! Reads and writes tags (and front cover) with lofty.

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::picture::{Picture, PictureType};
use lofty::tag::{Accessor, ItemKey, Tag};
use std::path::Path;

use crate::error::{AppError, AppResult};
use crate::state::{ExistingTags, TrackMetadata};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagMode {
    /// Start from an empty tag. Used for downloads: peer-supplied tags are
    /// often wrong or full of junk.
    Replace,
    /// Overwrite only the fields we know, keeping everything else (lyrics,
    /// comments, ReplayGain…). Used for files already in the library.
    Merge,
}

/// Audio properties and existing tags of a file on disk.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    /// Short format label, e.g. "FLAC", "MP3".
    pub format: String,
    pub lossless: bool,
    pub duration_secs: Option<u32>,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<u32>,
    /// Kilobits per second.
    pub bitrate: Option<u32>,
    pub tags: ExistingTags,
}

fn err(e: impl std::fmt::Display) -> AppError {
    AppError::Tagging(e.to_string())
}

pub fn read_file(path: &Path) -> AppResult<ScannedFile> {
    let file = lofty::read_from_path(path).map_err(err)?;
    let props = file.properties();
    let bit_depth = props.bit_depth().map(u32::from);

    let (format, lossless) = match file.file_type() {
        FileType::Flac => ("FLAC", true),
        FileType::Aiff => ("AIFF", true),
        FileType::Wav => ("WAV", true),
        FileType::WavPack => ("WavPack", true),
        FileType::Ape => ("APE", true),
        // ALAC reports a bit depth; AAC doesn't.
        FileType::Mp4 if bit_depth.is_some() => ("ALAC", true),
        FileType::Mp4 => ("AAC", false),
        FileType::Mpeg => ("MP3", false),
        FileType::Aac => ("AAC", false),
        FileType::Vorbis => ("Ogg Vorbis", false),
        FileType::Opus => ("Opus", false),
        _ => ("Audio", false),
    };

    let tags = file
        .primary_tag()
        .or_else(|| file.first_tag())
        .map(existing_tags)
        .unwrap_or_default();

    let duration = props.duration().as_secs_f64().round() as u32;
    Ok(ScannedFile {
        format: format.to_string(),
        lossless,
        duration_secs: (duration > 0).then_some(duration),
        bit_depth,
        sample_rate: props.sample_rate(),
        bitrate: props.audio_bitrate(),
        tags,
    })
}

/// The file's embedded front cover (or, failing that, any picture), as image bytes.
pub fn read_cover(path: &Path) -> AppResult<Option<Vec<u8>>> {
    let file = lofty::read_from_path(path).map_err(err)?;
    let pictures = file.tags().iter().flat_map(|tag| tag.pictures());
    let pictures: Vec<&Picture> = pictures.collect();
    let picture = pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first());
    Ok(picture.map(|p| p.data().to_vec()))
}

fn existing_tags(tag: &Tag) -> ExistingTags {
    let text = |key: ItemKey| {
        tag.get_string(key)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    ExistingTags {
        title: text(ItemKey::TrackTitle),
        artist: text(ItemKey::TrackArtist),
        album: text(ItemKey::AlbumTitle),
        album_artist: text(ItemKey::AlbumArtist),
        track_number: tag.track(),
        disc_number: tag.disk(),
        date: text(ItemKey::RecordingDate).or_else(|| text(ItemKey::Year)),
        isrc: text(ItemKey::Isrc),
        mb_recording_id: text(ItemKey::MusicBrainzRecordingId),
        mb_release_id: text(ItemKey::MusicBrainzReleaseId),
    }
}

/// Writes `meta` (and optionally a front cover) into the file's primary tag.
pub fn write_tags(path: &Path, meta: &TrackMetadata, cover: Option<&[u8]>, mode: TagMode) -> AppResult<()> {
    let mut file = lofty::read_from_path(path).map_err(err)?;
    let tag_type = file.primary_tag_type();
    let mut tag = match mode {
        TagMode::Merge => file.primary_tag().cloned().unwrap_or_else(|| Tag::new(tag_type)),
        TagMode::Replace => Tag::new(tag_type),
    };

    tag.set_title(meta.title.clone());
    tag.set_artist(meta.artist.clone());
    tag.set_album(meta.album.clone());
    tag.insert_text(ItemKey::AlbumArtist, meta.album_artist.clone());
    if meta.compilation {
        // Beets' `comp`: players group the album instead of splitting it by artist.
        tag.insert_text(ItemKey::FlagCompilation, "1".into());
    }
    if let Some(n) = meta.track_number {
        tag.set_track(n);
    }
    if let Some(n) = meta.track_total {
        tag.set_track_total(n);
    }
    if let Some(n) = meta.disc_number {
        tag.set_disk(n);
    }

    let optional = [
        (ItemKey::RecordingDate, &meta.date),
        (ItemKey::Isrc, &meta.isrc),
        (ItemKey::MusicBrainzRecordingId, &meta.mb_recording_id),
        (ItemKey::MusicBrainzReleaseId, &meta.mb_release_id),
        (ItemKey::MusicBrainzReleaseGroupId, &meta.mb_release_group_id),
        (ItemKey::MusicBrainzArtistId, &meta.mb_artist_id),
        (ItemKey::MusicBrainzReleaseArtistId, &meta.mb_album_artist_id),
    ];
    for (key, value) in optional {
        if let Some(value) = value {
            tag.insert_text(key, value.clone());
        }
    }

    if let Some(bytes) = cover {
        match Picture::from_reader(&mut &bytes[..]) {
            Ok(mut picture) => {
                picture.set_pic_type(PictureType::CoverFront);
                tag.remove_picture_type(PictureType::CoverFront);
                tag.push_picture(picture);
            }
            Err(e) => tracing::warn!("Skipping unreadable cover art: {e}"),
        }
    }

    file.insert_tag(tag);
    file.save_to_path(path, WriteOptions::default()).map_err(err)?;
    Ok(())
}
