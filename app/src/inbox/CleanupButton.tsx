import { useCallback, useRef, useState } from "react";
import { commands } from "../bindings";
import { useDialogFocus } from "../useDialogFocus";

interface CleanupButtonProps {
  missionId: string;
  disabled?: boolean;
  onCleaned?: () => void;
}

type DialogState =
  | { kind: "idle" }
  | { kind: "confirming" }
  | { kind: "submitting" }
  | { kind: "error"; message: string }
  | { kind: "complete" };

export default function CleanupButton({
  missionId,
  disabled,
  onCleaned,
}: CleanupButtonProps) {
  const [state, setState] = useState<DialogState>({ kind: "idle" });
  const open = useCallback(() => {
    if (!disabled) setState({ kind: "confirming" });
  }, [disabled]);
  const close = useCallback(() => setState({ kind: "idle" }), []);
  const submit = useCallback(() => {
    setState({ kind: "submitting" });
    commands
      .cleanupMissionArtifacts(missionId)
      .then((result) => {
        if (result.status === "ok") {
          setState({ kind: "complete" });
          onCleaned?.();
        } else {
          setState({ kind: "error", message: result.error });
        }
      })
      .catch((err: unknown) => {
        setState({
          kind: "error",
          message: typeof err === "string" ? err : String(err),
        });
      });
  }, [missionId, onCleaned]);

  const submitting = state.kind === "submitting";
  const dialogRef = useRef<HTMLElement>(null);
  useDialogFocus(
    state.kind === "confirming" ||
      state.kind === "submitting" ||
      state.kind === "error",
    dialogRef,
  );

  if (state.kind === "complete") {
    return <span role="status">Artifacts cleaned.</span>;
  }

  return (
    <>
      <button
        type="button"
        className="revert-button"
        onClick={open}
        disabled={disabled || submitting}
        aria-busy={submitting}
        aria-label="Clean up mission artifacts"
      >
        Clean up artifacts
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
            aria-labelledby="cleanup-dialog-title"
            ref={dialogRef}
          >
            <h2 id="cleanup-dialog-title" className="revert-dialog-title">
              Clean up mission artifacts?
            </h2>
            <div className="revert-dialog-body">
              <p>
                This permanently removes the aborted mission&apos;s Vigla
                worktrees, branches, and intermediate snapshot tags. The target
                branch and its commits are unchanged.
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
                aria-label="Confirm artifact cleanup"
              >
                {submitting ? "Cleaning up…" : "Clean up artifacts"}
              </button>
            </div>
          </section>
        </>
      ) : null}
    </>
  );
}
