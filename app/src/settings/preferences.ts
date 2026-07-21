// S3 — runtime preferences for the inbox-first UI. The defaults
// match the U7 acceptance criteria: silent by default, power-user
// surfaces opt-in.

import { useEffect, useState } from "react";

const SHOW_ALL_EVENTS_KEY = "vigla.prefs.show_all_events.v1";

type Listener = (value: boolean) => void;
const listeners = new Set<Listener>();

function readBool(key: string, fallback: boolean): boolean {
  if (typeof window === "undefined" || !window.localStorage) return fallback;
  try {
    const raw = window.localStorage.getItem(key);
    if (raw === null) return fallback;
    if (raw === "true") return true;
    if (raw === "false") return false;
    // Corrupted entry — return fallback rather than throw.
    return fallback;
  } catch {
    return fallback;
  }
}

function writeBool(key: string, value: boolean): void {
  if (typeof window === "undefined" || !window.localStorage) return;
  try {
    window.localStorage.setItem(key, value ? "true" : "false");
  } catch {
    // Quota / privacy mode — silent no-op so the UI doesn't
    // break on preference write.
  }
}

/**
 * Read the current "Show all events" preference. Default = false
 * (silent-by-default per the U7 acceptance criteria).
 */
export function getShowAllEvents(): boolean {
  return readBool(SHOW_ALL_EVENTS_KEY, false);
}

/**
 * Set the preference and notify subscribers. Used by the
 * useShowAllEvents hook below + any future settings UI.
 */
export function setShowAllEvents(value: boolean): void {
  writeBool(SHOW_ALL_EVENTS_KEY, value);
  for (const listener of listeners) listener(value);
}

/**
 * React hook returning the live preference + a setter. Re-renders
 * the consuming component when any caller flips the preference.
 */
export function useShowAllEvents(): [boolean, (v: boolean) => void] {
  const [value, setValue] = useState<boolean>(() => getShowAllEvents());

  useEffect(() => {
    const listener = (v: boolean) => setValue(v);
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }, []);

  return [value, setShowAllEvents];
}

// S10 — Notify-on-completion (macOS banner) preference. Default
// true; the banner only fires when the window is unfocused so a
// silent default is unnecessary.
const NOTIFY_ON_COMPLETION_KEY = "vigla.prefs.notify_on_completion.v1"; // gitleaks:allow
const completionListeners = new Set<Listener>();

export function getNotifyOnCompletion(): boolean {
  return readBool(NOTIFY_ON_COMPLETION_KEY, true);
}

export function setNotifyOnCompletion(value: boolean): void {
  writeBool(NOTIFY_ON_COMPLETION_KEY, value);
  for (const listener of completionListeners) listener(value);
}

export function useNotifyOnCompletion(): [boolean, (v: boolean) => void] {
  const [value, setValue] = useState<boolean>(() => getNotifyOnCompletion());

  useEffect(() => {
    const listener = (v: boolean) => setValue(v);
    completionListeners.add(listener);
    return () => {
      completionListeners.delete(listener);
    };
  }, []);

  return [value, setNotifyOnCompletion];
}

// QC-3 — Mission Pre-Planning default.
//
// 'direct' (default): the supervisor publishes a plan and proceeds
//   immediately within the four-bound envelope. The plan is still
//   recorded and visible in Ops Room / History, but does not gate.
// 'review':           one explicit user touch before workers spawn.
//   Equivalent to setting `MissionSpec.confirm_plan = true`.
//
// Envelope-fit `Exceeds` still forces a pause even in Direct mode —
// the supervisor's self-assessment overrides the user's default.

export type PlanMode = "direct" | "review";

const PLAN_MODE_KEY = "vigla.prefs.plan_mode.v1";
type PlanModeListener = (value: PlanMode) => void;
const planModeListeners = new Set<PlanModeListener>();

export function getPlanMode(): PlanMode {
  if (typeof window === "undefined" || !window.localStorage) return "direct";
  try {
    const raw = window.localStorage.getItem(PLAN_MODE_KEY);
    return raw === "review" ? "review" : "direct";
  } catch {
    return "direct";
  }
}

export function setPlanMode(value: PlanMode): void {
  if (typeof window !== "undefined" && window.localStorage) {
    try {
      window.localStorage.setItem(PLAN_MODE_KEY, value);
    } catch {
      // Quota / privacy mode — silent no-op so the UI doesn't
      // break on preference write.
    }
  }
  for (const listener of planModeListeners) listener(value);
}

export function usePlanMode(): [PlanMode, (v: PlanMode) => void] {
  const [value, setValue] = useState<PlanMode>(() => getPlanMode());

  useEffect(() => {
    const listener: PlanModeListener = (v) => setValue(v);
    planModeListeners.add(listener);
    return () => {
      planModeListeners.delete(listener);
    };
  }, []);

  return [value, setPlanMode];
}
