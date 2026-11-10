//! Soulseek password storage in the OS keychain (the Keychain on macOS,
//! Credential Manager on Windows). The password never touches
//! disk in plain text; it's handed to slskd through its environment at launch.
//!
//! Dev builds are the exception: they're re-signed on every rebuild, so macOS
//! asks for the login password each launch to read the Keychain. They keep the
//! password in a plain file under `target/` instead (see [`dev_file`]).

use keyring::Entry;

use crate::error::AppResult;

const SERVICE: &str = "com.jimbox.app.soulseek";

pub fn get_password(username: &str) -> AppResult<Option<String>> {
    #[cfg(debug_assertions)]
    if let Some(password) = dev_file::get(username)? {
        return Ok(Some(password));
    }

    let password = match Entry::new(SERVICE, username)?.get_password() {
        Ok(password) => Some(password),
        Err(keyring::Error::NoEntry) => crate::rename::adopt_old_password(SERVICE, username),
        Err(e) => return Err(e.into()),
    };

    // Copy a password saved by an earlier build, so this is the last prompt.
    #[cfg(debug_assertions)]
    if let Some(password) = &password {
        dev_file::set(username, password)?;
    }
    Ok(password)
}

pub fn set_password(username: &str, password: &str) -> AppResult<()> {
    #[cfg(debug_assertions)]
    return dev_file::set(username, password);

    #[cfg(not(debug_assertions))]
    {
        Entry::new(SERVICE, username)?.set_password(password)?;
        Ok(())
    }
}

pub fn delete_password(username: &str) -> AppResult<()> {
    #[cfg(debug_assertions)]
    dev_file::delete(username)?;

    // The old build's copy too, or it would be adopted again on the next read.
    for service in [SERVICE, crate::rename::OLD_KEYCHAIN_SERVICE] {
        match Entry::new(service, username)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Plain-text password store for dev builds: a JSON map of username to
/// password in `src-tauri/target/`, which is gitignored and never shipped.
#[cfg(debug_assertions)]
mod dev_file {
    use std::collections::HashMap;

    use crate::error::AppResult;

    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/target/dev-secrets.json");

    fn load() -> AppResult<HashMap<String, String>> {
        match std::fs::read_to_string(PATH) {
            Ok(text) => Ok(serde_json::from_str(&text).unwrap_or_default()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    fn save(entries: &HashMap<String, String>) -> AppResult<()> {
        let json = serde_json::to_string_pretty(entries).expect("string map serializes");
        std::fs::write(PATH, json)?;
        // Readable by this user only. (Windows keeps `target/` under the
        // user's profile, which other accounts can't read anyway.)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(std::path::Path::new(PATH), std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn get(username: &str) -> AppResult<Option<String>> {
        Ok(load()?.remove(username))
    }

    pub fn set(username: &str, password: &str) -> AppResult<()> {
        let mut entries = load()?;
        entries.insert(username.to_string(), password.to_string());
        save(&entries)
    }

    pub fn delete(username: &str) -> AppResult<()> {
        let mut entries = load()?;
        if entries.remove(username).is_some() {
            save(&entries)?;
        }
        Ok(())
    }
}
