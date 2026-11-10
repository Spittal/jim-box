//! The app used to be called Track Find, under the identifier
//! `com.trackfind.app`. These carry what that build saved over to this one's
//! folders and Keychain service, once, so the rename doesn't look like a
//! fresh install.

use std::path::PathBuf;

use tracing::{info, warn};

const OLD_IDENTIFIER: &str = "com.trackfind.app";
pub const OLD_KEYCHAIN_SERVICE: &str = "com.trackfind.app.soulseek";

/// Moves the old build's data folders to this identifier's. Runs before Tauri
/// and its plugins open anything in them. A folder is only moved when this
/// build hasn't made its own yet.
pub fn move_data_dirs(identifier: &str) {
    if identifier == OLD_IDENTIFIER {
        return;
    }
    let Some(library) = library_dir() else { return };
    // Settings, slskd's state and rekordbox backups; then the webview's storage.
    for parent in ["Application Support", "WebKit"] {
        let (old, new) = (library.join(parent).join(OLD_IDENTIFIER), library.join(parent).join(identifier));
        if !old.is_dir() || new.exists() {
            continue;
        }
        // Logging isn't set up this early, so these go to the console only.
        match std::fs::rename(&old, &new) {
            Ok(()) => println!("Moved {} to {}", old.display(), new.display()),
            Err(e) => eprintln!("Couldn't move {} to {}: {e}", old.display(), new.display()),
        }
    }
}

#[cfg(target_os = "macos")]
fn library_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Library"))
}

#[cfg(not(target_os = "macos"))]
fn library_dir() -> Option<PathBuf> {
    None
}

/// The Soulseek password the old build saved in the Keychain, copied to
/// `service` so it's found there from now on.
pub fn adopt_old_password(service: &str, username: &str) -> Option<String> {
    let password = keyring::Entry::new(OLD_KEYCHAIN_SERVICE, username).ok()?.get_password().ok()?;
    match keyring::Entry::new(service, username).and_then(|entry| entry.set_password(&password)) {
        Ok(()) => info!("Copied the Soulseek password saved by Track Find"),
        Err(e) => warn!("Couldn't copy the Soulseek password saved by Track Find: {e}"),
    }
    Some(password)
}
