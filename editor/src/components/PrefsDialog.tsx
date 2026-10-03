import { useEscape } from "./useEscape";
import { useState } from "react";
import { DEFAULT_PREFS, type Prefs } from "../prefs";
import { NumberField } from "./fields";

/** Preferences (per machine; see prefs.ts). */
export function PrefsDialog({ prefs, onSave, onClose }: { prefs: Prefs; onSave: (p: Prefs) => void; onClose: () => void }) {
  useEscape(onClose);
  const [p, setP] = useState(prefs);
  const set = (patch: Partial<Prefs>) => setP((old) => ({ ...old, ...patch }));
  const check = (key: keyof Prefs, label: string) => (
    <label className="check">
      <input type="checkbox" checked={p[key] as boolean} onChange={(e) => set({ [key]: e.target.checked })} />
      {label}
    </label>
  );
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <form
        className="modal"
        onClick={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          onSave(p);
          onClose();
        }}
      >
        <h3>Preferences</h3>
        <fieldset className="choice">
          <legend>Safety</legend>
          {check("autosave", "Autosave unsaved work (offered back after a crash)")}
          <NumberField label="Every" suffix="s" precision={0} min={5} max={600} value={p.autosaveSeconds} disabled={!p.autosave} onCommit={(autosaveSeconds) => set({ autosaveSeconds })} />
        </fieldset>
        <fieldset className="choice">
          <legend>Stage</legend>
          {check("showRulers", "Rulers")}
          {check("showGuides", "Guide layers")}
          {check("showGrid", "Grid")}
          <NumberField label="Grid" suffix="px" precision={0} min={2} max={500} value={p.gridSize} onCommit={(gridSize) => set({ gridSize })} />
          {check("snapToGrid", "Snap to grid")}
          {check("snapToObjects", "Snap to objects")}
        </fieldset>
        <fieldset className="choice">
          <legend>Playback</legend>
          {check("loopPreview", "Loop the preview")}
        </fieldset>
        <div className="btn-row">
          <button type="button" onClick={() => setP(DEFAULT_PREFS)}>
            Defaults
          </button>
          <button type="button" onClick={onClose}>
            Cancel
          </button>
          <button type="submit">Save</button>
        </div>
      </form>
    </div>
  );
}
