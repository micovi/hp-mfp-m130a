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

## Checks

```
just test       # probe: pytest
just lint       # probe: ruff, ty
just fw-test    # firmware: host unit tests with UBSan
just fw-lint    # firmware: function size, complexity, parameter and line-length limits
```

## License

MIT, see [LICENSE](LICENSE).
