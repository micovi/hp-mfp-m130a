import { type PageList, type PageMeta, selectBy, selectedIndex } from "./pages.ts";
import { pageDetails } from "./text.ts";

/** What a key does while the preview is open; anything else is swallowed. */
export type PreviewAction = "close" | "previous" | "next" | "trap-focus" | "ignore";

const ACTIONS: Readonly<Record<string, PreviewAction>> = {
  " ": "close",
  Escape: "close",
  ArrowLeft: "previous",
  ArrowUp: "previous",
  ArrowRight: "next",
  ArrowDown: "next",
  Tab: "trap-focus",
};

/** Keys with ⌘ or Ctrl do nothing in the preview, so it can never save or delete. */
export function previewAction(event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey">) {
  if (event.metaKey || event.ctrlKey) {
    return "ignore";
  }
  return ACTIONS[event.key] ?? "ignore";
}

/** The page the preview shows: always the selected one, so closing leaves it selected. */
export function previewPage(list: PageList): PageMeta | null {
  return list.pages[selectedIndex(list)] ?? null;
}

/** Steps one page in document order (↑/↓ too, not by grid row), stopping at the ends. */
export function stepPreview(list: PageList, delta: -1 | 1): PageList {
  return selectedIndex(list) === -1 ? list : selectBy(list, delta);
}

/** "Page 2 of 4 · 1240 × 1754 px · 150 dpi · Grey", or null with nothing to show. */
export function previewCaption(list: PageList): string | null {
  const page = previewPage(list);
  if (page === null) {
    return null;
  }
  const position = `Page ${selectedIndex(list) + 1} of ${list.pages.length}`;
  return `${position} · ${page.width_px} × ${page.height_px} px · ${pageDetails(page)}`;
}
