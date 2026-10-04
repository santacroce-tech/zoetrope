//! The native menu bar. Items carry ids that are forwarded to the frontend
//! as `menu` events (App.tsx's `menuActions`), except Cut, Copy and Paste,
//! which stay native so text fields and the editor's clipboard handling both
//! receive them as ordinary DOM clipboard events.
//!
//! Accelerators duplicate the editor's keyboard shortcuts. Depending on the
//! platform and focus, a key may reach the webview, the menu, or both; the
//! frontend ignores a menu event that arrives right after the same key was
//! handled as a keydown (see `MENU_KEYS` in App.tsx).

use tauri::menu::{AboutMetadata, Menu, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Wry};

pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let item = |id: &str, text: &str, accel: Option<&str>| {
        let b = MenuItemBuilder::with_id(id, text);
        match accel {
            Some(a) => b.accelerator(a).build(app),
            None => b.build(app),
        }
    };

    let about = AboutMetadata {
        name: Some("Zoetrope".into()),
        version: Some(app.package_info().version.to_string()),
        copyright: Some("Open source under the MIT license".into()),
        website: Some("https://santacroce-tech.github.io/zoetrope/".into()),
        website_label: Some("santacroce-tech.github.io/zoetrope".into()),
        ..Default::default()
    };

    let file = SubmenuBuilder::new(app, "File")
        .item(&item("new", "New Blank Project", Some("CmdOrCtrl+N"))?)
        .item(&item("new-animation", "New from Animation Demo", None)?)
        .item(&item("new-game", "New from Game Demo", None)?)
        .separator()
        .item(&item("open", "Open…", Some("CmdOrCtrl+O"))?)
        .separator()
        .item(&item("save", "Save", Some("CmdOrCtrl+S"))?)
        .item(&item("save-as", "Save As…", Some("CmdOrCtrl+Shift+S"))?)
        .separator()
        .item(&item("import", "Import Images…", Some("CmdOrCtrl+I"))?)
        .item(&item("export", "Export…", Some("CmdOrCtrl+Shift+E"))?)
        .separator()
        .item(&item("close", "Close Window", Some("CmdOrCtrl+W"))?);
    #[cfg(not(target_os = "macos"))]
    let file = file.separator().item(&item("preferences", "Preferences…", Some("CmdOrCtrl+Comma"))?).separator().item(&item(
        "quit",
        "Quit",
        Some("CmdOrCtrl+Q"),
    )?);

    let edit = SubmenuBuilder::new(app, "Edit")
        .item(&item("undo", "Undo", Some("CmdOrCtrl+Z"))?)
        .item(&item("redo", "Redo", Some("CmdOrCtrl+Shift+Z"))?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .item(&item("paste-in-place", "Paste in Place", Some("CmdOrCtrl+Shift+V"))?)
        .item(&item("duplicate", "Duplicate", Some("CmdOrCtrl+D"))?)
        .item(&item("delete", "Delete", None)?)
        .separator()
        .item(&item("select-all", "Select All", Some("CmdOrCtrl+A"))?)
        .item(&item("deselect", "Deselect", None)?)
        .build()?;

    let view = SubmenuBuilder::new(app, "View")
        .item(&item("zoom-in", "Zoom In", Some("CmdOrCtrl+Equal"))?)
        .item(&item("zoom-out", "Zoom Out", Some("CmdOrCtrl+Minus"))?)
        .item(&item("zoom-fit", "Fit Stage", Some("CmdOrCtrl+0"))?)
        .item(&item("zoom-100", "Actual Size", Some("CmdOrCtrl+1"))?)
        .separator()
        .item(&item("toggle-grid", "Grid", Some("CmdOrCtrl+Quote"))?)
        .item(&item("toggle-snap-grid", "Snap to Grid", Some("CmdOrCtrl+Shift+Quote"))?)
        .item(&item("toggle-rulers", "Rulers", None)?)
        .item(&item("toggle-guides", "Guide Layers", None)?)
        .separator()
        .item(&item("toggle-output", "Output Panel", None)?);
    #[cfg(target_os = "macos")]
    let view = view.separator().item(&PredefinedMenuItem::fullscreen(app, None)?);
    let view = view.build()?;

    let modify = SubmenuBuilder::new(app, "Modify")
        .item(&item("convert-to-symbol", "Convert to Symbol…", Some("F8"))?)
        .item(&item("edit-symbol", "Edit Symbol in Place", Some("CmdOrCtrl+E"))?)
        .item(&item("convert-to-path", "Convert to Path", Some("CmdOrCtrl+B"))?)
        .separator()
        .item(&item("bring-to-front", "Bring to Front", Some("CmdOrCtrl+Shift+BracketRight"))?)
        .item(&item("bring-forward", "Bring Forward", Some("CmdOrCtrl+BracketRight"))?)
        .item(&item("send-backward", "Send Backward", Some("CmdOrCtrl+BracketLeft"))?)
        .item(&item("send-to-back", "Send to Back", Some("CmdOrCtrl+Shift+BracketLeft"))?)
        .build()?;

    let control = SubmenuBuilder::new(app, "Control")
        .item(&item("play", "Play / Pause (Enter)", None)?)
        .item(&item("first-frame", "First Frame (Home)", None)?)
        .item(&item("prev-frame", "Previous Frame (,)", None)?)
        .item(&item("next-frame", "Next Frame (.)", None)?)
        .item(&item("last-frame", "Last Frame (End)", None)?)
        .separator()
        .item(&item("insert-frame", "Insert Frame", Some("F5"))?)
        .item(&item("insert-keyframe", "Insert Keyframe", Some("F6"))?)
        .item(&item("insert-blank-keyframe", "Insert Blank Keyframe", Some("F7"))?)
        .build()?;

    let help = SubmenuBuilder::new(app, "Help")
        .item(&item("help-manual", "Zoetrope Manual", None)?)
        .item(&item("help-shortcuts", "Keyboard Shortcuts (?)", None)?)
        .item(&item("help-scripting", "Scripting Reference", None)?)
        .separator()
        .item(&item("help-issue", "Report a Problem…", None)?);
    #[cfg(not(target_os = "macos"))]
    let help = help.separator().item(&PredefinedMenuItem::about(app, Some("About Zoetrope"), Some(about.clone()))?);
    let help = help.build()?;

    #[cfg(target_os = "macos")]
    {
        let app_menu = SubmenuBuilder::new(app, "Zoetrope")
            .item(&PredefinedMenuItem::about(app, Some("About Zoetrope"), Some(about))?)
            .separator()
            .item(&item("preferences", "Preferences…", Some("CmdOrCtrl+Comma"))?)
            .separator()
            .item(&PredefinedMenuItem::services(app, None)?)
            .separator()
            .item(&PredefinedMenuItem::hide(app, None)?)
            .item(&PredefinedMenuItem::hide_others(app, None)?)
            .item(&PredefinedMenuItem::show_all(app, None)?)
            .separator()
            .item(&item("quit", "Quit Zoetrope", Some("CmdOrCtrl+Q"))?)
            .build()?;
        let window = SubmenuBuilder::new(app, "Window")
            .item(&PredefinedMenuItem::minimize(app, None)?)
            .item(&PredefinedMenuItem::maximize(app, Some("Zoom"))?)
            .build()?;
        Menu::with_items(app, &[&app_menu, &file.build()?, &edit, &view, &modify, &control, &window, &help])
    }
    #[cfg(not(target_os = "macos"))]
    {
        Menu::with_items(app, &[&file.build()?, &edit, &view, &modify, &control, &help])
    }
}
