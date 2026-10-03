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

/// Where the current export goes: chosen by the user in `export_begin`;
/// `export_write` can only write there.
#[derive(Default)]
struct ExportTarget(Mutex<Option<ExportDest>>);

enum ExportDest {
    /// Single-file export: exactly this file.
    File(PathBuf),
    /// Folder export: plain file names inside this directory.
    Folder(PathBuf),
}

/// Starts an export: asks for the destination (a `.html` file when
/// `single`, else a folder). Returns the chosen path, or null if cancelled.
#[tauri::command]
async fn export_begin(app: AppHandle, single: bool, name: String, target: State<'_, ExportTarget>) -> Result<Option<String>, String> {
    let dest = if single {
        let picked = app.dialog().file().add_filter("Web page", &["html"]).set_file_name(name).blocking_save_file();
        match picked {
            Some(fp) => ExportDest::File(to_path(fp)?),
            None => return Ok(None),
        }
    } else {
        match app.dialog().file().set_title("Export into folder").blocking_pick_folder() {
            Some(fp) => ExportDest::Folder(to_path(fp)?),
            None => return Ok(None),
        }
    };
    let shown = match &dest {
        ExportDest::File(p) | ExportDest::Folder(p) => p.to_string_lossy().into_owned(),
    };
    *target.0.lock().unwrap() = Some(dest);
    Ok(Some(shown))
}

/// Writes one export file. The body is the raw bytes; the `x-file-name`
/// header names it (a plain name: no directories). Single-file exports
/// write their one file to the chosen path.
#[tauri::command]
fn export_write(request: tauri::ipc::Request<'_>, target: State<'_, ExportTarget>) -> Result<(), String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else { return Err("export_write expects raw bytes".into()) };
    let name = request.headers().get("x-file-name").and_then(|v| v.to_str().ok()).unwrap_or("");
    let name = percent_decode(name);
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(format!("bad export file name {name:?}"));
    }
    let guard = target.0.lock().unwrap();
    let path = match guard.as_ref() {
        Some(ExportDest::File(p)) => p.clone(),
        Some(ExportDest::Folder(dir)) => dir.join(&name),
        None => return Err("no export in progress".into()),
    };
    let tmp = path.with_extension("export.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("could not replace {}: {e}", path.display()))
}

/// Header values are ASCII; the frontend percent-encodes file names.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
        .manage(ExportTarget::default())
        .invoke_handler(tauri::generate_handler![save_project, open_project, export_begin, export_write, pick_files, read_picked_file])
        .run(tauri::generate_context!())
        .expect("error while running Zoetrope");
}
