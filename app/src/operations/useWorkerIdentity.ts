import { useSyncExternalStore } from "react";
import { commands } from "../bindings";
import type { Vendor } from "../bindings";

/// Module-level identity overlay for the Station tile (P1-6).
///
/// The store's `WorkerSnapshot` carries vendor/model only when seeded
/// via `registerWorker`. The live `Event` stream doesn't carry vendor,
/// so workers ingested purely from events stay stuck at the
/// `vendor: "mock"` default in `fresh()`. The orchestrator already
/// records authoritative identity in `WorkerInfo`; this hook fetches
/// it lazily, caches the result by id, and re-renders all consumers
/// when an id resolves.
///
/// Keeping the lookup out of the store reducer preserves purity: the
/// reducer stays a synchronous projection of events; identity overlay
/// is a UI-only concern.

export interface WorkerIdentity {
  vendor: Vendor;
  model: string | null;
}

type IdentityState =
  | { kind: "pending" }
  | { kind: "missing" }
  | { kind: "resolved"; identity: WorkerIdentity };

const cache: Map<string, IdentityState> = new Map();
const subscribers: Map<string, Set<() => void>> = new Map();

function notify(workerId: string): void {
  const set = subscribers.get(workerId);
  if (!set) return;
  for (const fn of set) fn();
}

function startFetch(workerId: string): void {
  cache.set(workerId, { kind: "pending" });
  commands
    .getWorkerInfo(workerId)
    .then((r) => {
      if (r.status === "ok" && r.data) {
        cache.set(workerId, {
          kind: "resolved",
          identity: { vendor: r.data.vendor, model: r.data.model },
        });
      } else {
        // null payload (mock IPC layer) or "error" — treat as missing.
        cache.set(workerId, { kind: "missing" });
      }
      notify(workerId);
    })
    .catch(() => {
      cache.set(workerId, { kind: "missing" });
      notify(workerId);
    });
}

/// Test-only escape hatch: drop every cached entry + subscriber map.
/// Component tests share the module-level cache across renders, so a
/// fresh test must clear prior pending fetches before mocking new ones.
export function __resetWorkerIdentityCache(): void {
  cache.clear();
  subscribers.clear();
}

/// Returns the resolved identity for a workerId, or null while the
/// fetch is in flight / never resolved. Subscribes the calling
/// component so a late resolution triggers a re-render.
export function useWorkerIdentity(workerId: string): WorkerIdentity | null {
  const subscribe = (cb: () => void) => {
    let set = subscribers.get(workerId);
    if (!set) {
      set = new Set();
      subscribers.set(workerId, set);
    }
    set.add(cb);
    // Kick the fetch on first observer, not at module-eval time —
    // avoids needless IPC for workerIds never rendered.
    if (!cache.has(workerId)) startFetch(workerId);
    return () => {
      set?.delete(cb);
      if (set && set.size === 0) subscribers.delete(workerId);
    };
  };
  const getSnapshot = (): IdentityState | null => cache.get(workerId) ?? null;
  const entry = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  if (!entry || entry.kind !== "resolved") return null;
  return entry.identity;
}
