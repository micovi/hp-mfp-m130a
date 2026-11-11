// A stand-in for the desktop process so the UI can be developed and screenshotted in a plain
// browser. Bundled only by `bun run build:mock`, used only when `window.__TAURI__` is missing:
// `pages=N` preloads N synthetic pages, `offline` starts disconnected, `fail` makes scans fail,
// `scanMs=N` sets the scan duration (default 2000). `window.mockScanner` toggles the rest.
import type { Backend, Dpi } from "./api.ts";
import type { PageMeta } from "./pages.ts";

interface MockOptions {
  readonly preload: number;
  readonly connected: boolean;
  readonly failScans: boolean;
  readonly scanMs: number;
}

interface PendingScan {
  readonly timer: number;
  readonly reject: (reason: string) => void;
}

const A4_INCHES = { width: 8.27, height: 11.69 };
const SHEET = { width: 620, height: 877 };
// Drawn at twice the layout size so the large preview can be judged for sharpness.
const SHEET_SCALE = 2;

function readOptions(params: URLSearchParams): MockOptions {
  return {
    preload: Number(params.get("pages") ?? "0"),
    connected: !params.has("offline"),
    failScans: params.has("fail"),
    scanMs: Number(params.get("scanMs") ?? "2000"),
  };
}

function textLines(ctx: CanvasRenderingContext2D, top: number, count: number, seed: number) {
  ctx.fillStyle = "#9a9a9a";
  for (let i = 0; i < count; i += 1) {
    const last = i === count - 1;
    const width = last ? 180 + ((seed * 37 + i * 53) % 160) : 470 - ((seed * 13 + i * 29) % 60);
    ctx.fillRect(72, top + i * 15, width, 6);
  }
}

function letterhead(ctx: CanvasRenderingContext2D, color: boolean, title: string) {
  ctx.fillStyle = color ? "#1d5c96" : "#3c3c3c";
  ctx.fillRect(72, 64, 34, 34);
  ctx.fillStyle = "#2b2b2b";
  ctx.font = "600 15px Helvetica, Arial, sans-serif";
  ctx.fillText("Northfield Housing Association", 118, 80);
  ctx.font = "11px Helvetica, Arial, sans-serif";
  ctx.fillStyle = "#6a6a6a";
  ctx.fillText("14 Mill Lane · Bristol BS1 4QA", 118, 96);
  ctx.fillStyle = "#2b2b2b";
  ctx.font = "600 20px Helvetica, Arial, sans-serif";
  ctx.fillText(title, 72, 150);
}

function table(ctx: CanvasRenderingContext2D, top: number, rows: number, color: boolean) {
  ctx.fillStyle = color ? "#e3edf7" : "#e6e6e6";
  ctx.fillRect(72, top, 476, 22);
  ctx.strokeStyle = "#cfcfcf";
  for (let i = 0; i <= rows; i += 1) {
    const y = top + 22 + i * 22;
    ctx.beginPath();
    ctx.moveTo(72, y);
    ctx.lineTo(548, y);
    ctx.stroke();
    if (i < rows) {
      ctx.fillStyle = "#9a9a9a";
      ctx.fillRect(80, y + 8, 200 - ((i * 41) % 90), 6);
      ctx.fillRect(470, y + 8, 60, 6);
    }
  }
}

function drawSheet(index: number, color: boolean): string {
  const canvas = document.createElement("canvas");
  canvas.width = SHEET.width * SHEET_SCALE;
  canvas.height = SHEET.height * SHEET_SCALE;
  const ctx = canvas.getContext("2d");
  if (ctx === null) {
    throw new Error("The mock scanner needs a 2D canvas to draw synthetic pages");
  }
  ctx.fillStyle = "#fbfaf6";
  ctx.scale(SHEET_SCALE, SHEET_SCALE);
  ctx.fillRect(0, 0, SHEET.width, SHEET.height);
  const titles = ["Tenancy agreement", "Rent statement", "Inspection report", "Notice of works"];
  letterhead(ctx, color, titles[index % titles.length] ?? "Document");
  textLines(ctx, 180, 9, index);
  if (index % 2 === 1) {
    table(ctx, 330, 8, color);
    textLines(ctx, 560, 6, index + 3);
  } else {
    textLines(ctx, 340, 12, index + 1);
    textLines(ctx, 540, 7, index + 2);
  }
  ctx.fillStyle = "#8a8a8a";
  ctx.font = "10px Helvetica, Arial, sans-serif";
  ctx.fillText(`Synthetic page ${index + 1}`, 72, 830);
  return canvas.toDataURL("image/png");
}

function pageMeta(id: number, color: boolean, dpi: Dpi): PageMeta {
  return {
    id,
    width_px: Math.round(A4_INCHES.width * dpi),
    height_px: Math.round(A4_INCHES.height * dpi),
    dpi,
    color,
  };
}

function isPermutation(ids: readonly number[], current: readonly number[]): boolean {
  const sorted = (list: readonly number[]) => [...list].sort((a, b) => a - b).join(",");
  return ids.length === current.length && sorted(ids) === sorted(current);
}

class MockScanner {
  private connected: boolean;
  private nextId = 1;
  private order: number[] = [];
  private readonly images = new Map<number, string>();
  private pending: PendingScan | null = null;
  private closeHandler: (() => void) | null = null;

  constructor(private readonly options: MockOptions) {
    this.connected = options.connected;
  }

  get isConnected(): boolean {
    return this.connected;
  }

  setConnected(connected: boolean): void {
    this.connected = connected;
  }

  requestClose(): void {
    this.closeHandler?.();
  }

  setCloseHandler(handler: () => void): void {
    this.closeHandler = handler;
  }

  addPage(color: boolean, dpi: Dpi): PageMeta {
    const id = this.nextId;
    this.nextId += 1;
    this.images.set(id, drawSheet(this.order.length, color));
    this.order.push(id);
    return pageMeta(id, color, dpi);
  }

  scan(color: boolean, dpi: Dpi): Promise<PageMeta> {
    if (!this.connected) {
      return Promise.reject("The printer is not connected; plug it in by USB and turn it on.");
    }
    if (this.pending !== null) {
      return Promise.reject("A scan is already running; wait for it to finish or cancel it.");
    }
    return new Promise((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.pending = null;
        if (this.options.failScans) {
          reject("The scanner is busy; try again in a moment.");
        } else {
          resolve(this.addPage(color, dpi));
        }
      }, this.options.scanMs);
      this.pending = { timer, reject };
    });
  }

  cancel(): void {
    if (this.pending !== null) {
      window.clearTimeout(this.pending.timer);
      this.pending.reject("The scan was cancelled.");
      this.pending = null;
    }
  }

  delete(id: number): void {
    if (!this.images.delete(id)) {
      throw `Page ${id} does not exist; it may already have been deleted.`;
    }
    this.order = this.order.filter((other) => other !== id);
  }

  reorder(ids: readonly number[]): void {
    if (!isPermutation(ids, this.order)) {
      throw "The new page order does not match the scanned pages; reopen the app.";
    }
    this.order = [...ids];
  }

  image(id: number): string {
    return this.images.get(id) ?? "";
  }

  get pageTotal(): number {
    return this.order.length;
  }
}

export function createMockBackend(params: URLSearchParams): Backend {
  const options = readOptions(params);
  const scanner = new MockScanner(options);
  Reflect.set(window, "mockScanner", scanner);
  const preloaded = Array.from({ length: options.preload }, (_, i) =>
    scanner.addPage(i % 3 !== 2, 300),
  );
  return {
    initialPages: () => preloaded,
    printerConnected: async () => scanner.isConnected,
    scanPage: (color, dpi) => scanner.scan(color, dpi),
    cancelScan: async () => scanner.cancel(),
    deletePage: async (id) => scanner.delete(id),
    reorderPages: async (ids) => scanner.reorder(ids),
    savePdf: async () => scanner.pageTotal,
    quit: async () => window.close(),
    chooseSavePath: async (name) => `/Users/you/Documents/${name}`,
    confirm: async (message) => window.confirm(message),
    onConfirmClose: async (handler) => scanner.setCloseHandler(handler),
    pageImageUrl: (id) => scanner.image(id),
  };
}
