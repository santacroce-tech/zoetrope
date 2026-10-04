// Frontend ↔ native boundary. Under Tauri, file I/O goes through the Rust
// shell's commands; in a plain browser (vite dev without Tauri) it falls back
// to download/upload so the editor stays usable for development and testing.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

export const isTauri = "__TAURI_INTERNALS__" in window;

export interface OpenedFile {
  path: string;
  contents: string;
}

export interface PickedBinary {
  name: string;
  bytes: Uint8Array;
}

/** Saves to `path`, or prompts for one when null. Returns the path, or null if cancelled. */
export async function saveProject(contents: string, path: string | null): Promise<string | null> {
  if (isTauri) return invoke<string | null>("save_project", { contents, path });
  const name = path ?? "Untitled.zoe";
  const url = URL.createObjectURL(new Blob([contents], { type: "application/json" }));
  const a = Object.assign(document.createElement("a"), { href: url, download: name });
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 0);
  return name;
}

/**
 * Writes exported files: asks for a .html path (one file) or a folder
 * (several). Returns where they went, or null if cancelled. In a browser,
 * each file is downloaded.
 */
export async function saveExport(files: { name: string; data: string | Uint8Array }[]): Promise<string | null> {
  const single = files.length === 1;
  if (isTauri) {
    const dest = await invoke<string | null>("export_begin", { single, name: files[0].name });
    if (!dest) return null;
    for (const f of files) {
      const bytes = typeof f.data === "string" ? new TextEncoder().encode(f.data) : f.data;
      await invoke("export_write", bytes, { headers: { "x-file-name": encodeURIComponent(f.name) } });
    }
    return dest;
  }
  for (const f of files) {
    const type = f.name.endsWith(".html") ? "text/html" : f.name.endsWith(".js") ? "text/javascript" : "application/octet-stream";
    const url = URL.createObjectURL(new Blob([f.data as BlobPart], { type }));
    const a = Object.assign(document.createElement("a"), { href: url, download: f.name });
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 0);
  }
  return single ? files[0].name : "Downloads";
}

/** Prompts for a project file. Returns null if cancelled. */
export async function openProject(): Promise<OpenedFile | null> {
  if (isTauri) return invoke<OpenedFile | null>("open_project");
  const [file] = await pickBrowserFiles(".zoe,.json", false);
  return file ? { path: file.name, contents: await file.text() } : null;
}

export type ImportKind = "image" | "font" | "audio";

const ACCEPT: Record<ImportKind, string> = {
  image: "image/png,image/jpeg,image/gif",
  font: ".ttf,.otf,font/ttf,font/otf",
  audio: "audio/*,.mp3,.wav,.m4a,.aac,.ogg,.flac",
};

/** Prompts for image files (multi-select). Returns [] if cancelled. */
export function importImages(): Promise<PickedBinary[]> {
  return importFiles("image");
}

/** Prompts for files of a kind (multi-select). Returns [] if cancelled. */
export async function importFiles(kind: ImportKind): Promise<PickedBinary[]> {
  if (isTauri) {
    const picked = await invoke<{ path: string; name: string }[]>("pick_files", { kind });
    return Promise.all(
      picked.map(async ({ path, name }) => ({
        name,
        bytes: new Uint8Array(await invoke<ArrayBuffer>("read_picked_file", { path })),
      })),
    );
  }
  const files = await pickBrowserFiles(ACCEPT[kind], true);
  return Promise.all(files.map(fileToBinary));
}

export async function fileToBinary(file: File): Promise<PickedBinary> {
  return { name: file.name, bytes: new Uint8Array(await file.arrayBuffer()) };
}

function pickBrowserFiles(accept: string, multiple: boolean): Promise<File[]> {
  return new Promise((resolve) => {
    const input = Object.assign(document.createElement("input"), { type: "file", accept, multiple });
    input.addEventListener("cancel", () => resolve([]));
    input.addEventListener("change", () => resolve(Array.from(input.files ?? [])));
    input.click();
  });
}

// ------------------------------------------------------------------ autosave

/** What the autosave knows besides the project itself. */
export interface AutosaveMeta {
  /** The file it belongs to, or null if it was never saved. */
  path: string | null;
  /** When it was written (ms since epoch). */
  savedAt: number;
}

export interface AutosaveEntry {
  contents: string;
  meta: AutosaveMeta;
}

const IDB = { name: "zoetrope", store: "autosave", key: "current" };

/** Browser fallback: one IndexedDB record (projects can be several MB, too big for localStorage). */
function idb<T>(mode: IDBTransactionMode, op: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open(IDB.name, 1);
    open.onupgradeneeded = () => open.result.createObjectStore(IDB.store);
    open.onerror = () => reject(open.error);
    open.onsuccess = () => {
      const tx = open.result.transaction(IDB.store, mode);
      const req = op(tx.objectStore(IDB.store));
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
      tx.oncomplete = () => open.result.close();
    };
  });
}

export async function writeAutosave(entry: AutosaveEntry): Promise<void> {
  if (isTauri) return invoke("autosave_write", { contents: entry.contents, meta: JSON.stringify(entry.meta) });
  await idb("readwrite", (s) => s.put(entry, IDB.key));
}

export async function readAutosave(): Promise<AutosaveEntry | null> {
  if (isTauri) {
    const r = await invoke<{ contents: string; meta: string } | null>("autosave_read");
    return r ? { contents: r.contents, meta: JSON.parse(r.meta) } : null;
  }
  return (await idb<AutosaveEntry | undefined>("readonly", (s) => s.get(IDB.key))) ?? null;
}

export async function clearAutosave(): Promise<void> {
  if (isTauri) return invoke("autosave_clear");
  await idb("readwrite", (s) => s.delete(IDB.key));
}

// ------------------------------------------------------------------ recent files

export interface RecentFile {
  path: string;
  name: string;
}

/** Recently opened/saved projects (native app only; a browser can't reopen files by path). */
export async function recentFiles(): Promise<RecentFile[]> {
  return isTauri ? invoke<RecentFile[]>("recent_files") : [];
}

export async function openRecent(path: string): Promise<OpenedFile> {
  return invoke<OpenedFile>("open_recent", { path });
}

// ------------------------------------------------------------------ native app integration

/** Native menu clicks (item ids from src-tauri/src/menu.rs). No-op in a browser. */
export async function onMenu(handler: (id: string) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  return listen<string>("menu", (e) => handler(e.payload));
}

/** The OS asked to open a project (double-clicked .zoe file). */
export async function onOpenFileRequest(handler: () => void): Promise<() => void> {
  if (!isTauri) return () => {};
  return listen("open-file", () => handler());
}

/** The next project the OS asked to open (at launch or since), or null. */
export async function takePendingOpen(): Promise<OpenedFile | null> {
  return isTauri ? invoke<OpenedFile | null>("take_pending_open") : null;
}

/** Opens one of the project's web pages in the browser. */
export async function openUrl(url: string): Promise<void> {
  if (isTauri) return invoke("open_url", { url });
  window.open(url, "_blank", "noopener");
}

/**
 * Asks before the window closes: `allow()` returns false to keep it open
 * (e.g. to show an unsaved-changes prompt). Browsers get the standard
 * "Leave site?" dialog instead.
 */
export async function onCloseRequested(allow: () => boolean): Promise<() => void> {
  if (isTauri) {
    return getCurrentWindow().onCloseRequested((e) => {
      if (!allow()) e.preventDefault();
    });
  }
  const before = (e: BeforeUnloadEvent) => {
    if (!allow()) {
      e.preventDefault();
      e.returnValue = "";
    }
  };
  window.addEventListener("beforeunload", before);
  return () => window.removeEventListener("beforeunload", before);
}

/** Closes the window without asking (after the prompt was answered). */
export async function closeWindow(): Promise<void> {
  if (isTauri) await getCurrentWindow().destroy();
  else window.close();
}

/** Quits the app (native only; after the prompt was answered). */
export async function exitApp(): Promise<void> {
  if (isTauri) await invoke("exit_app");
}
