// Frontend ↔ native boundary. Under Tauri, file I/O goes through the Rust
// shell's commands; in a plain browser (vite dev without Tauri) it falls back
// to download/upload so the editor stays usable for development and testing.
import { invoke } from "@tauri-apps/api/core";

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
