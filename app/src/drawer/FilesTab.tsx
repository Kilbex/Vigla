import { useMemo } from "react";
import type { Event } from "../bindings";
import DiffViewer from "./DiffViewer";
import { FilesStrip } from "./Strips";

interface FilesTabProps {
  events: Event[];
  workerId: string | null;
}

/// Batch 2 — Files tab showing both file activity and unified diff
export default function FilesTab({ events, workerId }: FilesTabProps) {
  const { fileEvents, filesAdded, filesModified, filesDeleted, diffRevision } =
    useMemo(() => {
      const fileEvents: Array<Extract<Event, { type: "file_activity" }>> = [];
      let filesAdded = 0;
      let filesModified = 0;
      let filesDeleted = 0;
      let diffRevision = 0;
      for (const event of events) {
        if (event.type === "file_activity") {
          fileEvents.push(event);
          if (event.payload.op === "created") filesAdded += 1;
          if (event.payload.op === "modified") filesModified += 1;
          if (event.payload.op === "deleted") filesDeleted += 1;
          diffRevision = Math.max(diffRevision, event.seq);
        } else if (
          event.type === "completion" ||
          event.type === "failure" ||
          (event.type === "state_change" &&
            (event.payload.state === "done" || event.payload.state === "failed"))
        ) {
          diffRevision = Math.max(diffRevision, event.seq);
        }
      }
      return {
        fileEvents,
        filesAdded,
        filesModified,
        filesDeleted,
        diffRevision,
      };
    }, [events]);

  if (!workerId) {
    return <div className="drawer-empty">no worker selected</div>;
  }

  return (
    <div className="drawer-files-tab">
      {fileEvents.length > 0 && (
        <div className="drawer-files-header">
          {filesAdded > 0 && (
            <div className="drawer-files-stat">
              <div className="drawer-files-stat-label">Added</div>
              <div className="drawer-files-stat-value drawer-files-stat-value--added">
                {filesAdded}
              </div>
            </div>
          )}
          {filesModified > 0 && (
            <div className="drawer-files-stat">
              <div className="drawer-files-stat-label">Modified</div>
              <div className="drawer-files-stat-value drawer-files-stat-value--modified">
                {filesModified}
              </div>
            </div>
          )}
          {filesDeleted > 0 && (
            <div className="drawer-files-stat">
              <div className="drawer-files-stat-label">Deleted</div>
              <div className="drawer-files-stat-value drawer-files-stat-value--deleted">
                {filesDeleted}
              </div>
            </div>
          )}
        </div>
      )}

      <div className="drawer-files-section">
        <h3 className="drawer-section-title">File Activity</h3>
        <FilesStrip events={fileEvents} />
      </div>

      <div className="drawer-files-section">
        <h3 className="drawer-section-title">Unified Diff</h3>
        <DiffViewer workerId={workerId} revision={diffRevision} />
      </div>
    </div>
  );
}
