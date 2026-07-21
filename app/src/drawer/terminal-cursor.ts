import type { Event } from "../bindings";

/// Slice of `events` whose seq exceeds `lastSeen`, in arrival order.
/// Walks from the tail backwards so it remains O(new events written)
/// even after the per-worker log rotates at `MAX_EVENTS_PER_WORKER`
/// — an index-based cursor would freeze at length=cap and silently
/// stop printing once the rotation begins (regression target for C3).
/// `lastSeen === null` means first call: write everything.
export function eventsAfterSeq(events: Event[], lastSeen: number | null): Event[] {
  if (events.length === 0) return [];
  if (lastSeen === null) return events.slice();
  const tail: Event[] = [];
  for (let i = events.length - 1; i >= 0; i--) {
    if (events[i].seq <= lastSeen) break;
    tail.push(events[i]);
  }
  return tail.reverse();
}
