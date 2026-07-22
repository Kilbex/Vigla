import { useCallback, useEffect, useId, useRef, useState } from "react";
import { commands, type PinNoteKind, type PinNoteResponse } from "../bindings";
import { selectCurrentRepoCwd, useMissionsStore } from "../missions/store";
import { useDialogFocus } from "../useDialogFocus";

/**
 * Tier-2C user-oracle surface. A single button in the command panel
 * that opens a tiny inline form for pinning a note. On submit, calls
 * `commands.memoryPinNote(...)`; success shows "Pinned" briefly,
 * rejection shows the redacted preview, error shows the message.
 *
 * Design notes:
 *
 * - **Self-contained**: no external state, no zustand action. The
 *   button is reusable from the drawer / comms feed in later phases
 *   by just rendering `<PinNoteButton />`.
 * - **Keyboard-first**: Cmd/Ctrl+Enter submits, Escape closes.
 * - **No backdrop**: the dialog floats next to its trigger; the rest
 *   of the operations room stays interactive. This is the user's
 *   preference for Vigla's "no modal overlays" style.
 * - **Fail-soft**: if the IPC errors, we surface the message but the
 *   form stays open with the body preserved so the user can retry.
 *
 * The four standard kinds map to the kernel's promotion thresholds.
 * UserAuthored witnesses promote at 0.5 (see V3 policy shortcut), so
 * a fresh pin on any kind is expected to promote on the spot.
 */
export default function PinNoteButton() {
  // A2 (Tier-2G): cwd of the current repo. The button is disabled
  // when no mission has been started this session — pinning requires
  // knowing which per-repo kernel to address.
  const cwd = useMissionsStore(selectCurrentRepoCwd);
  const [open, setOpen] = useState(false);
  const [body, setBody] = useState("");
  const [kind, setKind] = useState<PinNoteKind>("fact");
  const [submitting, setSubmitting] = useState(false);
  const [result, setResult] = useState<PinNoteResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const dialogRef = useRef<HTMLDivElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const titleId = useId();
  const dialogId = useId();

  // The popover is non-modal, so focus enters and returns to the trigger but
  // Tab remains free to leave the form.
  useDialogFocus(open, dialogRef, false);

  // Focus the textarea on open; the form is the only meaningful
  // interaction surface so jumping straight to it is the right
  // default for a pin-and-go workflow.
  useEffect(() => {
    if (open && textareaRef.current) {
      textareaRef.current.focus();
    }
  }, [open]);

  // Escape closes; outside-click closes — both let the user dismiss
  // without committing.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    const onDocClick = (e: MouseEvent) => {
      if (!dialogRef.current) return;
      if (!dialogRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    // setTimeout(0): defer the document handler until after the click
    // that opened the dialog has bubbled. Otherwise the dialog opens
    // and closes in the same tick.
    const t = window.setTimeout(() => {
      document.addEventListener("mousedown", onDocClick);
    }, 0);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.clearTimeout(t);
      document.removeEventListener("mousedown", onDocClick);
    };
  }, [open]);

  const reset = useCallback(() => {
    setBody("");
    setKind("fact");
    setResult(null);
    setError(null);
  }, []);

  // Auto-dismiss the dialog shortly after a successful pin so the user sees
  // the success badge before it closes itself. Driven by an effect (not a
  // bare setTimeout in the submit handler) so its cleanup cancels the
  // pending close whenever the dialog is manually closed, reopened, or
  // unmounted — otherwise a stale timer would tear down a freshly reopened
  // dialog and discard the user's newly typed note.
  useEffect(() => {
    if (!open || result?.outcome !== "pinned") return;
    const closeTimer = window.setTimeout(() => setOpen(false), 1200);
    return () => window.clearTimeout(closeTimer);
  }, [open, result]);

  const handleSubmit = useCallback(async () => {
    const trimmed = body.trim();
    if (trimmed.length === 0 || submitting) return;
    if (!cwd) {
      setError("Start a mission first — memory is per-repository.");
      return;
    }
    setSubmitting(true);
    setResult(null);
    setError(null);
    try {
      const r = await commands.memoryPinNote(cwd, {
        kind,
        scope_kind: "repo",
        scope_value: null,
        body: trimmed,
      });
      if (r.status === "ok") {
        setResult(r.data);
        // On a successful pin, clear the body so the next open is a clean
        // slate. The dialog is kept open briefly (so the user sees the
        // success badge) and then auto-closed by the effect below, whose
        // cleanup cancels the pending close if the dialog is manually
        // closed/reopened in the meantime.
        if (r.data.outcome === "pinned") {
          setBody("");
        }
      } else {
        setError(r.error);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  }, [body, kind, submitting, cwd]);

  const handleKey = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      // Cmd/Ctrl+Enter submits. Plain Enter inserts a newline so
      // multi-line bodies work as expected.
      if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
        e.preventDefault();
        handleSubmit();
      }
    },
    [handleSubmit],
  );

  return (
    <span className="pin-note-trigger">
      <button
        type="button"
        className="pin-note-button"
        onClick={() => {
          if (open) {
            setOpen(false);
          } else {
            reset();
            setOpen(true);
          }
        }}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? dialogId : undefined}
        disabled={!cwd}
        title={
          cwd
            ? "Pin a project note to memory (Cmd/Ctrl+Enter to submit)"
            : "Start a mission first — memory is per-repository"
        }
      >
        Pin Note
      </button>
      {open ? (
        <div
          id={dialogId}
          ref={dialogRef}
          className="pin-note-dialog"
          role="dialog"
          aria-labelledby={titleId}
        >
          <div id={titleId} className="pin-note-title">
            Pin a project note
          </div>
          <div className="pin-note-row">
            <label className="pin-note-label" htmlFor={`${titleId}-kind`}>
              Kind
            </label>
            <select
              id={`${titleId}-kind`}
              className="pin-note-select"
              value={kind}
              onChange={(e) => setKind(e.target.value as PinNoteKind)}
              disabled={submitting}
            >
              <option value="fact">fact</option>
              <option value="decision">decision</option>
              <option value="procedure">procedure</option>
              <option value="hazard">hazard</option>
            </select>
          </div>
          <textarea
            ref={textareaRef}
            className="pin-note-textarea"
            placeholder="What did you learn? Workers in the next mission will see this."
            value={body}
            onChange={(e) => setBody(e.target.value)}
            onKeyDown={handleKey}
            rows={4}
            maxLength={4 * 1024}
            disabled={submitting}
            aria-label="Note body"
          />
          <div className="pin-note-feedback" aria-live="polite">
            {error ? <span className="pin-note-error">{error}</span> : null}
            {result?.outcome === "pinned" ? (
              <span className="pin-note-ok">
                Pinned {result.promoted ? "· learned" : "· awaiting policy"}
              </span>
            ) : null}
            {result?.outcome === "rejected" ? (
              <span className="pin-note-error">
                Rejected ({result.reason}). Preview: {result.redacted_preview}
              </span>
            ) : null}
          </div>
          <div className="pin-note-actions">
            <button
              type="button"
              className="pin-note-cancel"
              onClick={() => setOpen(false)}
              disabled={submitting}
            >
              Cancel
            </button>
            <button
              type="button"
              className="pin-note-submit"
              onClick={handleSubmit}
              disabled={submitting || body.trim().length === 0}
            >
              {submitting ? "Pinning…" : "Pin"}
            </button>
          </div>
        </div>
      ) : null}
    </span>
  );
}
