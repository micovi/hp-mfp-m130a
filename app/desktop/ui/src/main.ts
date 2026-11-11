import { type Backend, createBackend, type Dpi, errorMessage } from "./api.ts";
import { createGrid, type GridView } from "./grid.ts";
import {
  addPage,
  dropPageAt,
  emptyList,
  movePageBy,
  type PageList,
  pageIds,
  removePage,
  sameOrder,
  selectBy,
  selectPage,
} from "./pages.ts";
import { defaultPdfName, pageCount, savedNotice } from "./text.ts";

type Connection = "checking" | "connected" | "disconnected";

interface AppState {
  list: PageList;
  connection: Connection;
  scanning: boolean;
  cancelling: boolean;
  saving: boolean;
  dirty: boolean;
  notice: string;
  error: string | null;
}

const POLL_MS = 2000;
const SCANNING_TEXT = "Scanning… place the next page after this one finishes";
const CONNECTION_TEXT: Record<Connection, string> = {
  checking: "Looking for the printer…",
  connected: "Printer connected",
  disconnected: "Printer not connected",
};
const EMPTY_TEXT = {
  ready: {
    title: "Place a page on the glass",
    text: "Lay it face down against the corner marker, close the lid, then press Scan page.",
  },
  offline: {
    title: "Connect the printer",
    text:
      "Plug the M130a into this Mac with a USB cable and turn it on. " +
      "Scanning becomes available as soon as it appears.",
  },
};

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (found === null) {
    throw new Error(`index.html is missing #${id}`);
  }
  return found as T;
}

const dom = {
  scan: element<HTMLButtonElement>("scan"),
  save: element<HTMLButtonElement>("save"),
  mode: element<HTMLFieldSetElement>("mode"),
  dpi: element<HTMLSelectElement>("dpi"),
  banner: element<HTMLDivElement>("error"),
  bannerText: element<HTMLParagraphElement>("error-text"),
  dismiss: element<HTMLButtonElement>("dismiss"),
  workspace: element<HTMLElement>("workspace"),
  empty: element<HTMLElement>("empty"),
  emptyTitle: element<HTMLHeadingElement>("empty-title"),
  emptyText: element<HTMLParagraphElement>("empty-text"),
  pages: element<HTMLOListElement>("pages"),
  connection: element<HTMLParagraphElement>("connection"),
  connectionText: element<HTMLSpanElement>("connection-text"),
  activity: element<HTMLParagraphElement>("activity"),
};

function startBackend(): Backend {
  try {
    return createBackend();
  } catch (error) {
    dom.bannerText.textContent = errorMessage(error);
    dom.banner.hidden = false;
    throw error;
  }
}

const backend = startBackend();
const state: AppState = {
  list: emptyList,
  connection: "checking",
  scanning: false,
  cancelling: false,
  saving: false,
  dirty: false,
  notice: "",
  error: null,
};
for (const page of backend.initialPages()) {
  state.list = addPage(state.list, page);
}

const grid: GridView = createGrid(dom.pages, backend.pageImageUrl, {
  select: (id) => select(selectPage(state.list, id), false),
  remove: (id) => void deletePage(id),
  drop: (id, gap) => void reorder(dropPageAt(state.list, id, gap)),
});

// Tauri may run commands concurrently; page edits must reach the store in the order made.
let edits: Promise<unknown> = Promise.resolve();
function queueEdit<T>(task: () => Promise<T>): Promise<T> {
  const run = edits.then(task, task);
  edits = run.catch(() => undefined);
  return run;
}

function showError(error: unknown) {
  state.error = errorMessage(error);
}

function activityText(): string {
  if (state.scanning) {
    return state.cancelling ? "Cancelling the scan…" : SCANNING_TEXT;
  }
  if (state.saving) {
    return "Saving the PDF…";
  }
  if (state.notice !== "") {
    return state.notice;
  }
  const count = state.list.pages.length;
  if (count === 0) {
    return "";
  }
  return state.dirty ? `${pageCount(count)}, not saved yet` : pageCount(count);
}

function renderToolbar() {
  const offline = state.connection !== "connected";
  dom.scan.textContent = state.scanning ? "Cancel" : "Scan page";
  dom.scan.classList.toggle("primary", !state.scanning);
  dom.scan.disabled = state.scanning ? state.cancelling : offline || state.saving;
  dom.scan.title = !state.scanning && offline ? "Connect the printer to scan" : "";
  dom.save.disabled = state.list.pages.length === 0 || state.scanning || state.saving;
  dom.mode.disabled = state.scanning;
  dom.dpi.disabled = state.scanning;
}

function renderStatus() {
  dom.connection.setAttribute("data-state", state.connection);
  dom.connectionText.textContent = CONNECTION_TEXT[state.connection];
  dom.activity.textContent = activityText();
  dom.activity.classList.toggle("busy", state.scanning || state.saving);
  dom.banner.hidden = state.error === null;
  dom.bannerText.textContent = state.error ?? "";
}

function renderEmpty() {
  const copy = state.connection === "disconnected" ? EMPTY_TEXT.offline : EMPTY_TEXT.ready;
  dom.empty.hidden = state.list.pages.length > 0 || state.scanning;
  dom.empty.setAttribute("data-state", state.connection);
  dom.emptyTitle.textContent = copy.title;
  dom.emptyText.textContent = copy.text;
}

function render() {
  renderToolbar();
  renderStatus();
  renderEmpty();
  grid.render(state.list, state.scanning);
}

function selectedColor(): boolean {
  return dom.mode.querySelector<HTMLInputElement>("input:checked")?.value !== "grey";
}

function selectedDpi(): Dpi {
  const value = Number(dom.dpi.value);
  return value === 150 || value === 600 ? value : 300;
}

async function scan() {
  state.scanning = true;
  state.cancelling = false;
  state.notice = "";
  render();
  grid.revealLast();
  try {
    const page = await backend.scanPage(selectedColor(), selectedDpi());
    state.list = addPage(state.list, page);
    state.dirty = true;
    state.error = null;
  } catch (error) {
    if (!state.cancelling) {
      showError(error);
    }
  } finally {
    state.scanning = false;
    state.cancelling = false;
    render();
    grid.revealLast();
  }
}

async function cancelScan() {
  state.cancelling = true;
  render();
  try {
    await backend.cancelScan();
  } catch (error) {
    state.cancelling = false;
    showError(error);
    render();
  }
}

async function deletePage(id: number) {
  try {
    await queueEdit(() => backend.deletePage(id));
    state.list = removePage(state.list, id);
    state.dirty = true;
    state.notice = "";
  } catch (error) {
    showError(error);
  }
  render();
}

/** Shows the new order at once and rolls it back if the desktop process refuses it. */
async function reorder(next: PageList) {
  const previous = state.list;
  state.list = next;
  if (sameOrder(previous, next)) {
    render();
    return;
  }
  state.dirty = true;
  state.notice = "";
  render();
  try {
    await queueEdit(() => backend.reorderPages(pageIds(next)));
  } catch (error) {
    if (state.list === next) {
      state.list = previous;
    }
    showError(error);
    render();
  }
}

async function savePdf() {
  if (state.list.pages.length === 0 || state.scanning || state.saving) {
    return;
  }
  try {
    const path = await backend.chooseSavePath(defaultPdfName(new Date()));
    if (path === null) {
      return;
    }
    state.saving = true;
    render();
    const written = await queueEdit(() => backend.savePdf(path));
    state.dirty = false;
    state.notice = savedNotice(written, path);
    state.error = null;
  } catch (error) {
    showError(error);
  } finally {
    state.saving = false;
    render();
  }
}

let lastPollError = "";

async function pollConnection() {
  try {
    const connected = await backend.printerConnected();
    state.connection = connected ? "connected" : "disconnected";
    lastPollError = "";
  } catch (error) {
    state.connection = "disconnected";
    // Repeating the same failure every two seconds would undo the user's dismissal.
    if (errorMessage(error) !== lastPollError) {
      lastPollError = errorMessage(error);
      showError(error);
    }
  }
  render();
  window.setTimeout(pollConnection, POLL_MS);
}

async function confirmClose() {
  try {
    if (await backend.confirm("You have unsaved pages. Quit anyway?", "Quit")) {
      await backend.quit();
    }
  } catch (error) {
    showError(error);
    render();
  }
}

function select(next: PageList, focus: boolean) {
  state.list = next;
  render();
  if (focus) {
    grid.focusSelected();
  }
}

function isFormControl(target: EventTarget | null): boolean {
  return target instanceof HTMLInputElement || target instanceof HTMLSelectElement;
}

/** Page keys apply while focus is on the pages or nowhere in particular. */
function pageKeysActive(target: EventTarget | null): boolean {
  return target === document.body || (target instanceof Node && dom.pages.contains(target));
}

function moveKey(event: KeyboardEvent): number {
  const steps: Record<string, number> = { ArrowUp: -1, ArrowLeft: -1, ArrowDown: 1, ArrowRight: 1 };
  return event.metaKey ? (steps[event.key] ?? 0) : 0;
}

function selectKey(event: KeyboardEvent): number {
  const columns = grid.columns();
  const steps: Record<string, number> = {
    ArrowLeft: -1,
    ArrowRight: 1,
    ArrowUp: -columns,
    ArrowDown: columns,
  };
  return event.metaKey || event.altKey ? 0 : (steps[event.key] ?? 0);
}

function handlePageKey(event: KeyboardEvent): boolean {
  const id = state.list.selectedId;
  const move = moveKey(event);
  if (move !== 0 && id !== null) {
    void reorder(movePageBy(state.list, id, move)).then(() => grid.focusSelected());
    return true;
  }
  const step = selectKey(event);
  if (step !== 0) {
    select(selectBy(state.list, step), true);
    return true;
  }
  if ((event.key === "Delete" || event.key === "Backspace") && id !== null) {
    void deletePage(id).then(() => grid.focusSelected());
    return true;
  }
  return false;
}

/** Esc and ⌘. cancel, as in macOS sheets and progress windows. */
function isCancelKey(event: KeyboardEvent): boolean {
  return event.key === "Escape" || (event.metaKey && event.key === ".");
}

function handleWindowKey(event: KeyboardEvent): boolean {
  if (event.metaKey && event.key.toLowerCase() === "s") {
    void savePdf();
    return true;
  }
  if (isCancelKey(event) && state.scanning && !state.cancelling) {
    void cancelScan();
    return true;
  }
  return false;
}

function onKeyDown(event: KeyboardEvent) {
  if (handleWindowKey(event)) {
    event.preventDefault();
    return;
  }
  if (isFormControl(event.target) || !pageKeysActive(event.target)) {
    return;
  }
  if (handlePageKey(event)) {
    event.preventDefault();
  }
}

function wire() {
  dom.scan.addEventListener("click", () => {
    void (state.scanning ? cancelScan() : scan());
  });
  dom.save.addEventListener("click", () => void savePdf());
  dom.dismiss.addEventListener("click", () => {
    state.error = null;
    render();
  });
  dom.workspace.addEventListener("click", (event) => {
    if (event.target === dom.workspace || event.target === dom.pages) {
      select(selectPage(state.list, null), false);
    }
  });
  document.addEventListener("keydown", onKeyDown);
  // The webview's Reload / Inspect menu does not belong in the app; error text stays copyable.
  document.addEventListener("contextmenu", (event) => {
    if (!(event.target instanceof Element && event.target.closest(".banner-text"))) {
      event.preventDefault();
    }
  });
}

wire();
render();
void backend
  .onConfirmClose(() => void confirmClose())
  .catch((error: unknown) => {
    showError(error);
    render();
  });
void pollConnection();
