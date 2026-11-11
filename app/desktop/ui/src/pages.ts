export interface PageMeta {
  readonly id: number;
  readonly width_px: number;
  readonly height_px: number;
  readonly dpi: number;
  readonly color: boolean;
}

export interface PageList {
  readonly pages: readonly PageMeta[];
  readonly selectedId: number | null;
}

export const emptyList: PageList = { pages: [], selectedId: null };

function indexOf(list: PageList, id: number): number {
  return list.pages.findIndex((page) => page.id === id);
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

export function pageIds(list: PageList): number[] {
  return list.pages.map((page) => page.id);
}

export function selectedIndex(list: PageList): number {
  return list.selectedId === null ? -1 : indexOf(list, list.selectedId);
}

/** Appends a freshly scanned page and selects it, so the newest page is the one in focus. */
export function addPage(list: PageList, page: PageMeta): PageList {
  if (indexOf(list, page.id) !== -1) {
    throw new Error(`Page ${page.id} is already in the list`);
  }
  return { pages: [...list.pages, page], selectedId: page.id };
}

/**
 * Removes a page. When it was selected, the selection moves to the page that took its place,
 * or to the new last page, so repeated Delete presses keep working through the list.
 */
export function removePage(list: PageList, id: number): PageList {
  const index = indexOf(list, id);
  if (index === -1) {
    return list;
  }
  const pages = list.pages.filter((page) => page.id !== id);
  if (list.selectedId !== id) {
    return { pages, selectedId: list.selectedId };
  }
  const neighbour = pages[Math.min(index, pages.length - 1)];
  return { pages, selectedId: neighbour?.id ?? null };
}

/** Moves a page to `target`, clamped to the list; the page stays selected. */
export function movePageTo(list: PageList, id: number, target: number): PageList {
  const from = indexOf(list, id);
  if (from === -1) {
    return list;
  }
  const to = clamp(target, 0, list.pages.length - 1);
  if (to === from) {
    return list;
  }
  const pages = [...list.pages];
  const [page] = pages.splice(from, 1);
  if (page === undefined) {
    return list;
  }
  pages.splice(to, 0, page);
  return { pages, selectedId: id };
}

export function movePageBy(list: PageList, id: number, delta: number): PageList {
  const from = indexOf(list, id);
  return from === -1 ? list : movePageTo(list, id, from + delta);
}

/**
 * Drag and drop reports a gap between pages (0 = before the first, length = after the last).
 * Removing the dragged page first shifts every gap after it down by one.
 */
export function dropPageAt(list: PageList, id: number, gap: number): PageList {
  const from = indexOf(list, id);
  if (from === -1) {
    return list;
  }
  return movePageTo(list, id, gap > from ? gap - 1 : gap);
}

/**
 * Undoes a reorder the desktop process refused: the pages of `previous` that are still listed,
 * in its order, then the pages added since (scans only append), keeping the current selection.
 */
export function restoreOrder(current: PageList, previous: PageList): PageList {
  const listed = new Map(current.pages.map((page) => [page.id, page]));
  const before = new Set(pageIds(previous));
  const kept = previous.pages.flatMap((page) => listed.get(page.id) ?? []);
  const added = current.pages.filter((page) => !before.has(page.id));
  return { pages: [...kept, ...added], selectedId: current.selectedId };
}

export function selectPage(list: PageList, id: number | null): PageList {
  if (id !== null && indexOf(list, id) === -1) {
    return list;
  }
  return { pages: list.pages, selectedId: id };
}

/** Steps the selection; with nothing selected, any step selects the first page. */
export function selectBy(list: PageList, delta: number): PageList {
  if (list.pages.length === 0) {
    return list;
  }
  const current = selectedIndex(list);
  const next = current === -1 ? 0 : clamp(current + delta, 0, list.pages.length - 1);
  return { pages: list.pages, selectedId: list.pages[next]?.id ?? null };
}

export function sameOrder(a: PageList, b: PageList): boolean {
  return a.pages.length === b.pages.length && a.pages.every((page, i) => page === b.pages[i]);
}
