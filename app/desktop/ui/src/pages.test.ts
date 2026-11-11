import { describe, expect, it } from "vitest";
import {
  addPage,
  dropPageAt,
  emptyList,
  movePageBy,
  movePageTo,
  type PageList,
  type PageMeta,
  pageIds,
  removePage,
  sameOrder,
  selectBy,
  selectedIndex,
  selectPage,
} from "./pages.ts";

function page(id: number): PageMeta {
  return { id, width_px: 2480, height_px: 3508, dpi: 300, color: false };
}

function listOf(ids: number[], selectedId: number | null = null): PageList {
  return { pages: ids.map(page), selectedId };
}

describe("addPage", () => {
  it("appends the page and selects it", () => {
    const list = addPage(listOf([1, 2], 1), page(3));
    expect(pageIds(list)).toEqual([1, 2, 3]);
    expect(list.selectedId).toBe(3);
  });

  it("starts an empty list", () => {
    const list = addPage(emptyList, page(7));
    expect(pageIds(list)).toEqual([7]);
    expect(list.selectedId).toBe(7);
  });

  it("rejects a duplicate id", () => {
    expect(() => addPage(listOf([1]), page(1))).toThrow("Page 1 is already in the list");
  });

  it("leaves the original list unchanged", () => {
    const original = listOf([1]);
    addPage(original, page(2));
    expect(pageIds(original)).toEqual([1]);
  });
});

describe("removePage", () => {
  it("keeps the selection when another page is removed", () => {
    const list = removePage(listOf([1, 2, 3], 3), 1);
    expect(pageIds(list)).toEqual([2, 3]);
    expect(list.selectedId).toBe(3);
  });

  it("selects the page that takes the removed page's place", () => {
    const list = removePage(listOf([1, 2, 3], 2), 2);
    expect(list.selectedId).toBe(3);
  });

  it("selects the new last page when the last page is removed", () => {
    const list = removePage(listOf([1, 2, 3], 3), 3);
    expect(list.selectedId).toBe(2);
  });

  it("clears the selection when the only page is removed", () => {
    const list = removePage(listOf([1], 1), 1);
    expect(list.pages).toEqual([]);
    expect(list.selectedId).toBeNull();
  });

  it("ignores an unknown id", () => {
    const list = listOf([1, 2], 1);
    expect(removePage(list, 9)).toBe(list);
  });
});

describe("movePageTo and movePageBy", () => {
  it("moves a page forward and selects it", () => {
    const list = movePageTo(listOf([1, 2, 3, 4]), 1, 2);
    expect(pageIds(list)).toEqual([2, 3, 1, 4]);
    expect(list.selectedId).toBe(1);
  });

  it("moves a page backward", () => {
    expect(pageIds(movePageTo(listOf([1, 2, 3, 4]), 4, 0))).toEqual([4, 1, 2, 3]);
  });

  it("clamps targets outside the list", () => {
    expect(pageIds(movePageTo(listOf([1, 2, 3]), 2, 99))).toEqual([1, 3, 2]);
    expect(pageIds(movePageTo(listOf([1, 2, 3]), 2, -5))).toEqual([2, 1, 3]);
  });

  it("returns the same list when nothing moves", () => {
    const list = listOf([1, 2, 3], 1);
    expect(movePageTo(list, 2, 1)).toBe(list);
    expect(movePageBy(list, 1, -1)).toBe(list);
    expect(movePageBy(list, 3, 1)).toBe(list);
    expect(movePageTo(list, 9, 0)).toBe(list);
    expect(movePageBy(list, 9, 1)).toBe(list);
  });

  it("steps one place up and down", () => {
    expect(pageIds(movePageBy(listOf([1, 2, 3]), 2, -1))).toEqual([2, 1, 3]);
    expect(pageIds(movePageBy(listOf([1, 2, 3]), 2, 1))).toEqual([1, 3, 2]);
  });
});

describe("dropPageAt", () => {
  it("drops before the first page", () => {
    expect(pageIds(dropPageAt(listOf([1, 2, 3]), 3, 0))).toEqual([3, 1, 2]);
  });

  it("drops after the last page", () => {
    expect(pageIds(dropPageAt(listOf([1, 2, 3]), 1, 3))).toEqual([2, 3, 1]);
  });

  it("accounts for the dragged page's own slot when moving forward", () => {
    expect(pageIds(dropPageAt(listOf([1, 2, 3, 4]), 1, 2))).toEqual([2, 1, 3, 4]);
  });

  it("is a no-op on the gaps either side of the dragged page", () => {
    const list = listOf([1, 2, 3]);
    expect(dropPageAt(list, 2, 1)).toBe(list);
    expect(dropPageAt(list, 2, 2)).toBe(list);
  });

  it("ignores an unknown id", () => {
    const list = listOf([1, 2]);
    expect(dropPageAt(list, 5, 0)).toBe(list);
  });
});

describe("selection", () => {
  it("selects and clears", () => {
    expect(selectPage(listOf([1, 2]), 2).selectedId).toBe(2);
    expect(selectPage(listOf([1, 2], 2), null).selectedId).toBeNull();
  });

  it("ignores an unknown id", () => {
    const list = listOf([1, 2], 1);
    expect(selectPage(list, 9)).toBe(list);
  });

  it("steps the selection and stops at the ends", () => {
    expect(selectBy(listOf([1, 2, 3], 2), 1).selectedId).toBe(3);
    expect(selectBy(listOf([1, 2, 3], 3), 1).selectedId).toBe(3);
    expect(selectBy(listOf([1, 2, 3], 1), -4).selectedId).toBe(1);
  });

  it("selects the first page when stepping from no selection", () => {
    expect(selectBy(listOf([1, 2, 3]), -1).selectedId).toBe(1);
  });

  it("does nothing on an empty list", () => {
    expect(selectBy(emptyList, 1)).toBe(emptyList);
  });

  it("reports the selected index", () => {
    expect(selectedIndex(listOf([4, 5, 6], 6))).toBe(2);
    expect(selectedIndex(listOf([4, 5, 6]))).toBe(-1);
  });
});

describe("sameOrder", () => {
  it("compares page order, not selection", () => {
    const list = listOf([1, 2], 1);
    expect(sameOrder(list, selectPage(list, 2))).toBe(true);
    expect(sameOrder(list, movePageBy(list, 1, 1))).toBe(false);
    expect(sameOrder(list, removePage(list, 2))).toBe(false);
  });
});
