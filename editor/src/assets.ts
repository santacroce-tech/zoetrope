// Font and audio import: pick files, check them, and embed them in the
// project. Fonts are validated by the core; audio is decoded here first,
// which both proves it plays and gives its duration.
import { probeAudio } from "./runtime/audio";
import type { Engine } from "./engine";
import { importFiles, type PickedBinary } from "./platform";

export interface ImportResult {
  ids: number[];
  errors: string[];
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export async function importFonts(engine: Engine, files?: PickedBinary[]): Promise<ImportResult> {
  const picked = files ?? (await importFiles("font"));
  const r: ImportResult = { ids: [], errors: [] };
  for (const f of picked) {
    try {
      r.ids.push(engine.importFont(f.name, f.bytes));
    } catch (e) {
      r.errors.push(`${f.name}: ${message(e)}`);
    }
  }
  return r;
}

export async function importAudio(engine: Engine, files?: PickedBinary[]): Promise<ImportResult> {
  const picked = files ?? (await importFiles("audio"));
  const r: ImportResult = { ids: [], errors: [] };
  for (const f of picked) {
    try {
      const duration = await probeAudio(f.bytes);
      r.ids.push(engine.importAudio(f.name, f.bytes, duration));
    } catch (e) {
      r.errors.push(`${f.name}: ${message(e) || "not a playable audio file"}`);
    }
  }
  return r;
}

/** One status-bar line for an import. */
export function describeImport(kind: string, r: ImportResult): { text: string; error?: boolean } {
  if (r.errors.length) return { text: r.errors.join("; "), error: true };
  return { text: `Imported ${r.ids.length} ${kind}${r.ids.length === 1 ? "" : "s"}` };
}
