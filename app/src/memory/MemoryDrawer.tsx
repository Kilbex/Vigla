import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import {
  commands,
  type MemoryBundleDto,
  type MemoryEventDto,
  type MemoryEventKindDto,
  type MemoryNoteSummaryDto,
  type Result,
} from "../bindings";
import {
  selectCurrentRepoCwd,
  selectMissionId,
  useMissionsStore,
} from "../missions/store";
import { useDialogFocus } from "../useDialogFocus";

/**
 * Tier-2E read-only Memory drawer.
 *
 * Three sections:
 *
 *   1. **Attached memory** — the latest bundle composed for the
 *      active mission (note ids the most recent worker saw).
 *   2. **Recent proposals** — mission-scoped memory events
 *      (proposed / ratified / promoted / drift / barrier).
 *   3. **Promoted notes** — every note currently in `state=promoted`.
 *      Lives outside the mission scope; users see what's accumulated
 *      across the whole codex.
 *
 * Refresh strategy: poll every `POLL_INTERVAL_MS` *only while the
 * drawer is open*. Closing the drawer halts polling so the IPC chatter
 * is bounded by user attention, not wall-clock time.
 *
 * The drawer is side-by-side with the existing worker drawer (bottom
 * sheet) — it slides in from the right and does not obscure the
 * operations room. There is no scrim: users can keep dispatching
 * while the drawer is open. ESC closes.
 */
const POLL_INTERVAL_MS = 3000;
const RECENT_EVENTS_LIMIT = 50;
const PROMOTED_NOTES_LIMIT = 100;

interface Props {
  onClose: () => void;
}

export default function MemoryDrawer({ onClose }: Props) {
  const missionId = useMissionsStore(selectMissionId);
  // A2 (Tier-2G): cwd of the current repo. Memory commands need it
  // to resolve the per-repo kernel. When null (no mission yet this
  // session), all three sections render an explanatory empty state
  // and no IPC fires.
  const cwd = useMissionsStore(selectCurrentRepoCwd);

  const [bundle, setBundle] = useState<MemoryBundleDto | null>(null);
  const [events, setEvents] = useState<MemoryEventDto[]>([]);
  const [notes, setNotes] = useState<MemoryNoteSummaryDto[]>([]);
  const [error, setError] = useState<string | null>(null);

  const titleId = useId();
  const refreshSeq = useRef(0);
  const drawerRef = useRef<HTMLElement | null>(null);
  useDialogFocus(true, drawerRef, false);

  const refresh = useCallback(async () => {
    const seq = ++refreshSeq.current;
    if (!cwd) {
      // No repo selected — clear state and bail without IPC.
      setNotes([]);
      setBundle(null);
      setEvents([]);
      setError(null);
      return;
    }
    // Run the three queries in parallel. Each fails independently:
    // one section's IPC error doesn't blank the other two.
    const promotedPromise = safeCommand(
      commands.memoryListNotes(cwd, "promoted", PROMOTED_NOTES_LIMIT),
    );
    const bundlePromise = missionId
      ? safeCommand(commands.memoryLatestBundleForMission(cwd, missionId))
      : Promise.resolve({ status: "ok", data: null } as const);
    const eventsPromise = missionId
      ? safeCommand(
          commands.memoryRecentEventsForMission(cwd, missionId, RECENT_EVENTS_LIMIT),
        )
      : Promise.resolve({ status: "ok", data: [] as MemoryEventDto[] } as const);

    const [promotedRes, bundleRes, eventsRes] = await Promise.all([
      promotedPromise,
      bundlePromise,
      eventsPromise,
    ]);
    if (seq !== refreshSeq.current) return;

    let firstErr: string | null = null;
    if (promotedRes.status === "ok") setNotes(promotedRes.data);
    else firstErr ??= promotedRes.error;
    if (bundleRes.status === "ok") setBundle(bundleRes.data);
    else firstErr ??= bundleRes.error;
    if (eventsRes.status === "ok") setEvents(eventsRes.data);
    else firstErr ??= eventsRes.error;
    setError(firstErr);
  }, [missionId, cwd]);

  useEffect(() => {
    // Fire once on open, then on a steady tick. Cancel on unmount.
    let cancelled = false;
    const run = () => {
      void refresh().catch((e) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : String(e));
      });
    };
    run();
    const id = window.setInterval(run, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      refreshSeq.current += 1;
      window.clearInterval(id);
    };
  }, [refresh]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const noteById = useMemo(() => {
    const m = new Map<string, MemoryNoteSummaryDto>();
    for (const n of notes) m.set(n.id, n);
    return m;
  }, [notes]);

  return (
    <aside
      ref={drawerRef}
      className="memory-drawer"
      role="dialog"
      aria-labelledby={titleId}
      aria-modal="false"
    >
      <header className="memory-drawer-header">
        <h2 id={titleId} className="memory-drawer-title">
          Memory
        </h2>
        <button
          type="button"
          className="memory-drawer-close"
          onClick={onClose}
          aria-label="Close memory drawer"
        >
          ×
        </button>
      </header>

      {error ? (
        <div className="memory-drawer-error" role="alert">
          {error}
        </div>
      ) : null}

      <Section title="Attached memory" subtitle={subtitleForBundle(bundle, missionId)}>
        {bundle && bundle.note_ids.length > 0 ? (
          <ul className="memory-drawer-list">
            {bundle.note_ids.map((id) => (
              <li key={id} className="memory-drawer-item">
                <NoteRow id={id} noteById={noteById} />
              </li>
            ))}
          </ul>
        ) : (
          <EmptyHint>
            {missionId
              ? "No bundle composed for this mission yet."
              : "Start a mission to see what memory is attached."}
          </EmptyHint>
        )}
      </Section>

      <Section
        title="Recent proposals"
        subtitle={
          missionId
            ? events.length > 0
              ? `${events.length} event${events.length === 1 ? "" : "s"}`
              : null
            : "No active mission"
        }
      >
        {missionId && events.length > 0 ? (
          <ul className="memory-drawer-list">
            {events.map((ev) => (
              <li key={ev.event_id} className="memory-drawer-item">
                <EventRow event={ev} />
              </li>
            ))}
          </ul>
        ) : (
          <EmptyHint>
            {missionId
              ? "No proposals yet. Workers can propose by emitting an vigla_memory line."
              : "Start a mission and worker proposals will appear here."}
          </EmptyHint>
        )}
      </Section>

      <Section
        title="Promoted notes"
        subtitle={
          !cwd
            ? "No active repository"
            : notes.length > 0
              ? `${notes.length} promoted`
              : null
        }
      >
        {!cwd ? (
          <EmptyHint>Start a mission — memory is per-repository.</EmptyHint>
        ) : notes.length > 0 ? (
          <ul className="memory-drawer-list">
            {notes.map((n) => (
              <li key={n.id} className="memory-drawer-item">
                <NoteSummaryRow note={n} />
              </li>
            ))}
          </ul>
        ) : (
          <EmptyHint>Pin a note above or accept a mission to grow memory.</EmptyHint>
        )}
      </Section>
    </aside>
  );
}

async function safeCommand<T>(promise: Promise<Result<T, string>>): Promise<Result<T, string>> {
  try {
    return await promise;
  } catch (e) {
    return {
      status: "error",
      error: e instanceof Error ? e.message : String(e),
    };
  }
}

// ---------------------------------------------------------------------
// Subcomponents — kept inside the same file because the drawer is the
// only consumer. Promote to siblings when a second surface emerges.
// ---------------------------------------------------------------------

function Section({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle?: string | null;
  children: React.ReactNode;
}) {
  return (
    <section className="memory-drawer-section">
      <div className="memory-drawer-section-head">
        <h3 className="memory-drawer-section-title">{title}</h3>
        {subtitle ? (
          <span className="memory-drawer-section-subtitle">{subtitle}</span>
        ) : null}
      </div>
      <div className="memory-drawer-section-body">{children}</div>
    </section>
  );
}

function EmptyHint({ children }: { children: React.ReactNode }) {
  return <p className="memory-drawer-empty">{children}</p>;
}

function NoteRow({
  id,
  noteById,
}: {
  id: string;
  noteById: Map<string, MemoryNoteSummaryDto>;
}) {
  // The bundle carries note ids; we cross-reference into the
  // promoted-notes list to render kind + scope inline. If the note
  // isn't promoted (transient owned-state), we still surface the id
  // so the user has something to scan.
  const note = noteById.get(id);
  if (!note) {
    return (
      <div className="memory-drawer-row">
        <span className="memory-drawer-row-kind">note</span>
        <span className="memory-drawer-row-id" title={id}>
          {shortId(id)}
        </span>
      </div>
    );
  }
  return <NoteSummaryRow note={note} />;
}

function NoteSummaryRow({ note }: { note: MemoryNoteSummaryDto }) {
  return (
    <div className="memory-drawer-row">
      <span className={`memory-drawer-row-kind memory-drawer-kind-${note.kind}`}>
        {note.kind}
      </span>
      <span className="memory-drawer-row-scope">
        {note.scope_kind === "repo" ? "repo" : `${note.scope_kind}:${note.scope_value ?? ""}`}
      </span>
      <span className="memory-drawer-row-id" title={note.id}>
        {shortId(note.id)}
      </span>
    </div>
  );
}

function EventRow({ event }: { event: MemoryEventDto }) {
  const { label, detail } = describeEvent(event.kind);
  return (
    <div className="memory-drawer-row memory-drawer-event-row">
      <span className="memory-drawer-event-label">{label}</span>
      {detail ? <span className="memory-drawer-event-detail">{detail}</span> : null}
      <span className="memory-drawer-event-ts" title={event.ts}>
        {shortTs(event.ts)}
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------
// Pure formatters — no React, no IPC. Easy to unit-test from outside.
// ---------------------------------------------------------------------

export function describeEvent(kind: MemoryEventKindDto): {
  label: string;
  detail: string | null;
} {
  switch (kind.type) {
    case "proposed":
      return {
        label: `proposed ${kind.kind}`,
        detail: kind.body_preview,
      };
    case "proposal_rejected":
      return { label: "proposal rejected", detail: kind.reason };
    case "normalized":
      return { label: "supervisor normalized proposal", detail: null };
    case "ratified":
      return {
        label: `supervisor ${kind.decision}`,
        detail: kind.note_id ? `→ ${shortId(kind.note_id)}` : null,
      };
    case "rejected":
      return { label: "supervisor rejected", detail: kind.reason };
    case "promoted":
      return {
        label: "learned after accept",
        detail: `note ${shortId(kind.note_id)} · conf ${kind.confidence.toFixed(2)}`,
      };
    case "barrier":
      return { label: `mission ${kind.kind}`, detail: null };
    case "bundle_composed":
      return {
        label: `bundle composed (${kind.note_count} note${kind.note_count === 1 ? "" : "s"})`,
        detail: `worker ${shortId(kind.worker_id)} · turn ${kind.turn}`,
      };
    case "bundle_rendered":
      return { label: "bundle written to worktree", detail: null };
    case "drift_detected":
      return { label: "memory drift detected", detail: shortId(kind.bundle_id) };
    case "other":
      return { label: kind.event_type, detail: null };
  }
}

export function shortId(id: string): string {
  if (id.length <= 12) return id;
  return `${id.slice(0, 8)}…`;
}

export function shortTs(ts: string): string {
  // ISO 8601 → HH:MM:SS local-clock-ish. We pull the time component
  // straight from the canonical RFC 3339 string the kernel emits;
  // this is good enough for "what just happened" UX. A future
  // iteration can localise per the user's timezone setting.
  const match = ts.match(/T(\d{2}:\d{2}:\d{2})/);
  return match ? match[1] : ts;
}

function subtitleForBundle(
  bundle: MemoryBundleDto | null,
  missionId: string | null,
): string | null {
  if (!missionId) return "No active mission";
  if (!bundle) return null;
  const count = bundle.note_ids.length;
  return `turn ${bundle.turn} · ${count} note${count === 1 ? "" : "s"} · ${bundle.vendor}`;
}
