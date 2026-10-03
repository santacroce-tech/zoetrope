//! Native shell. Boundary contract (see docs/ARCHITECTURE.md): this side does
//! OS dialogs and filesystem I/O ONLY. It never parses or interprets project
//! contents — those are opaque strings owned by the WASM core.

use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};
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

// ------------------------------------------------------------------ recent files

/// Most recent first; at most this many.
const RECENT_LIMIT: usize = 10;

fn recent_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    Ok(dir.join("recent.json"))
}

fn read_recent(app: &AppHandle) -> Vec<PathBuf> {
    let Ok(path) = recent_path(app) else { return Vec::new() };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

/// Records a project the user opened or saved through a dialog: it goes to
/// the top of the recent list, and the frontend may save to it again.
fn remember(app: &AppHandle, known: &KnownProjects, path: &Path) {
    known.0.lock().unwrap().insert(path.to_path_buf());
    let mut list = read_recent(app);
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(RECENT_LIMIT);
    if let Ok(file) = recent_path(app) {
        let json = serde_json::to_string(&list.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>()).unwrap_or_default();
        let _ = write_atomic(&file, &json);
    }
}

/// Project paths the frontend may write without a dialog: ones the user
/// chose this session (plus the recent list, checked separately).
#[derive(Default)]
struct KnownProjects(Mutex<HashSet<PathBuf>>);

#[derive(Serialize)]
struct RecentFile {
    path: String,
    name: String,
}

/// Recent projects that still exist, most recent first.
#[tauri::command]
fn recent_files(app: AppHandle) -> Vec<RecentFile> {
    read_recent(&app)
        .into_iter()
        .filter(|p| p.is_file())
        .map(|p| RecentFile {
            name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            path: p.to_string_lossy().into_owned(),
        })
        .collect()
}

/// Opens a project from the recent list (and only from it).
#[tauri::command]
fn open_recent(app: AppHandle, path: String, known: State<'_, KnownProjects>) -> Result<OpenedFile, String> {
    let path = PathBuf::from(path);
    if !read_recent(&app).contains(&path) {
        return Err("that file is not in the recent list".into());
    }
    let contents = std::fs::read_to_string(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    remember(&app, &known, &path);
    Ok(OpenedFile { path: path.to_string_lossy().into_owned(), contents })
}

// ------------------------------------------------------------------ autosave

/// Unsaved work, kept in the app data folder until the project is saved
/// (or the user discards it). On the next launch it is offered back.
fn autosave_paths(app: &AppHandle) -> Result<(PathBuf, PathBuf), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    Ok((dir.join("autosave.zoe"), dir.join("autosave.json")))
}

/// `meta` is opaque to the shell (the frontend stores the original path and time).
#[tauri::command]
fn autosave_write(app: AppHandle, contents: String, meta: String) -> Result<(), String> {
    let (data, info) = autosave_paths(&app)?;
    write_atomic(&data, &contents)?;
    write_atomic(&info, &meta)
}

#[derive(Serialize)]
struct Autosave {
    contents: String,
    meta: String,
}

#[tauri::command]
fn autosave_read(app: AppHandle) -> Result<Option<Autosave>, String> {
    let (data, info) = autosave_paths(&app)?;
    match (std::fs::read_to_string(&data), std::fs::read_to_string(&info)) {
        (Ok(contents), Ok(meta)) => Ok(Some(Autosave { contents, meta })),
        _ => Ok(None),
    }
}

#[tauri::command]
fn autosave_clear(app: AppHandle) -> Result<(), String> {
    let (data, info) = autosave_paths(&app)?;
    let _ = std::fs::remove_file(data);
    let _ = std::fs::remove_file(info);
    Ok(())
}

/// Saves `contents` to `path`, or asks for a path when `path` is null.
/// A given `path` must be one the user chose (this session or recently).
/// Returns the path written, or null if the user cancelled.
#[tauri::command]
async fn save_project(
    app: AppHandle,
    contents: String,
    path: Option<String>,
    known: State<'_, KnownProjects>,
) -> Result<Option<String>, String> {
    let path = match path {
        Some(p) => {
            let p = PathBuf::from(p);
            if !known.0.lock().unwrap().contains(&p) && !read_recent(&app).contains(&p) {
                return Err("can only save to a file chosen in a dialog: use Save As".into());
            }
            p
        }
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
    remember(&app, &known, &path);
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
async fn export_begin(
    app: AppHandle,
    single: bool,
    name: String,
    target: State<'_, ExportTarget>,
) -> Result<Option<String>, String> {
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
async fn open_project(app: AppHandle, known: State<'_, KnownProjects>) -> Result<Option<OpenedFile>, String> {
    let picked = app.dialog().file().add_filter("Zoetrope Project", &[EXTENSION]).blocking_pick_file();
    let Some(fp) = picked else { return Ok(None) };
    let path = to_path(fp)?;
    let contents = std::fs::read_to_string(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    remember(&app, &known, &path);
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
        .manage(KnownProjects::default())
        .invoke_handler(tauri::generate_handler![
            save_project,
            open_project,
            recent_files,
            open_recent,
            autosave_write,
            autosave_read,
            autosave_clear,
            export_begin,
            export_write,
            pick_files,
            read_picked_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running Zoetrope");
}
