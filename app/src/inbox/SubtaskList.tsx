// S10 — per-task accept/scrub status. Reads the active mission's
// `tasks: MissionTask[]` slice and renders one row per task with
// its integration status. Pure presentation.

import type { MissionTask, TaskStatus } from "../missions/types";

interface SubtaskListProps {
  tasks: MissionTask[];
}

const STATUS_LABEL: Record<TaskStatus, string> = {
  pending: "Pending",
  in_progress: "In progress",
  under_review: "Reviewing",
  integrated: "Integrated",
  failed: "Failed",
};

export default function SubtaskList({ tasks }: SubtaskListProps) {
  if (tasks.length === 0) {
    return (
      <div className="subtask-list">
        <div className="mission-inbox-empty">no subtasks</div>
      </div>
    );
  }
  return (
    <div className="subtask-list" role="list" aria-label="Subtasks">
      {tasks.map((t) => (
        <div key={t.index} className="subtask-row" role="listitem">
          <span className="subtask-row-index">{t.index + 1}.</span>
          <span className="subtask-row-title">{t.title}</span>
          <span
            className={[
              "subtask-row-status",
              `subtask-row-status--${t.status}`,
            ].join(" ")}
            aria-label={`Task status: ${STATUS_LABEL[t.status]}`}
          >
            {STATUS_LABEL[t.status]}
          </span>
        </div>
      ))}
    </div>
  );
}
