//! Filesystem helpers for the library: finding audio, moving files safely.

use std::path::{Path, PathBuf};

use crate::error::AppResult;

/// Extensions the library scanner picks up (anything lofty can tag).
pub const AUDIO_EXTENSIONS: [&str; 9] = ["flac", "mp3", "m4a", "aif", "aiff", "wav", "ogg", "opus", "aac"];

/// Files macOS and Windows litter folders with; safe to delete when tidying up.
const JUNK_FILES: [&str; 4] = [".DS_Store", "._.DS_Store", "Thumbs.db", "desktop.ini"];

/// A file's extension, lowercased; empty when it has none.
pub fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

/// Every file under `root`, depth-first, skipping hidden files and folders.
pub fn walk_files(root: &Path) -> AppResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if is_hidden(&path) {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// `path`, or `path` with " (2)", " (3)"… before the extension if it's taken.
pub fn unique_destination(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|candidate| !candidate.exists())
        .expect("unbounded range always yields a free name")
}

/// Moves a file, creating parent folders. Never overwrites: returns the
/// actual destination, which may carry a " (n)" suffix. Works across volumes.
pub fn move_file(src: &Path, dest: PathBuf) -> AppResult<PathBuf> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dest = unique_destination(dest);
    if std::fs::rename(src, &dest).is_err() {
        // Different volume: copy, then remove the original only once the copy succeeded.
        std::fs::copy(src, &dest)?;
        std::fs::remove_file(src)?;
    }
    Ok(dest)
}

/// Removes `dir` and its ancestors while they're empty (ignoring Finder junk),
/// stopping at `stop_at`, which is never removed.
pub fn remove_empty_dirs(dir: &Path, stop_at: &Path) {
    let mut current = Some(dir);
    while let Some(dir) = current {
        if dir == stop_at || !dir.starts_with(stop_at) || !remove_if_empty(dir) {
            break;
        }
        current = dir.parent();
    }
}

/// Deletes a folder that contains nothing but Finder junk. Returns whether it was removed.
pub fn remove_if_empty(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    let entries: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    let only_junk = entries.iter().all(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| JUNK_FILES.contains(&n))
    });
    if !only_junk {
        return false;
    }
    entries.iter().for_each(|p| {
        let _ = std::fs::remove_file(p);
    });
    std::fs::remove_dir(dir).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jim-box-test-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn move_file_never_overwrites() {
        let root = temp_dir("move");
        let a = root.join("a.flac");
        let b = root.join("b.flac");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();

        let dest = move_file(&a, root.join("sub").join("x.flac")).unwrap();
        assert_eq!(dest, root.join("sub/x.flac"));
        let dest2 = move_file(&b, root.join("sub").join("x.flac")).unwrap();
        assert_eq!(dest2, root.join("sub/x (2).flac"));
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "a");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removes_empty_dirs_up_to_root() {
        let root = temp_dir("prune");
        let deep = root.join("Artist/Album");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join(".DS_Store"), "").unwrap();
        std::fs::create_dir_all(root.join("Keep")).unwrap();
        std::fs::write(root.join("Keep/song.mp3"), "").unwrap();

        remove_empty_dirs(&deep, &root);
        assert!(!root.join("Artist").exists());
        assert!(root.join("Keep/song.mp3").exists());
        assert!(root.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn walks_files_skipping_hidden() {
        let root = temp_dir("walk");
        std::fs::create_dir_all(root.join("A/.hidden")).unwrap();
        std::fs::write(root.join("A/1.flac"), "").unwrap();
        std::fs::write(root.join("A/.hidden/2.flac"), "").unwrap();
        std::fs::write(root.join(".DS_Store"), "").unwrap();
        assert_eq!(walk_files(&root).unwrap(), vec![root.join("A/1.flac")]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
