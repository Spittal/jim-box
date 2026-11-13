//! The notification-area icon on Windows, which stands in for the macOS Dock
//! icon and menu bar: it brings the window back after it was closed, and is
//! where the app is quit.

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

use crate::menu;

pub const ID: &str = "main";

/// Adds the icon. Its menu items are handled with the menu bar's, in
/// [`menu::handle_event`].
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let item = |id: &str, text: &str| MenuItemBuilder::with_id(id, text).build(app);
    let tray_menu = MenuBuilder::new(app)
        .item(&item("show-main", "Open Jim Box")?)
        .item(&item("show-logs", "Show Logs")?)
        .separator()
        .item(&item("quit", "Quit Jim Box")?)
        .build()?;

    let mut tray = TrayIconBuilder::with_id(ID)
        .tooltip("Jim Box")
        .menu(&tray_menu)
        // A click opens the window; the menu is for the right button.
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                menu::show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Whether the icon is there to bring a hidden window back.
pub fn exists(app: &AppHandle) -> bool {
    app.tray_by_id(ID).is_some()
}
