import { useRef, useState } from "react";
import { selectSquadIds, useOpsStore } from "../store";
import { SQUAD_COLORS, type SquadColor } from "../store/types";

/// Minimal squad management UI. Lives inside the comms-feed
/// actions column. Lets the operator create / rename / delete squads
/// and see member counts at a glance. Worker assignment lives in the
/// drawer (per-worker context). Playbooks can populate squads; this panel is
/// the manual path.
export default function SquadPanel() {
  const ids = useOpsStore(selectSquadIds);
  const squads = useOpsStore((s) => s.squads);
  const createSquad = useOpsStore((s) => s.createSquad);
  const deleteSquad = useOpsStore((s) => s.deleteSquad);
  const renameSquad = useOpsStore((s) => s.renameSquad);

  const [draftOpen, setDraftOpen] = useState(false);
  const [draftName, setDraftName] = useState("");
  const [draftColor, setDraftColor] = useState<SquadColor>("indigo");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState("");

  /// Roving-tabindex (audit-r5): only the selected color is tabbable;
  /// arrow keys move focus + selection between siblings with wrap-around.
  /// Matches the WAI-ARIA radio-group pattern. Derives the "current"
  /// index from the keydown target rather than from `draftColor` so
  /// navigation works correctly even when focus has moved without
  /// changing selection.
  const swatchRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const moveSwatchFocus = (fromIdx: number, delta: number) => {
    const len = SQUAD_COLORS.length;
    const next = ((fromIdx + delta) % len + len) % len;
    setDraftColor(SQUAD_COLORS[next]);
    swatchRefs.current[next]?.focus();
  };

  const startCreate = () => {
    setDraftOpen(true);
    setDraftName("");
    setDraftColor("indigo");
  };
  const cancelCreate = () => {
    setDraftOpen(false);
  };
  const submitCreate = () => {
    const trimmed = draftName.trim();
    if (!trimmed) return;
    createSquad(trimmed, draftColor);
    setDraftOpen(false);
    setDraftName("");
  };

  const startEdit = (id: string, name: string) => {
    setEditingId(id);
    setEditingName(name);
  };
  const submitEdit = () => {
    if (editingId === null) return;
    const trimmed = editingName.trim();
    if (trimmed) renameSquad(editingId, trimmed);
    setEditingId(null);
  };

  return (
    <div className="squad-panel" aria-label="Squad management">
      <div className="comms-actions-title">SQUADS</div>
      {!draftOpen ? (
        <button
          className="spawn-btn"
          onClick={startCreate}
          aria-label="create squad"
        >
          + create squad
        </button>
      ) : (
        <div className="squad-draft" role="group" aria-label="new squad">
          <label className="real-spawn-label">
            name
            <input
              autoFocus
              value={draftName}
              onChange={(e) => setDraftName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") submitCreate();
                if (e.key === "Escape") cancelCreate();
              }}
              placeholder="e.g. Frontend Squad"
            />
          </label>
          <div className="real-spawn-label">
            color
            <div
              className="squad-color-row"
              role="radiogroup"
              aria-label="squad color"
              onKeyDown={(e) => {
                const fromIdx = swatchRefs.current.indexOf(
                  e.target as HTMLButtonElement,
                );
                if (fromIdx < 0) return;
                if (e.key === "ArrowRight" || e.key === "ArrowDown") {
                  e.preventDefault();
                  moveSwatchFocus(fromIdx, 1);
                } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
                  e.preventDefault();
                  moveSwatchFocus(fromIdx, -1);
                }
              }}
            >
              {SQUAD_COLORS.map((c, idx) => (
                <button
                  key={c}
                  ref={(el) => {
                    swatchRefs.current[idx] = el;
                  }}
                  type="button"
                  role="radio"
                  aria-checked={draftColor === c}
                  aria-label={c}
                  tabIndex={draftColor === c ? 0 : -1}
                  className={
                    "squad-color-swatch squad-color-swatch--" +
                    c +
                    (draftColor === c ? " squad-color-swatch--on" : "")
                  }
                  onClick={() => setDraftColor(c)}
                />
              ))}
            </div>
          </div>
          <div className="squad-draft-actions">
            <button
              className="spawn-btn"
              onClick={submitCreate}
              disabled={!draftName.trim()}
            >
              create
            </button>
            <button className="spawn-btn spawn-btn-clear" onClick={cancelCreate}>
              cancel
            </button>
          </div>
        </div>
      )}

      {ids.length > 0 ? (
        <ul className="squad-list">
          {ids.map((id) => {
            const sq = squads[id];
            if (!sq) return null;
            const memberCount = sq.workerIds.length;
            const isEditing = editingId === id;
            return (
              <li key={id} className="squad-row">
                <span
                  className={"squad-color-dot squad-color-dot--" + sq.color}
                  aria-hidden
                />
                {isEditing ? (
                  <input
                    autoFocus
                    className="squad-row-input"
                    value={editingName}
                    onChange={(e) => setEditingName(e.target.value)}
                    onBlur={submitEdit}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") submitEdit();
                      if (e.key === "Escape") setEditingId(null);
                    }}
                    aria-label={`rename ${sq.name}`}
                  />
                ) : (
                  <button
                    className="squad-row-name"
                    onClick={() => startEdit(id, sq.name)}
                    aria-label={`rename ${sq.name}`}
                  >
                    {sq.name}
                  </button>
                )}
                <span className="squad-row-count" aria-label={`${memberCount} members`}>
                  {memberCount}
                </span>
                <button
                  className="squad-row-delete"
                  onClick={() => deleteSquad(id)}
                  aria-label={`delete ${sq.name}`}
                  title="delete squad"
                >
                  ×
                </button>
              </li>
            );
          })}
        </ul>
      ) : (
        <div className="comms-empty">no squads yet</div>
      )}
    </div>
  );
}
