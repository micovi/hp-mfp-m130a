import { describe, expect, it } from "vitest";
import { defaultPdfName, fileName, pageCount, pageDetails, savedNotice } from "./text.ts";

describe("defaultPdfName", () => {
  it("pads month and day", () => {
    expect(defaultPdfName(new Date(2026, 0, 5, 9, 30))).toBe("scan-2026-01-05.pdf");
  });

  it("uses the local date late in the evening", () => {
    expect(defaultPdfName(new Date(2026, 10, 11, 23, 59))).toBe("scan-2026-11-11.pdf");
  });
});

describe("pageCount", () => {
  it("uses the singular for one page", () => {
    expect(pageCount(1)).toBe("1 page");
  });

  it("uses the plural otherwise", () => {
    expect(pageCount(0)).toBe("0 pages");
    expect(pageCount(12)).toBe("12 pages");
  });
});

describe("pageDetails", () => {
  it("names the resolution and colour mode", () => {
    const base = { id: 1, width_px: 1240, height_px: 1754, dpi: 150 };
    expect(pageDetails({ ...base, color: true })).toBe("150 dpi · Colour");
    expect(pageDetails({ ...base, color: false })).toBe("150 dpi · Grey");
  });
});

describe("fileName and savedNotice", () => {
  it("keeps the last path component", () => {
    expect(fileName("/Users/me/Documents/scan-2026-11-11.pdf")).toBe("scan-2026-11-11.pdf");
    expect(fileName("plain.pdf")).toBe("plain.pdf");
  });

  it("falls back to the whole path when it ends in a separator", () => {
    expect(fileName("/odd/")).toBe("/odd/");
  });

  it("reports what was saved where", () => {
    expect(savedNotice(3, "/tmp/a.pdf")).toBe("Saved 3 pages to a.pdf");
  });
});
