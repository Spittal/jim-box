//! Running command-line tools (yt-dlp) from a windowed app.

use tokio::process::Command;

/// Keeps Windows from opening a console window for a command-line tool
/// started by the app, which has no console of its own to lend it. Does
/// nothing elsewhere.
pub fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}
