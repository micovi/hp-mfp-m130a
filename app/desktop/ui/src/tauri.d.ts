// The slice of Tauri's global API (`app.withGlobalTauri`) this UI uses, so it needs no npm
// runtime packages. Shapes follow @tauri-apps/api 2 and @tauri-apps/plugin-dialog 2.

export interface TauriEvent<T> {
  readonly event: string;
  readonly payload: T;
}

export interface TauriDialogFilter {
  readonly name: string;
  readonly extensions: readonly string[];
}

export interface TauriSaveOptions {
  readonly title?: string;
  readonly defaultPath?: string;
  readonly filters?: readonly TauriDialogFilter[];
}

export interface TauriAskOptions {
  readonly title?: string;
  readonly kind?: "info" | "warning" | "error";
  readonly okLabel?: string;
  readonly cancelLabel?: string;
}

export interface TauriGlobal {
  readonly core: {
    invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  };
  readonly event: {
    listen<T>(event: string, handler: (event: TauriEvent<T>) => void): Promise<() => void>;
  };
  readonly dialog: {
    save(options?: TauriSaveOptions): Promise<string | null>;
    ask(message: string, options?: TauriAskOptions): Promise<boolean>;
  };
  readonly path: {
    documentDir(): Promise<string>;
    join(...paths: string[]): Promise<string>;
  };
}

declare global {
  interface Window {
    readonly __TAURI__?: TauriGlobal;
  }
}
