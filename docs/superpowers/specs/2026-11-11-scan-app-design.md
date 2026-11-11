# Scan app: design

A macOS desktop app, plus a CLI, that scans multi-page documents from the HP LaserJet Pro MFP M130a
over USB into one PDF, with no HP software installed.

## Decisions

- **Workflow:** multi-page documents from the flatbed (the printer has no ADF): scan a page, see it
  in a list, reorder or delete pages, save one PDF.
- **Platform:** macOS only. The printer's IPP-over-USB interface (`ff/09/01`) is vendor-class, so
  macOS lets a user-space program claim it without a driver (`just probe` already does).
- **Location:** `app/` in this repository, built from the same Nix flake and CI.
- **Stack:** Rust. A library (`scan-core`) does everything printer-related; a CLI and a Tauri 2 app
  are thin layers over it. The UI is TypeScript without a framework.
- **Protocol:** eSCL (plain HTTP/1.1) over the bulk endpoints of interface 3 (interface 4 as a
  fallback when 3 is busy). Facts this relies on are in [docs/research.md](../../research.md).

## Structure

```
app/
├── Cargo.toml            workspace, shared lints
├── crates/scan-core/     library
│   ├── transport.rs      `Transport` trait: write bytes, read bytes with a deadline
│   ├── usb.rs            nusb implementation: find 03f0 with an ff/09/01 interface, claim it
│   ├── http.rs           request encoding, response framing (Content-Length, chunked, 1xx)
│   ├── escl.rs           ScanSettings XML, capabilities and status parsing, the scan job
│   ├── jpeg.rs           reads width, height and components from a JPEG's SOF marker
│   ├── pdf.rs            JPEG pages → one PDF, embedded unchanged (DCTDecode)
│   └── lib.rs            `Scanner`: open, status, capabilities, scan_page, cancel
├── crates/scan-cli/      `m130a-scan`
└── desktop/
    ├── src-tauri/        commands over scan-core; pages held in memory
    └── ui/               HTML, CSS, TypeScript
```

Dependencies, each for one job: `nusb` (USB, pure Rust), `httparse` (response heads), `quick-xml`
(eSCL XML), `pdf-writer` (PDF), `thiserror` (library errors), `clap` and `anyhow` (CLI), `tauri` and
`tauri-plugin-dialog` (desktop), `lopdf` (tests only, to read generated PDFs back).

## One scan

Observed on the printer (2026-11-11, 75 dpi grey, fixtures in `app/crates/scan-core/tests/fixtures/`):

```
GET  /eSCL/ScannerStatus      pwg:State must be Idle. Right after power-on it reports Processing
                              for a few seconds; a job started then completes with 0 images.
POST /eSCL/ScanJobs           ScanSettings (text/xml) → 201, Location: /eSCL/ScanJobs/1002
GET  {job}/NextDocument       503 (empty body) while the page is being scanned, polled every second
                              200, image/jpeg, Transfer-Encoding: chunked (2048-byte chunks):
                              the whole page, delivered at once when scanning has finished
                              503 again a few times, then
                              404 → the job is finished (flatbed: one page per job)
```

- The job path is taken from `Location`, relative (as observed) or absolute.
- ScanSettings: Version 2.0, Intent Document, Platen, `RGB24` (colour) or `Grayscale8` (grey),
  150/300/600 dpi (default 300), `pwg:DocumentFormat` image/jpeg, region A4 = 2480×3508 in
  1/300 in at offset 0,0, clamped to the reported maximum. The captured request
  (`m130a-scanjobs-request.http`, 75 dpi grey) is the golden reference.
- Waiting for Idle: poll ScannerStatus every second for up to 30 s before giving up as "busy".
- The 503 polling for one page is bounded by a deadline that grows with the work: 60 s at 150 dpi
  grey up to 240 s at 600 dpi colour.
- After the page, the client does not wait for 404: it sends `DELETE {job}` and ignores the
  answer. This printer answered a few more 503s and then 404, but the closely related M28w keeps
  answering 503 after its single flatbed page (sane-airscan issue #38 trace), and sane-airscan
  likewise stops after the first page and deletes the job.
- Each exchange measures silence by the clock, not by read results: the idle printer answers bulk
  reads with zero-length packets continuously. A response must arrive within 10 s of the request;
  zero-length packets never extend that.
- Before each request the transport drains leftover bytes up to the first empty read.
- Cancel sends `DELETE {job}` and stops polling.

## Pages and PDF

- Pages live in the desktop process as the original JPEG bytes with their dpi. The webview shows
  them through a custom URI scheme (`page://<id>`), scaled by CSS; no thumbnails are computed.
- Reorder and delete operate on the list of ids. Save writes pages in the given order to a path
  chosen in the native save dialog; the default name is `scan-YYYY-MM-DD.pdf` in Documents.
- Each page becomes a PDF page of its physical size (pixels ÷ dpi × 72 pt) with the JPEG embedded
  as is. Grey JPEGs use DeviceGray, colour DeviceRGB, CMYK DeviceCMYK.

## Errors

Every error reaches the user as one sentence saying what happened and what to do.

| Situation | Behaviour |
| --------- | --------- |
| Printer not on USB | Status "Printer not connected"; the app re-checks every 2 s |
| Interface busy (claim fails on 3 and 4) | "The printer's USB interface is in use by another app" |
| Scanner not Idle | "The scanner is busy; try again in a moment" |
| Job rejected (not 201) | HTTP status and the first line of the body |
| 503 for longer than 60 s, or silence over the limit | Timeout error; the job is deleted |
| USB unplugged mid-scan | Error; pages already scanned are kept |
| Closing with unsaved pages | Confirmation dialog |
| Save fails | The OS error message and the path |

## CLI

`m130a-scan --out FILE.pdf [--grey] [--dpi 150|300|600]` scans a page, then asks "another page?
[y/N]", repeating until no, and writes the PDF. `m130a-scan status` prints the scanner state and
capabilities. Exit codes: 0 success, 1 error (message on stderr).

## Testing

- `scan-core` unit tests, no hardware: HTTP framing (split reads, zero-length packets, chunked,
  1xx, malformed); ScanSettings XML; capabilities and status parsing against fixtures captured from
  the printer (serial number removed); the whole scan flow against a scripted fake `Transport`
  (201 with absolute and relative Location, 503 retries, chunked JPEG, 404, timeouts, cancel);
  JPEG SOF parsing; PDF output read back with `lopdf` (page count, MediaBox, filter).
- Hardware tests marked `#[ignore]`, run with the printer attached: status, capabilities, one
  75 dpi grey scan.
- UI: Biome lint and format; Vitest for the page-list logic.
- Limits as for the firmware: functions ≤ 100 lines, complexity ≤ 8, ≤ 5 parameters, 100-character
  lines; clippy with the pedantic set and the project's deny list; zero warnings.

## Toolchain and CI

- The flake adds Rust (rustc, cargo, clippy, rustfmt), cargo-tauri, Bun and Biome.
- `just app-test`, `just app-lint`, `just app-build` (a `.app` bundle in `app/target`).
- CI: the Linux job also runs `scan-core` and `scan-cli` tests and lints; a macOS job builds the
  desktop app and runs the UI checks.

## Out of scope

Rotation and cropping, OCR, other printers, Windows and Linux desktop builds, scanning over WiFi
(that is the Pico bridge's job).
