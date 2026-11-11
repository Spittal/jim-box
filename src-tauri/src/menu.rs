//! The macOS menu bar, and the window behaviour that goes with it. Windows
//! has no menu bar: the tray icon (see `tray.rs`) and the window's own
//! shortcuts (see `App.vue`) do its job there.
//!
//! Items that change what the window shows are forwarded to the frontend as a
//! `menu` event carrying the item id (see `MenuCommand`); the rest are handled
//! here.

use tauri::menu::{AboutMetadataBuilder, Menu, MenuBuilder, MenuEvent, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow};
use tauri_plugin_opener::OpenerExt;
use tracing::warn;

/// Menu ids forwarded to the frontend, which owns navigation.
const FRONTEND_COMMANDS: &[&str] = &["settings", "find", "find-online", "consolidate"];

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let pkg = app.package_info();
    let about = AboutMetadataBuilder::new()
        .name(Some(pkg.name.clone()))
        .version(Some(pkg.version.to_string()))
        .copyright(app.config().bundle.copyright.clone())
        .comments(Some(
            "Manages your rekordbox library, and finds lossless copies of tracks for it.\n\
             Soulseek access by soulseek-rs."
                .to_string(),
        ))
        .build();

    let item = |id: &str, text: &str, accel: &str| {
        MenuItemBuilder::with_id(id, text).accelerator(accel).build(app)
    };

    let app_menu = SubmenuBuilder::new(app, &pkg.name)
        .about(Some(about))
        .separator()
        .item(&item("settings", "Settings…", "CmdOrCtrl+,")?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let file_menu = SubmenuBuilder::new(app, "File")
        .item(&item("find", "Find…", "CmdOrCtrl+F")?)
        .item(&item("find-online", "Find Online", "CmdOrCtrl+Shift+F")?)
        .separator()
        .item(&MenuItemBuilder::with_id("consolidate", "Consolidate Library…").build(app)?)
        .separator()
        .close_window()
        .build()?;

    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let view_menu = SubmenuBuilder::new(app, "View").fullscreen().build()?;

    let window_menu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize_with_text("Zoom")
        .separator()
        .item(&item("show-main", "jim-box", "CmdOrCtrl+0")?)
        .separator()
        .bring_all_to_front()
        .build()?;

    let help_menu = SubmenuBuilder::new(app, "Help")
        .item(&MenuItemBuilder::with_id("show-logs", "Show Logs in Finder").build(app)?)
        .build()?;

    #[cfg(target_os = "macos")]
    {
        window_menu.set_as_windows_menu_for_nsapp()?;
        help_menu.set_as_help_menu_for_nsapp()?;
    }

    MenuBuilder::new(app)
        .items(&[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu, &help_menu])
        .build()
}

pub fn handle_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let id = event.id().as_ref();
    match id {
        "show-main" => show_main(app),
        // Disconnects from Soulseek on the way out (see `RunEvent::Exit`).
        "quit" => app.exit(0),
        "show-logs" => match app.path().app_log_dir() {
            Ok(dir) => {
                if let Err(e) = app.opener().reveal_item_in_dir(dir.join("app.log")) {
                    warn!("Couldn't reveal the log file: {e}");
                }
            }
            Err(e) => warn!("No log folder: {e}"),
        },
        id if FRONTEND_COMMANDS.contains(&id) => {
            show_main(app);
            let _ = app.emit_to("main", "menu", id);
        }
        _ => {}
    }
}

pub fn main_window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window("main")
}

/// Brings the main window back: after it was closed (hidden), minimized, or
/// is behind another app's windows.
pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = main_window(app) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
