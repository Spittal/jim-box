//! Files dragged into the window from Finder or Explorer.
//!
//! Tauri's own file drop would take every drag from the web view, and with
//! them the dragging of tracks and playlists, so it stays off
//! (`dragDropEnabled: false`) and the web view handles the drop. What it
//! can't tell the page is where the dropped files are, so that is asked of
//! the system here.

use std::path::PathBuf;

/// The files and folders of the drag that just ended over the window.
///
/// macOS keeps what was last dragged on a pasteboard of its own, which still
/// holds it when the page hears of the drop.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn dropped_paths() -> Vec<PathBuf> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardNameDrag, NSPasteboardTypeFileURL};
    use objc2_foundation::NSURL;

    let pasteboard = NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag });
    let Some(items) = pasteboard.pasteboardItems() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| item.stringForType(unsafe { NSPasteboardTypeFileURL }))
        // Through NSURL, which also resolves the file reference URLs
        // (`file:///.file/id=…`) Finder may hand over.
        .filter_map(|url| NSURL::URLWithString(&url)?.path())
        .map(|path| PathBuf::from(path.to_string()))
        .collect()
}

/// Elsewhere the paths arrive by themselves, as a `files-dropped` event.
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn dropped_paths() -> Vec<PathBuf> {
    Vec::new()
}

/// Has the window say where dropped files are, as a `files-dropped` event.
///
/// WebView2 gives a page no paths either, but the page can post the dropped
/// files themselves (`postMessageWithAdditionalObjects`), and here they have
/// paths. Such a message isn't text, so the handler that carries Tauri's own
/// messages lets it pass.
#[cfg(windows)]
pub fn watch(window: &tauri::WebviewWindow) {
    use tauri::{Emitter, Manager};
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2File, ICoreWebView2WebMessageReceivedEventArgs2,
    };
    use webview2_com::{take_pwstr, WebMessageReceivedEventHandler};
    use windows_core::{Interface, PWSTR};

    let app = window.app_handle().clone();
    let hooked = window.with_webview(move |webview| unsafe {
        let Ok(core) = webview.controller().CoreWebView2() else {
            return;
        };
        let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args.and_then(|args| args.cast::<ICoreWebView2WebMessageReceivedEventArgs2>().ok())
            else {
                return Ok(());
            };
            // Absent on every message but a drop's.
            let Ok(objects) = args.AdditionalObjects() else {
                return Ok(());
            };
            let mut count = 0;
            objects.Count(&mut count)?;
            let mut paths = Vec::new();
            for index in 0..count {
                let Ok(file) = objects.GetValueAtIndex(index).and_then(|object| object.cast::<ICoreWebView2File>())
                else {
                    continue;
                };
                let mut path = PWSTR::null();
                if file.Path(&mut path).is_ok() {
                    paths.push(PathBuf::from(take_pwstr(path)));
                }
            }
            if !paths.is_empty() {
                let _ = app.emit("files-dropped", paths);
            }
            Ok(())
        }));
        let mut token = 0;
        if let Err(e) = core.add_WebMessageReceived(&handler, &mut token) {
            tracing::warn!("can't hear of dropped files: {e}");
        }
    });
    if let Err(e) = hooked {
        tracing::warn!("can't hear of dropped files: {e}");
    }
}
