import type { PageList, PageMeta } from "./pages.ts";
import { pageDetails } from "./text.ts";

export interface GridEvents {
  select(id: number): void;
  remove(id: number): void;
  drop(id: number, gap: number): void;
}

export interface GridView {
  render(list: PageList, scanning: boolean): void;
  focusSelected(): void;
  /** Scrolls the last card (the page being scanned, or the one just added) into view. */
  revealLast(): void;
  /** Pages per row, so ↑ and ↓ can step a whole row. */
  columns(): number;
}

const ONCE: AddEventListenerOptions = { once: true };

interface Target {
  readonly item: HTMLLIElement;
  readonly after: boolean;
}

function template(id: string): HTMLLIElement {
  const node = document.querySelector<HTMLTemplateElement>(`#${id}`);
  const item = node?.content.firstElementChild?.cloneNode(true);
  if (!(item instanceof HTMLLIElement)) {
    throw new Error(`index.html is missing the #${id} template`);
  }
  return item;
}

function part<T extends Element>(item: Element, selector: string): T {
  const found = item.querySelector<T>(selector);
  if (found === null) {
    throw new Error(`The page template is missing ${selector}`);
  }
  return found;
}

function pageItems(root: HTMLOListElement): HTMLLIElement[] {
  return [...root.querySelectorAll<HTMLLIElement>("li.page[data-id]")];
}

function createItem(page: PageMeta, imageUrl: string, animate: boolean): HTMLLIElement {
  const item = template("page-template");
  item.setAttribute("data-id", String(page.id));
  if (animate) {
    // Moving a node restarts its animation, so the entrance class must not outlive it.
    item.classList.add("fresh");
    item.addEventListener("animationend", () => item.classList.remove("fresh"), ONCE);
  }
  const sheet = part<HTMLElement>(item, ".page-sheet");
  sheet.style.aspectRatio = [page.width_px, page.height_px].join(" / ");
  const image = document.createElement("img");
  image.alt = "";
  image.draggable = false;
  image.decoding = "async";
  image.src = imageUrl;
  sheet.append(image);
  part<HTMLElement>(item, ".page-details").textContent = pageDetails(page);
  return item;
}

interface ItemState {
  readonly number: number;
  readonly selected: boolean;
  /** The one page Tab lands on: the selected page, or the first when none is selected. */
  readonly tabStop: boolean;
}

function updateItem(item: HTMLLIElement, page: PageMeta, { number, selected, tabStop }: ItemState) {
  const sheet = part<HTMLButtonElement>(item, ".page-sheet");
  item.classList.toggle("selected", selected);
  sheet.tabIndex = tabStop ? 0 : -1;
  sheet.setAttribute("aria-label", `Page ${number}, ${pageDetails(page)}`);
  sheet.toggleAttribute("aria-current", selected);
  part<HTMLElement>(item, ".page-number").textContent = String(number);
  part<HTMLElement>(item, ".page-delete").setAttribute("aria-label", `Delete page ${number}`);
}

function distance(rect: DOMRect, x: number, y: number): number {
  const dx = x - (rect.left + rect.width / 2);
  const dy = y < rect.top || y > rect.bottom ? Math.abs(y - (rect.top + rect.height / 2)) : 0;
  return dy * 4 + Math.abs(dx);
}

/** The page nearest the pointer, preferring its own row, and which side of it the pointer is on. */
function dropTarget(root: HTMLOListElement, x: number, y: number): Target | null {
  let best: Target | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const item of pageItems(root)) {
    const rect = item.getBoundingClientRect();
    const d = distance(rect, x, y);
    if (d < bestDistance) {
      bestDistance = d;
      best = { item, after: x > rect.left + rect.width / 2 };
    }
  }
  return best;
}

function clearDropMarks(root: HTMLOListElement) {
  for (const item of root.querySelectorAll(".drop-before, .drop-after")) {
    item.classList.remove("drop-before", "drop-after");
  }
}

function itemId(element: EventTarget | null): number | null {
  const item = element instanceof Element ? element.closest<HTMLLIElement>("li[data-id]") : null;
  return item === null ? null : Number(item.getAttribute("data-id"));
}

function wireClicks(root: HTMLOListElement, events: GridEvents) {
  root.addEventListener("click", (event) => {
    const id = itemId(event.target);
    if (id === null) {
      return;
    }
    const remove = event.target instanceof Element && event.target.closest(".page-delete");
    if (remove) {
      events.remove(id);
    } else {
      events.select(id);
    }
  });
}

function wireDrag(root: HTMLOListElement, events: GridEvents) {
  let dragged: number | null = null;
  root.addEventListener("dragstart", (event) => {
    dragged = itemId(event.target);
    if (dragged === null || event.dataTransfer === null || !(event.target instanceof Element)) {
      return;
    }
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", String(dragged));
    root.classList.add("dragging");
    event.target.classList.add("lifted");
  });
  root.addEventListener("dragover", (event) => {
    if (dragged === null) {
      return;
    }
    event.preventDefault();
    const target = dropTarget(root, event.clientX, event.clientY);
    clearDropMarks(root);
    target?.item.classList.add(target.after ? "drop-after" : "drop-before");
  });
  root.addEventListener("drop", (event) => {
    event.preventDefault();
    const target = dropTarget(root, event.clientX, event.clientY);
    if (dragged !== null && target !== null) {
      const gap = pageItems(root).indexOf(target.item) + (target.after ? 1 : 0);
      events.drop(dragged, gap);
    }
  });
  root.addEventListener("dragend", () => {
    dragged = null;
    clearDropMarks(root);
    root.classList.remove("dragging");
    root.querySelector(".lifted")?.classList.remove("lifted");
  });
}

function reconcile(
  root: HTMLOListElement,
  list: PageList,
  imageUrl: (id: number) => string,
  animate: boolean,
) {
  const existing = new Map(
    pageItems(root).map((item) => [Number(item.getAttribute("data-id")), item]),
  );
  const ordered = list.pages.map((page, index) => {
    const item = existing.get(page.id) ?? createItem(page, imageUrl(page.id), animate);
    existing.delete(page.id);
    const selected = page.id === list.selectedId;
    const tabStop = selected || (list.selectedId === null && index === 0);
    updateItem(item, page, { number: index + 1, selected, tabStop });
    return item;
  });
  for (const stale of existing.values()) {
    stale.remove();
  }
  return ordered;
}

export function createGrid(
  root: HTMLOListElement,
  imageUrl: (id: number) => string,
  events: GridEvents,
): GridView {
  const scanningItem = template("scanning-template");
  wireClicks(root, events);
  wireDrag(root, events);
  let rendered = false;
  return {
    render(list, scanning) {
      // Pages present when the window opens appear at once; only later arrivals animate in.
      const items = reconcile(root, list, imageUrl, rendered);
      rendered = true;
      // Re-appending in order moves existing nodes without reloading their images.
      items.forEach((item, index) => {
        if (root.children[index] !== item) {
          root.insertBefore(item, root.children[index] ?? null);
        }
      });
      if (scanning) {
        root.append(scanningItem);
      } else {
        scanningItem.remove();
      }
      root.hidden = items.length === 0 && !scanning;
    },
    revealLast() {
      root.lastElementChild?.scrollIntoView({ block: "nearest", behavior: "smooth" });
    },
    focusSelected() {
      root.querySelector<HTMLElement>("li.selected .page-sheet")?.focus();
    },
    columns() {
      const template = getComputedStyle(root).gridTemplateColumns;
      return Math.max(1, template.split(" ").filter((track) => track !== "").length);
    },
  };
}
