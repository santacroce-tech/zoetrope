import { useEscape } from "./useEscape";
const MOD = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl+";

/** Every keyboard shortcut, in one place (keep in sync with App.tsx's handler). */
export const SHORTCUTS: { group: string; keys: [string, string][] }[] = [
  {
    group: "File",
    keys: [
      [`${MOD}S`, "Save"],
      [`⇧${MOD}S`, "Save as"],
      [`${MOD}O`, "Open"],
      [`${MOD}I`, "Import images"],
      [`${MOD}Z`, "Undo"],
      [`⇧${MOD}Z / ${MOD}Y`, "Redo"],
      ["?", "This list"],
    ],
  },
  {
    group: "Tools",
    keys: [
      ["V", "Selection"],
      ["A", "Subselection (anchors)"],
      ["P", "Pen"],
      ["Y", "Pencil"],
      ["R / O / U / N", "Rectangle / ellipse / polygon / line"],
      ["T", "Text"],
      ["F", "Gradient transform"],
      ["I / K", "Eyedropper / paint bucket"],
      ["H or hold Space", "Hand (pan)"],
    ],
  },
  {
    group: "Objects",
    keys: [
      [`${MOD}C / ${MOD}X / ${MOD}V`, "Copy / cut / paste (also between projects)"],
      [`⇧${MOD}V`, "Paste in place"],
      [`${MOD}A`, "Select all"],
      ["Esc", "Deselect · leave symbol editing"],
      ["Delete", "Delete"],
      ["Arrows (⇧ = 10 px)", "Nudge"],
      [`${MOD}D`, "Duplicate"],
      [`${MOD}B`, "Convert to path"],
      [`${MOD}] / ${MOD}[`, "Forward / backward (⇧: front / back)"],
      ["F8", "Convert to symbol"],
      [`${MOD}E or double-click`, "Edit symbol in place"],
    ],
  },
  {
    group: "Timeline",
    keys: [
      ["Enter", "Play / pause (the preview owns the keyboard; Esc stops it)"],
      [", / .", "Previous / next frame"],
      ["Home / End", "First / last frame"],
      ["F5 / ⇧F5", "Insert / remove frame"],
      ["F6 / ⇧F6", "Insert / clear keyframe"],
      ["F7", "Blank keyframe"],
    ],
  },
  {
    group: "View",
    keys: [
      [`${MOD}= / ${MOD}−`, "Zoom in / out"],
      [`${MOD}0`, "Fit"],
      [`${MOD}1`, "100%"],
      [`${MOD}'`, "Grid (⇧: snap to grid)"],
      [`${MOD} + wheel or pinch`, "Zoom at cursor"],
    ],
  },
];

export function ShortcutsDialog({ onClose }: { onClose: () => void }) {
  useEscape(onClose);
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal shortcuts" onClick={(e) => e.stopPropagation()}>
        <h3>Keyboard shortcuts</h3>
        <div className="shortcut-groups">
          {SHORTCUTS.map((g) => (
            <section key={g.group}>
              <h4>{g.group}</h4>
              <table>
                <tbody>
                  {g.keys.map(([k, what]) => (
                    <tr key={k}>
                      <td>
                        <kbd>{k}</kbd>
                      </td>
                      <td>{what}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>
          ))}
        </div>
        <div className="btn-row">
          <button autoFocus onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
