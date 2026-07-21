// S10 — RevertButton. Destructive-action button + confirmation
// dialog around the existing S4 `revert_mission` Tauri command.
// Dialog body surfaces the durable rollback anchor so the user has explicit
// context. The host independently verifies that the mission was merged.

import { useCallback, useRef, useState } from "react";
import { commands } from "../bindings";
import { useDialogFocus } from "../useDialogFocus";

interface RevertButtonProps {
  missionId: string;
  rollbackAnchor: string;
  disabled?: boolean;
  onReverted?: (outcome: { restored_sha: string; pre_merge_tag: string }) => void;
}

type DialogState =
  | { kind: "idle" }
  | { kind: "confirming" }
  | { kind: "submitting" }
  | { kind: "error"; message: string };

export default function RevertButton({
  missionId,
  rollbackAnchor,
  disabled,
  onReverted,
}: RevertButtonProps) {
  const [state, setState] = useState<DialogState>({ kind: "idle" });
  const open = useCallback(() => {
    if (!disabled) setState({ kind: "confirming" });
  }, [disabled]);
  const close = useCallback(() => setState({ kind: "idle" }), []);
  const submit = useCallback(() => {
    setState({ kind: "submitting" });
    commands
      .revertMission(missionId)
      .then((result) => {
        if (result.status === "ok") {
          setState({ kind: "idle" });
          onReverted?.(result.data);
        } else {
          setState({ kind: "error", message: result.error });
        }
      })
      .catch((err: unknown) => {
        const message = typeof err === "string" ? err : String(err);
        setState({ kind: "error", message });
      });
  }, [missionId, onReverted]);

  const submitting = state.kind === "submitting";

  const dialogRef = useRef<HTMLElement>(null);
  // Move focus into the destructive dialog on open (lands on Cancel), trap Tab
  // within it, and restore focus to the trigger on close (FE-A2).
  useDialogFocus(state.kind !== "idle", dialogRef);

  return (
    <>
      <button
        type="button"
        className="revert-button"
        onClick={open}
        disabled={disabled || submitting}
        aria-busy={submitting}
        aria-label="Revert mission"
      >
        Revert mission
      </button>
      {state.kind !== "idle" ? (
        <>
          <button
            type="button"
            className="revert-dialog-scrim"
            onClick={close}
            aria-label="close dialog"
            tabIndex={-1}
          />
          <section
            className="revert-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="revert-dialog-title"
            ref={dialogRef}
          >
            <h2 id="revert-dialog-title" className="revert-dialog-title">
              Revert mission?
            </h2>
            <div className="revert-dialog-body">
              <p>
                This creates a revert commit on the mission&apos;s target branch,
                undoing its merged changes while preserving commits made later.
              </p>
              <p>
                <strong>Recorded rollback anchor:</strong>{" "}
                <span className="revert-dialog-sha">{rollbackAnchor}</span>
              </p>
              {state.kind === "error" ? (
                <p className="revert-dialog-error" role="alert">
                  {state.message}
                </p>
              ) : null}
            </div>
            <div className="revert-dialog-actions">
              <button
                type="button"
                className="revert-dialog-cancel"
                onClick={close}
                disabled={submitting}
              >
                Cancel
              </button>
              <button
                type="button"
                className="revert-dialog-confirm"
                onClick={submit}
                disabled={submitting}
                aria-label="Confirm revert"
              >
                {submitting ? "Reverting…" : "Confirm revert"}
              </button>
            </div>
          </section>
        </>
      ) : null}
    </>
  );
}
