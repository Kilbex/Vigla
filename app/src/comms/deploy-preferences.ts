import {
  DEFAULT_WORKER_MODELS,
  fillWorkerCliModels,
  fillWorkerModels,
  normalizeWorkerVendor,
  type SupervisorVendor,
  type WorkerCliModelValue,
  type WorkerCountChoice,
  type WorkerVendor,
} from "./deploy-models";

const PREFS_KEY = "vigla.deploy.prefs.v1";

export interface StoredPrefs {
  cwd?: string;
  supervisorModel?: string;
  workerModels?: string[];
  workerCliModels?: (string | null)[];
  /** Legacy v1 preference from the single shared worker selector. */
  workerModel?: string;
  workerCount?: string;
  /** Raw textarea contents are persisted verbatim; validation runs on Start. */
  scopePathsText?: string;
}

const SUPERVISOR_VALUES = new Set<string>(["claude"]);
const COUNT_VALUES = new Set<string>(["auto", "1", "2", "3", "4", "5"]);

export function loadPrefs(): StoredPrefs {
  if (typeof window === "undefined" || !window.localStorage) return {};
  try {
    const raw = window.localStorage.getItem(PREFS_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (parsed && typeof parsed === "object") return parsed as StoredPrefs;
  } catch {
    // Corrupted entry — fall through to empty defaults.
  }
  return {};
}

export function savePrefs(prefs: StoredPrefs): void {
  if (typeof window === "undefined" || !window.localStorage) return;
  try {
    window.localStorage.setItem(PREFS_KEY, JSON.stringify(prefs));
  } catch {
    // Quota / privacy mode — preferences must never break mission setup.
  }
}

export function initialSupervisor(prefs: StoredPrefs): SupervisorVendor {
  return prefs.supervisorModel && SUPERVISOR_VALUES.has(prefs.supervisorModel)
    ? (prefs.supervisorModel as SupervisorVendor)
    : "claude";
}

export function initialWorkerModels(prefs: StoredPrefs): WorkerVendor[] {
  if (Array.isArray(prefs.workerModels)) {
    const parsed = prefs.workerModels
      .map(normalizeWorkerVendor)
      .filter((vendor): vendor is WorkerVendor => vendor !== null);
    if (parsed.length > 0) return fillWorkerModels(parsed);
  }
  const legacy = normalizeWorkerVendor(prefs.workerModel);
  if (legacy) return fillWorkerModels([legacy]);
  return fillWorkerModels(DEFAULT_WORKER_MODELS);
}

export function initialWorkerCliModels(
  prefs: StoredPrefs,
  workerModels: readonly WorkerVendor[],
): WorkerCliModelValue[] {
  return fillWorkerCliModels(
    Array.isArray(prefs.workerCliModels) ? prefs.workerCliModels : [],
    workerModels,
  );
}

export function initialCount(prefs: StoredPrefs): WorkerCountChoice {
  return prefs.workerCount && COUNT_VALUES.has(prefs.workerCount)
    ? (prefs.workerCount as WorkerCountChoice)
    : "auto";
}

export function initialScopePathsText(prefs: StoredPrefs): string {
  return typeof prefs.scopePathsText === "string" ? prefs.scopePathsText : "";
}
