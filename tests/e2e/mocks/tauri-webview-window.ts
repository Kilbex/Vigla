// Browser-side mock of `@tauri-apps/api/webviewWindow`. The
// generated bindings only consume the `WebviewWindow` type at
// build time, but the import must resolve at runtime under
// Vite's ESM resolver. Provide just enough of the surface for
// the type system + any accidental constructor call.

export class WebviewWindow {
  label: string;
  constructor(label: string, _options?: unknown) {
    this.label = label;
  }
  static getByLabel(_label: string): WebviewWindow | null {
    return null;
  }
  async listen(): Promise<() => void> {
    return () => {};
  }
  async once(): Promise<() => void> {
    return () => {};
  }
  async emit(): Promise<void> {
    /* noop */
  }
}

export type EventCallback<T> = (event: { payload: T }) => void;
