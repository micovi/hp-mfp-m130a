import { describe, expect, it } from "vitest";
import type { PageList, PageMeta } from "./pages.ts";
import { previewAction, previewCaption, previewPage, stepPreview } from "./preview.ts";

function page(id: number, color = false): PageMeta {
  return { id, width_px: 1240, height_px: 1754, dpi: 150, color };
}

function listOf(ids: number[], selectedId: number | null): PageList {
  return { pages: ids.map((id) => page(id)), selectedId };
}

function key(name: string, modifiers: { metaKey?: boolean; ctrlKey?: boolean } = {}) {
  return { key: name, metaKey: modifiers.metaKey ?? false, ctrlKey: modifiers.ctrlKey ?? false };
}

describe("previewAction", () => {
  it("closes on Space and Escape", () => {
    expect(previewAction(key(" "))).toBe("close");
    expect(previewAction(key("Escape"))).toBe("close");
  });

  it("steps back on ← and ↑, forward on → and ↓", () => {
    expect(previewAction(key("ArrowLeft"))).toBe("previous");
    expect(previewAction(key("ArrowUp"))).toBe("previous");
    expect(previewAction(key("ArrowRight"))).toBe("next");
    expect(previewAction(key("ArrowDown"))).toBe("next");
  });

  it("keeps Tab inside the preview", () => {
    expect(previewAction(key("Tab"))).toBe("trap-focus");
  });

  it("ignores Delete and Backspace, so the preview never deletes a page", () => {
    expect(previewAction(key("Delete"))).toBe("ignore");
    expect(previewAction(key("Backspace"))).toBe("ignore");
  });

  it("ignores shortcuts with ⌘ or Ctrl, even on navigation keys", () => {
    expect(previewAction(key("s", { metaKey: true }))).toBe("ignore");
    expect(previewAction(key("ArrowUp", { metaKey: true }))).toBe("ignore");
    expect(previewAction(key("Backspace", { metaKey: true }))).toBe("ignore");
    expect(previewAction(key(" ", { ctrlKey: true }))).toBe("ignore");
  });

  it("ignores other keys", () => {
    expect(previewAction(key("a"))).toBe("ignore");
    expect(previewAction(key("Enter"))).toBe("ignore");
  });
});

describe("previewPage", () => {
  it("is the selected page", () => {
    expect(previewPage(listOf([4, 5, 6], 5))?.id).toBe(5);
  });

  it("is null without a selection or with an unknown one", () => {
    expect(previewPage(listOf([4, 5], null))).toBeNull();
    expect(previewPage(listOf([], null))).toBeNull();
    expect(previewPage(listOf([4], 9))).toBeNull();
  });
});

describe("stepPreview", () => {
  it("moves the selection one page at a time", () => {
    expect(stepPreview(listOf([1, 2, 3], 2), 1).selectedId).toBe(3);
    expect(stepPreview(listOf([1, 2, 3], 2), -1).selectedId).toBe(1);
  });

  it("stops at the first and last page", () => {
    expect(stepPreview(listOf([1, 2, 3], 1), -1).selectedId).toBe(1);
    expect(stepPreview(listOf([1, 2, 3], 3), 1).selectedId).toBe(3);
  });

  it("does not select a page when nothing is shown", () => {
    const list = listOf([1, 2], null);
    expect(stepPreview(list, 1)).toBe(list);
  });

  it("keeps the page order", () => {
    const list = listOf([3, 1, 2], 1);
    expect(stepPreview(list, 1).pages).toBe(list.pages);
  });
});

describe("previewCaption", () => {
  it("names the position, size, resolution and colour mode", () => {
    expect(previewCaption(listOf([7, 8, 9, 10], 8))).toBe(
      "Page 2 of 4 · 1240 × 1754 px · 150 dpi · Grey",
    );
  });

  it("names colour pages", () => {
    const list: PageList = { pages: [page(1, true)], selectedId: 1 };
    expect(previewCaption(list)).toBe("Page 1 of 1 · 1240 × 1754 px · 150 dpi · Colour");
  });

  it("is null with nothing selected", () => {
    expect(previewCaption(listOf([1], null))).toBeNull();
  });
});
