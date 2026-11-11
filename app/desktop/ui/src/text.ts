import type { PageMeta } from "./pages.ts";

function twoDigits(value: number): string {
  return String(value).padStart(2, "0");
}

/** `scan-YYYY-MM-DD.pdf` in local time, the day the user sees on their clock. */
export function defaultPdfName(date: Date): string {
  const month = twoDigits(date.getMonth() + 1);
  const day = twoDigits(date.getDate());
  return `scan-${date.getFullYear()}-${month}-${day}.pdf`;
}

export function pageCount(count: number): string {
  return count === 1 ? "1 page" : `${count} pages`;
}

export function pageDetails(page: PageMeta): string {
  return `${page.dpi} dpi · ${page.color ? "Colour" : "Grey"}`;
}

export function fileName(path: string): string {
  return path.split(/[/\\]/).pop() || path;
}

export function savedNotice(count: number, path: string): string {
  return `Saved ${pageCount(count)} to ${fileName(path)}`;
}
