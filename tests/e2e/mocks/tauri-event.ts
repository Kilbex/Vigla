// Browser-side mock of `@tauri-apps/api/event`. Provides
// `listen` / `once` / `emit` backed by an in-process channel
// map stored on `window.__viglaE2eListeners`. The test
// helpers on `window.__viglaE2e` (installed by
// `./tauri-core.ts`) forward emit() through the same map.

type Listener = (payload: unknown) => void;

declare global {
  interface Window {
    __viglaE2eListeners?: Map<string, Set<Listener>>;
  }
}

function table(): Map<string, Set<Listener>> {
  if (!window.__viglaE2eListeners) {
    window.__viglaE2eListeners = new Map();
  }
  return window.__viglaE2eListeners;
}

export type UnlistenFn = () => void;
export interface Event<T> {
  event: string;
  id: number;
  payload: T;
}

export async function listen<T = unknown>(
  event: string,
  handler: (e: Event<T>) => void,
): Promise<UnlistenFn> {
  const map = table();
  let set = map.get(event);
  if (!set) {
    set = new Set();
    map.set(event, set);
  }
  const wrapped: Listener = (payload) => {
    handler(payload as Event<T>);
  };
  set.add(wrapped);
  return () => {
    set?.delete(wrapped);
  };
}

export async function once<T = unknown>(
  event: string,
  handler: (e: Event<T>) => void,
): Promise<UnlistenFn> {
  const unlisten = await listen<T>(event, (e) => {
    unlisten();
    handler(e);
  });
  return unlisten;
}

export async function emit(event: string, payload?: unknown): Promise<void> {
  const map = table();
  const set = map.get(event);
  if (!set) return;
  for (const fn of set) {
    try {
      fn({ event, id: 0, payload });
    } catch {
      // listener threw — swallow per real-tauri semantics
    }
  }
}

export async function emitTo(
  _target: unknown,
  event: string,
  payload?: unknown,
): Promise<void> {
  return emit(event, payload);
}

export const TauriEvent = {
  WINDOW_RESIZED: "tauri://resize",
  WINDOW_MOVED: "tauri://move",
  WINDOW_CLOSE_REQUESTED: "tauri://close-requested",
  WINDOW_CREATED: "tauri://window-created",
  WINDOW_DESTROYED: "tauri://destroyed",
  WINDOW_FOCUS: "tauri://focus",
  WINDOW_BLUR: "tauri://blur",
  WINDOW_SCALE_FACTOR_CHANGED: "tauri://scale-change",
  WINDOW_THEME_CHANGED: "tauri://theme-changed",
} as const;
