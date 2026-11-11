import type { PageList, PageMeta } from "./pages.ts";
import { previewCaption, previewPage } from "./preview.ts";

export interface PreviewHost {
  /** The full-resolution image of a page. */
  imageUrl(id: number): string;
  /** The page's card in the grid: where the zoom starts and ends, and an image already loaded. */
  cardSheet(id: number): HTMLElement | null;
  /** The backdrop or close button was clicked. */
  requestClose(): void;
}

export interface PreviewView {
  isOpen(): boolean;
  open(list: PageList): void;
  /** Shows the selected page; cheap when it is already shown, so every render may call it. */
  show(list: PageList): void;
  close(list: PageList): Promise<void>;
  focus(): void;
}

interface Parts {
  readonly root: HTMLElement;
  readonly backdrop: HTMLElement;
  readonly sheet: HTMLElement;
  readonly caption: HTMLElement;
  readonly close: HTMLButtonElement;
  readonly low: HTMLImageElement;
  readonly full: HTMLImageElement;
}

const OPEN_MS = 180;
const CLOSE_MS = 150;
const EASE_OUT = "cubic-bezier(0.16, 1, 0.3, 1)";
const EASE_IN = "cubic-bezier(0.5, 0, 0.75, 0)";

function part<T extends HTMLElement>(root: HTMLElement, selector: string, type: new () => T): T {
  const found = root.querySelector(selector);
  if (!(found instanceof type)) {
    throw new Error(`index.html's preview is missing ${selector}`);
  }
  return found;
}

function image(className: string): HTMLImageElement {
  const element = document.createElement("img");
  element.className = `preview-image ${className}`;
  element.alt = "";
  element.decoding = "async";
  element.draggable = false;
  element.hidden = true;
  return element;
}

function findParts(root: HTMLElement): Parts {
  const sheet = part(root, ".preview-sheet", HTMLDivElement);
  const low = image("low");
  const full = image("full");
  sheet.append(low, full);
  return {
    root,
    backdrop: part(root, ".preview-backdrop", HTMLDivElement),
    sheet,
    caption: part(root, ".preview-caption", HTMLElement),
    close: part(root, ".preview-close", HTMLButtonElement),
    low,
    full,
  };
}

function reducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** The transform that lays the preview sheet exactly over the card. */
function overCard(card: DOMRect, sheet: DOMRect): string {
  const scale = card.width / sheet.width;
  return `translate(${card.left - sheet.left}px, ${card.top - sheet.top}px) scale(${scale})`;
}

function visibleRect(element: HTMLElement | null): DOMRect | null {
  const rect = element?.getBoundingClientRect();
  return rect === undefined || rect.width === 0 ? null : rect;
}

/** Zooms between the card and the preview; resolves at once when there is nothing to animate. */
function zoom(parts: Parts, card: DOMRect | null, opening: boolean): Promise<unknown> {
  if (card === null || reducedMotion()) {
    return Promise.resolve();
  }
  const timing = { duration: opening ? OPEN_MS : CLOSE_MS, easing: opening ? EASE_OUT : EASE_IN };
  const atCard = { transform: overCard(card, parts.sheet.getBoundingClientRect()) };
  const atRest = { transform: "none" };
  const fade = opening ? [{ opacity: 0 }, { opacity: 1 }] : [{ opacity: 1 }, { opacity: 0 }];
  for (const element of [parts.backdrop, parts.caption, parts.close]) {
    element.animate(fade, timing);
  }
  const frames = opening ? [atCard, atRest] : [atRest, atCard];
  const finished = parts.sheet.animate(frames, { ...timing, fill: "forwards" }).finished;
  // A hidden or minimised window may never advance the animation; closing must not hang on it.
  const deadline = new Promise((resolve) => window.setTimeout(resolve, timing.duration + 50));
  return Promise.race([finished, deadline]);
}

/** The card's image is already decoded, so it stands in while the full image decodes. */
function showLowImage(low: HTMLImageElement, card: HTMLElement | null) {
  const loaded = card?.querySelector("img");
  const usable = loaded instanceof HTMLImageElement && loaded.complete && loaded.naturalWidth > 0;
  low.hidden = !usable;
  if (usable) {
    low.src = loaded.currentSrc || loaded.src;
  }
}

function showFullImage(parts: Parts, page: PageMeta, url: string, number: number) {
  const { full, sheet } = parts;
  full.hidden = true;
  full.alt = `Scan of page ${number}`;
  full.src = url;
  sheet.removeAttribute("data-failed");
  full.decode().then(
    () => {
      if (full.src === url) {
        full.hidden = false;
      }
    },
    () => {
      // The card cannot show it either; say so on the sheet instead of leaving it blank.
      if (full.src === url && parts.low.hidden) {
        sheet.setAttribute("data-failed", "");
      }
    },
  );
  sheet.style.setProperty("--aspect", `${page.width_px} / ${page.height_px}`);
  sheet.style.setProperty("--ratio", String(page.height_px / page.width_px));
}

function setBackgroundInert(root: HTMLElement, inert: boolean) {
  for (const element of document.body.children) {
    if (element !== root && element instanceof HTMLElement && element.tagName !== "TEMPLATE") {
      element.inert = inert;
    }
  }
}

export function createPreview(root: HTMLElement, host: PreviewHost): PreviewView {
  const parts = findParts(root);
  let open = false;
  let shownId: number | null = null;
  let closing: Promise<void> | null = null;

  root.addEventListener("click", (event) => {
    const target = event.target instanceof Element ? event.target : null;
    const inside = target?.closest(".preview-sheet, .preview-caption");
    if (!inside) {
      host.requestClose();
    }
  });

  const view: PreviewView = {
    isOpen: () => open,
    show(list) {
      const page = previewPage(list);
      parts.caption.textContent = previewCaption(list) ?? "";
      if (page === null || page.id === shownId) {
        return;
      }
      shownId = page.id;
      showLowImage(parts.low, host.cardSheet(page.id));
      const number = list.pages.indexOf(page) + 1;
      showFullImage(parts, page, host.imageUrl(page.id), number);
    },
    open(list) {
      const page = previewPage(list);
      if (open || closing !== null || page === null) {
        return;
      }
      open = true;
      root.hidden = false;
      setBackgroundInert(root, true);
      view.show(list);
      parts.close.focus({ preventScroll: true });
      void zoom(parts, visibleRect(host.cardSheet(page.id)), true);
    },
    close(list) {
      if (!open) {
        return closing ?? Promise.resolve();
      }
      open = false;
      const page = previewPage(list);
      const card = page === null ? null : visibleRect(host.cardSheet(page.id));
      closing = zoom(parts, card, false).then(() => {
        root.hidden = true;
        setBackgroundInert(root, false);
        // The closing zoom holds its last frame until the sheet is hidden; drop it for next time.
        for (const animation of parts.sheet.getAnimations()) {
          animation.cancel();
        }
        shownId = null;
        closing = null;
      });
      return closing;
    },
    focus: () => parts.close.focus({ preventScroll: true }),
  };
  return view;
}
