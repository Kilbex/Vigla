import { useCallback, useSyncExternalStore } from "react";
import { commands } from "../bindings";
import type { Vendor } from "../bindings";

/// Module-level identity overlay for the Station tile (P1-6).
///
/// Standalone workers can enter the store before their authoritative
/// `WorkerInfo` row has been projected. This hook fetches that row lazily,
/// caches the result by id, and re-renders all consumers when an id resolves.
/// Mission `worker.spawned` events now carry identity directly, so their
/// stations disable this lookup rather than polling a repository they never
/// persist into.
///
/// Keeping the lookup out of the store reducer preserves purity: the
/// reducer stays a synchronous projection of events; identity overlay
/// is a UI-only concern.

export interface WorkerIdentity {
  vendor: Vendor;
  model: string | null;
}

type IdentityState =
  | { kind: "pending"; attempt: number; requestId: number }
  | { kind: "missing"; retryAt: number }
  | { kind: "failed"; retryAt: number; nextAttempt: number }
  | { kind: "resolved"; identity: WorkerIdentity };

// Transport failures get two bounded retries before a longer cooldown.
// A not-yet-persisted WorkerInfo row is cheaper to poll, but still receives
// a TTL so multiple station renders cannot create a tight request loop.
const TRANSPORT_RETRY_DELAYS_MS = [1_000, 5_000] as const;
const TRANSPORT_FAILURE_TTL_MS = 30_000;
const MISSING_TTL_MS = 10_000;

const cache: Map<string, IdentityState> = new Map();
const subscribers: Map<string, Set<() => void>> = new Map();
const retryTimers: Map<string, number> = new Map();
let requestSequence = 0;

function notify(workerId: string): void {
  const set = subscribers.get(workerId);
  if (!set) return;
  for (const fn of set) fn();
}

function clearRetryTimer(workerId: string): void {
  const timer = retryTimers.get(workerId);
  if (timer === undefined) return;
  window.clearTimeout(timer);
  retryTimers.delete(workerId);
}

function hasSubscribers(workerId: string): boolean {
  return (subscribers.get(workerId)?.size ?? 0) > 0;
}

function scheduleRetry(workerId: string, retryAt: number): void {
  clearRetryTimer(workerId);
  if (!hasSubscribers(workerId)) return;

  const timer = window.setTimeout(() => {
    retryTimers.delete(workerId);
    if (!hasSubscribers(workerId)) return;
    const current = cache.get(workerId);
    if (
      !current ||
      (current.kind !== "failed" && current.kind !== "missing") ||
      current.retryAt !== retryAt
    ) {
      return;
    }
    startFetch(workerId, current.kind === "failed" ? current.nextAttempt : 1);
  }, Math.max(0, retryAt - Date.now()));
  retryTimers.set(workerId, timer);
}

function isCurrentRequest(workerId: string, requestId: number): boolean {
  const current = cache.get(workerId);
  return current?.kind === "pending" && current.requestId === requestId;
}

function recordTransportFailure(
  workerId: string,
  requestId: number,
  attempt: number,
): void {
  if (!isCurrentRequest(workerId, requestId)) return;
  const retryDelay = TRANSPORT_RETRY_DELAYS_MS[attempt - 1];
  const retryAt =
    Date.now() + (retryDelay ?? TRANSPORT_FAILURE_TTL_MS);
  cache.set(workerId, {
    kind: "failed",
    retryAt,
    nextAttempt: retryDelay === undefined ? 1 : attempt + 1,
  });
  scheduleRetry(workerId, retryAt);
  notify(workerId);
}

function startFetch(workerId: string, attempt = 1): void {
  clearRetryTimer(workerId);
  const requestId = ++requestSequence;
  cache.set(workerId, { kind: "pending", attempt, requestId });

  let request: ReturnType<typeof commands.getWorkerInfo>;
  try {
    request = commands.getWorkerInfo(workerId);
  } catch {
    recordTransportFailure(workerId, requestId, attempt);
    return;
  }

  request
    .then((r) => {
      if (!isCurrentRequest(workerId, requestId)) return;
      if (r.status === "error") {
        // Tauri reports command/repository failures through the resolved
        // Result error branch. Treat those like transport failures so a
        // broken runtime cannot create an unbounded ten-second poll loop.
        recordTransportFailure(workerId, requestId, attempt);
        return;
      }
      if (r.data) {
        cache.set(workerId, {
          kind: "resolved",
          identity: { vendor: r.data.vendor, model: r.data.model },
        });
      } else {
        // A business-level "not found" (or the mock IPC null payload) is
        // distinct from a rejected transport. Cache it only for a TTL.
        const retryAt = Date.now() + MISSING_TTL_MS;
        cache.set(workerId, { kind: "missing", retryAt });
        scheduleRetry(workerId, retryAt);
      }
      notify(workerId);
    })
    .catch(() => {
      recordTransportFailure(workerId, requestId, attempt);
    });
}

function ensureFetch(workerId: string): void {
  const current = cache.get(workerId);
  if (!current) {
    startFetch(workerId);
    return;
  }
  if (current.kind !== "failed" && current.kind !== "missing") return;
  if (Date.now() >= current.retryAt) {
    startFetch(workerId, current.kind === "failed" ? current.nextAttempt : 1);
  } else {
    scheduleRetry(workerId, current.retryAt);
  }
}

/// Test-only escape hatch: drop every cached entry + subscriber map.
/// Component tests share the module-level cache across renders, so a
/// fresh test must clear prior pending fetches before mocking new ones.
export function __resetWorkerIdentityCache(): void {
  for (const timer of retryTimers.values()) window.clearTimeout(timer);
  retryTimers.clear();
  cache.clear();
  subscribers.clear();
}

/// Returns the resolved identity for a workerId, or null while the
/// fetch is in flight / never resolved. Subscribes the calling
/// component so a late resolution triggers a re-render.
export function useWorkerIdentity(
  workerId: string,
  enabled = true,
): WorkerIdentity | null {
  const subscribe = useCallback(
    (cb: () => void) => {
      if (!enabled) return () => {};
      let set = subscribers.get(workerId);
      if (!set) {
        set = new Set();
        subscribers.set(workerId, set);
      }
      set.add(cb);
      // Kick the fetch on first observer, not at module-eval time —
      // avoids needless IPC for workerIds never rendered.
      ensureFetch(workerId);
      return () => {
        set?.delete(cb);
        if (set && set.size === 0) {
          subscribers.delete(workerId);
          clearRetryTimer(workerId);
        }
      };
    },
    [enabled, workerId],
  );
  const getSnapshot = useCallback(
    (): IdentityState | null =>
      enabled ? (cache.get(workerId) ?? null) : null,
    [enabled, workerId],
  );
  const entry = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  if (!entry || entry.kind !== "resolved") return null;
  return entry.identity;
}
