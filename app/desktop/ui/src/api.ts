import { createMockBackend } from "./mock.ts";
import type { PageMeta } from "./pages.ts";
import type { TauriGlobal } from "./tauri.d.ts";

export type Dpi = 150 | 300 | 600;

// Set by `bun build --define`: true only in `build:mock`, so release bundles carry no mock.
declare const SCAN_UI_MOCK: boolean;

/** Everything the UI asks of the desktop process. Commands reject with one-sentence messages. */
export interface Backend {
  initialPages(): readonly PageMeta[];
  printerConnected(): Promise<boolean>;
  scanPage(color: boolean, dpi: Dpi): Promise<PageMeta>;
  cancelScan(): Promise<void>;
  deletePage(id: number): Promise<void>;
  reorderPages(ids: readonly number[]): Promise<void>;
  savePdf(path: string): Promise<number>;
  quit(): Promise<void>;
  chooseSavePath(defaultName: string): Promise<string | null>;
  confirm(message: string, okLabel: string): Promise<boolean>;
  onConfirmClose(handler: () => void): Promise<void>;
  pageImageUrl(id: number): string;
}

/** Tauri commands reject with the backend's `String` error; anything else is a bug. */
export function errorMessage(error: unknown): string {
  if (typeof error === "string" && error.trim() !== "") {
    return error;
  }
  if (error instanceof Error && error.message !== "") {
    return error.message;
  }
  return `Unexpected error (${String(error)}). Restart the app and try again.`;
}

function tauriBackend(tauri: TauriGlobal): Backend {
  const invoke = tauri.core.invoke;
  return {
    // The desktop process starts every window with no pages; only the mock preloads some.
    initialPages: () => [],
    printerConnected: () => invoke<boolean>("printer_connected"),
    scanPage: (color, dpi) => invoke<PageMeta>("scan_page", { color, dpi }),
    cancelScan: () => invoke<void>("cancel_scan"),
    deletePage: (id) => invoke<void>("delete_page", { id }),
    reorderPages: (ids) => invoke<void>("reorder_pages", { ids }),
    savePdf: (path) => invoke<number>("save_pdf", { path }),
    quit: () => invoke<void>("quit"),
    chooseSavePath: async (defaultName) => {
      const defaultPath = await tauri.path.join(await tauri.path.documentDir(), defaultName);
      return tauri.dialog.save({
        title: "Save PDF",
        defaultPath,
        filters: [{ name: "PDF document", extensions: ["pdf"] }],
      });
    },
    confirm: (message, okLabel) =>
      tauri.dialog.ask(message, { title: "M130a Scan", kind: "warning", okLabel }),
    onConfirmClose: async (handler) => {
      await tauri.event.listen<null>("confirm-close", handler);
    },
    pageImageUrl: (id) => `page://localhost/${id}`,
  };
}

/** In a `build:mock` bundle opened in a plain browser, the in-memory mock stands in. */
export function createBackend(): Backend {
  const tauri = window.__TAURI__;
  if (tauri !== undefined) {
    return tauriBackend(tauri);
  }
  if (SCAN_UI_MOCK) {
    return createMockBackend(new URLSearchParams(window.location.search));
  }
  throw new Error(
    "The app's Tauri API is missing; enable app.withGlobalTauri in tauri.conf.json and rebuild.",
  );
}
