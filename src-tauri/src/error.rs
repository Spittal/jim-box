use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Validation(String),

    #[error("Soulseek is not connected yet")]
    DaemonNotReady,

    #[error("slskd: {0}")]
    Slskd(String),

    #[error("YouTube: {0}")]
    Youtube(String),

    #[error("Tagging failed: {0}")]
    Tagging(String),

    #[error("rekordbox: {0}")]
    Rekordbox(String),

    #[error("Download not found: {0}")]
    JobNotFound(Uuid),

    #[error("Keychain error: {0}")]
    Keychain(#[from] keyring::Error),

    #[error("Network error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Tauri error: {0}")]
    Tauri(#[from] tauri::Error),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        serializer.serialize_str(self.to_string().as_ref())
    }
}

pub type AppResult<T> = Result<T, AppError>;
