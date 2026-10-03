//! Native shell. Boundary contract (see docs/ARCHITECTURE.md): this side does
//! OS dialogs and filesystem I/O ONLY. It never parses or interprets project
//! contents — those are opaque strings owned by the WASM core.

use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::{DialogExt, FilePath};

const EXTENSION: &str = "zoe";

#[derive(Serialize)]
struct OpenedFile {
    path: String,
    contents: String,
}

fn to_path(fp: FilePath) -> Result<PathBuf, String> {
    fp.into_path().map_err(|e| e.to_string())
}

/// Writes via a temp file + rename so a crash mid-write never truncates the
/// user's existing project.
fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let tmp = path.with_extension(format!("{EXTENSION}.tmp"));
    std::fs::write(&tmp, contents).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("could not replace {}: {e}", path.display()))
}

/// Saves `contents` to `path`, or asks for a path when `path` is null.
/// Returns the path written, or null if the user cancelled.
#[tauri::command]
async fn save_project(app: AppHandle, contents: String, path: Option<String>) -> Result<Option<String>, String> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => {
            let picked = app
                .dialog()
                .file()
                .add_filter("Zoetrope Project", &[EXTENSION])
                .set_file_name(format!("Untitled.{EXTENSION}"))
                .blocking_save_file();
            match picked {
                Some(fp) => to_path(fp)?,
                None => return Ok(None),
            }
        }
    };
    write_atomic(&path, &contents)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Shows an open dialog and returns the chosen file, or null if cancelled.
#[tauri::command]
async fn open_project(app: AppHandle) -> Result<Option<OpenedFile>, String> {
    let picked = app.dialog().file().add_filter("Zoetrope Project", &[EXTENSION]).blocking_pick_file();
    let Some(fp) = picked else { return Ok(None) };
    let path = to_path(fp)?;
    let contents = std::fs::read_to_string(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    Ok(Some(OpenedFile { path: path.to_string_lossy().into_owned(), contents }))
}

/// Paths the user picked in an import dialog and that the frontend may read
/// once. The frontend can never read an arbitrary path.
#[derive(Default)]
struct PickedFiles(Mutex<HashSet<PathBuf>>);

#[derive(Serialize)]
struct PickedFile {
    path: String,
    name: String,
}

/// Shows a multi-select dialog for `kind` ("image" | "font" | "audio").
/// Returns the picked files (possibly empty).
#[tauri::command]
async fn pick_files(app: AppHandle, kind: String, picked: State<'_, PickedFiles>) -> Result<Vec<PickedFile>, String> {
    let (label, extensions): (&str, &[&str]) = match kind.as_str() {
        "image" => ("Images", &["png", "jpg", "jpeg", "gif"]),
        "font" => ("Fonts", &["ttf", "otf"]),
        "audio" => ("Audio", &["mp3", "wav", "m4a", "aac", "ogg", "flac"]),
        _ => return Err(format!("unknown file kind {kind:?}")),
    };
    let files = app.dialog().file().add_filter(label, extensions).blocking_pick_files().unwrap_or_default();
    let mut out = Vec::new();
    let mut allowed = picked.0.lock().map_err(|e| e.to_string())?;
    for fp in files {
        let path = to_path(fp)?;
        let name = path.file_name().map_or_else(|| "image".into(), |n| n.to_string_lossy().into_owned());
        allowed.insert(path.clone());
        out.push(PickedFile { path: path.to_string_lossy().into_owned(), name });
    }
    Ok(out)
}

/// Returns the raw bytes of a file previously returned by `pick_files`
/// (as an ArrayBuffer, without JSON/base64 overhead).
#[tauri::command]
async fn read_picked_file(path: String, picked: State<'_, PickedFiles>) -> Result<tauri::ipc::Response, String> {
    let path = PathBuf::from(path);
    if !picked.0.lock().map_err(|e| e.to_string())?.remove(&path) {
        return Err("file was not picked by the user".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(PickedFiles::default())
        .invoke_handler(tauri::generate_handler![save_project, open_project, pick_files, read_picked_file])
        .run(tauri::generate_context!())
        .expect("error while running Zoetrope");
}
