# hp-mfp-m130a

Making a USB-only HP LaserJet Pro MFP M130a printable and scannable over WiFi, with a Raspberry Pi
Pico 2 W plugged into its USB port.

The printer speaks IPP and eSCL over USB on a vendor-class interface (`ff/09/01`), so the plan is
for the Pico to proxy HTTP between WiFi and that interface and advertise it over Bonjour. macOS
would then see an AirPrint printer and an AirScan scanner, with no driver. Background, measurements
and the options considered: [docs/research.md](docs/research.md).

## Status

- **Probe (works):** `just probe` confirmed on the real printer that it answers IPP 2.0 (URF and
  PCLm) and eSCL (flatbed, 75-1200 dpi) over USB.
- **Firmware milestone 1 (builds, not yet run on hardware):** the Pico enumerates the printer as
  USB host, sends the same IPP and eSCL requests and logs the answers on UART. Plan:
  [docs/superpowers/plans/](docs/superpowers/plans/).
- **Scan app (works over USB):** a macOS app and a CLI scan multi-page documents from the flatbed
  into one PDF, talking eSCL to the printer directly ([App](#app)).
- **Not started:** the WiFi proxy and Bonjour advertisement.

## Setup

Every tool comes from the Nix dev shell in `flake.nix`. With direnv, `direnv allow` once; otherwise
prefix commands with `nix develop --command`.

## Probe

`just probe` talks to the printer over USB from this computer: it lists the USB interfaces, then
asks the IPP interface for the printer's attributes and its eSCL scanner capabilities, and the
LEDM interface for its resource list. It is read-only, never sending a print or scan job. The
answers land in `probe-output/`.

## Firmware

C on pico-sdk 2.3.1 with TinyUSB 0.21.0 in host mode, in `firmware/`. Wiring:
[docs/wiring.svg](docs/wiring.svg) (a 5 V supply on VBUS, a micro-USB OTG adapter to the printer
cable, and a Raspberry Pi Debug Probe for flashing and the UART log).

```
just fw-build   # cross-compile build/fw/m130a_bridge.{elf,uf2}
just flash      # flash through the Debug Probe
just logs       # the Pico's UART log
```

## App

A macOS app, M130a Scan, and a command-line tool, `m130a-scan`, that scan multi-page documents from
the printer's flatbed over USB into one PDF, with no HP software installed. Both talk eSCL to the
printer's IPP-over-USB interface directly; the Rust code is in `app/`.

```
just app-build  # prints where the bundle landed: app/target/release/bundle/macos/M130a Scan.app
```

Using it: choose Colour or Grey and a resolution (150, 300 or 600 dpi), lay a page face down on
the glass and press Scan page. Each page joins the list; reorder pages by dragging them or with
⌘↑/⌘↓, remove the selected one with Delete, and save them as one PDF with ⌘S. Esc cancels a
scan in progress.

The CLI scans a page, asks whether another follows, and writes the PDF when you answer no:

```
m130a-scan status
m130a-scan scan --out file.pdf [--grey] [--dpi 150|300|600] [--force]
```

`status` prints the scanner's model, state, resolutions and colour modes. `scan` defaults to colour
at 300 dpi and will not replace an existing file without `--force`. Run it with
`cargo run --manifest-path app/Cargo.toml -p scan-cli -- status` (arguments after `--`), or build
it once with `cargo build --manifest-path app/Cargo.toml -p scan-cli --release` and use
`app/target/release/m130a-scan`.

`just app-hw-test` runs the tests that need the printer attached by USB and a sheet on the glass:
they read the status and capabilities, scan a 75 dpi grey page, and check that a scan abandoned
partway leaves the printer usable.

Known limitations:

- macOS only.
- Quitting from the Dock menu or at logout does not ask about unsaved pages: tao, Tauri's window
  library, gives the app no hook there. Quitting with ⌘Q or by closing the window does ask.
- If another program abandons a scan mid-transfer, the printer may need a moment: the app drains
  the leftover data and falls back to the second USB interface.

## Checks

```
just test       # probe: pytest
just lint       # probe: ruff, ty
just fw-test    # firmware: host unit tests with UBSan
just fw-lint    # firmware: function size, complexity, parameter and line-length limits
just app-test   # scan app: Rust and UI unit tests, no printer needed
just app-lint   # scan app: rustfmt, clippy, Biome, tsc, size and complexity limits
```

## License

MIT, see [LICENSE](LICENSE).
