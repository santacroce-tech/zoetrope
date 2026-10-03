import { useState } from "react";
import type { Engine } from "../engine";
import { buildExport, embedSnippet, totalSize, type PublishSettings, type ScaleMode } from "../export";
import { saveExport } from "../platform";
import { ColorField } from "./fields";

interface Props {
  engine: Engine;
  /** Title used when the settings leave it empty (the file name). */
  fallbackTitle: string;
  onClose: () => void;
  /** Settings changed: save them with the project (one undo step). */
  onSettings: (s: PublishSettings) => void;
  onMessage: (m: { text: string; error?: boolean }) => void;
}

const DEFAULTS: PublishSettings = { title: "", scale: "letterbox", mode: "singleFile", pageColor: "#111111", startOnClick: false };

const SCALE_HELP: Record<ScaleMode, string> = {
  letterbox: "The whole stage, as large as the window allows; bars in the page color fill the rest.",
  fill: "Covers the whole window, keeping proportions; whatever sticks out is cropped.",
  fixed: "Stage pixels 1:1, centered. Best inside an iframe of the stage's size.",
};

/** Export settings (saved with the project), embed snippet, and the export itself. */
export function ExportDialog({ engine, fallbackTitle, onClose, onSettings, onMessage }: Props) {
  const [s, setS] = useState<PublishSettings>(() => ({ ...DEFAULTS, ...JSON.parse(engine.publishJson()) }));
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const set = (patch: Partial<PublishSettings>) => setS((old) => ({ ...old, ...patch }));
  const snippet = embedSnippet(engine, s, fallbackTitle);

  const run = async () => {
    setBusy(true);
    try {
      onSettings(s);
      const files = await buildExport(engine, s, fallbackTitle);
      const dest = await saveExport(files);
      if (dest) {
        const mb = (totalSize(files) / 1048576).toFixed(1);
        onMessage({ text: `Exported ${files.length === 1 ? files[0].name : `${files.length} files`} to ${dest} (${mb} MB)` });
        onClose();
      }
    } catch (e) {
      onMessage({ text: `Export failed: ${e instanceof Error ? e.message : String(e)}`, error: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <form
        className="modal export-dialog"
        onClick={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          void run();
        }}
      >
        <h3>Export</h3>
        <label className="field wide">
          <span className="field-label">Title</span>
          <input value={s.title} placeholder={fallbackTitle} onChange={(e) => set({ title: e.target.value })} />
        </label>

        <fieldset className="choice">
          <legend>Format</legend>
          <label className="check">
            <input type="radio" checked={s.mode === "singleFile"} onChange={() => set({ mode: "singleFile" })} />
            <span>
              <b>Single HTML file</b>
              <small>Everything in one page. Plays offline, even opened straight from disk.</small>
            </span>
          </label>
          <label className="check">
            <input type="radio" checked={s.mode === "folder"} onChange={() => set({ mode: "folder" })} />
            <span>
              <b>Folder</b>
              <small>Page + player script + asset pack, for large projects on a web server (needs http/https).</small>
            </span>
          </label>
        </fieldset>

        <div className="row">
          <span className="field-label">Scaling</span>
          <select value={s.scale} onChange={(e) => set({ scale: e.target.value as ScaleMode })}>
            <option value="letterbox">Letterbox (fit)</option>
            <option value="fill">Fill (crop)</option>
            <option value="fixed">Fixed size</option>
          </select>
          <span className="field-label">Page</span>
          <ColorField value={s.pageColor} onCommit={(pageColor) => set({ pageColor })} />
        </div>
        <p className="hint flush">{SCALE_HELP[s.scale]}</p>
        <label className="check">
          <input type="checkbox" checked={s.startOnClick} onChange={(e) => set({ startOnClick: e.target.checked })} />
          Start on click (browsers only allow sound after a click)
        </label>

        <div className="embed">
          <span className="field-label">Embed in a web page</span>
          <textarea readOnly value={snippet} rows={3} onFocus={(e) => e.target.select()} />
          <button
            type="button"
            className="mini"
            onClick={() => {
              void navigator.clipboard?.writeText(snippet).then(() => setCopied(true));
            }}
          >
            {copied ? "Copied" : "Copy"}
          </button>
        </div>

        <div className="btn-row">
          <button type="button" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" disabled={busy}>
            {busy ? "Exporting…" : "Export…"}
          </button>
        </div>
      </form>
    </div>
  );
}
