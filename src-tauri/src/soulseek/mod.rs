mod client;
mod session;
pub mod types;

pub use client::{can_save, SoulseekClient, UNSAVEABLE_NAME};
pub use session::{staging_dir, Soulseek, SoulseekStatus};

/// Lets the process hold the sockets a busy session needs: every peer that
/// answers a search is one, and apps start with room for only a few hundred
/// open files.
pub fn raise_open_file_limit() {
    #[cfg(unix)]
    {
        const WANTED: libc::rlim_t = 4096;
        // SAFETY: both calls only read or write the struct they're handed.
        unsafe {
            let mut limit = std::mem::zeroed::<libc::rlimit>();
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 || limit.rlim_cur >= WANTED {
                return;
            }
            limit.rlim_cur = WANTED.min(limit.rlim_max);
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                tracing::warn!("Couldn't raise the open file limit: {}", std::io::Error::last_os_error());
            }
        }
    }
}
