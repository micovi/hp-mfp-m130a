# hp-mfp-m130a

Making a USB-only HP LaserJet Pro MFP M130a printable (and ideally scannable) over WiFi, with a
Raspberry Pi Pico 2 W plugged into its USB port. Background and options: [docs/research.md](docs/research.md).

## Setup

Every tool comes from the Nix dev shell in `flake.nix`. With direnv, `direnv allow` once; otherwise
prefix commands with `nix develop --command`.

## Probe

`just probe` talks to the printer over USB from this computer: it lists the USB interfaces, then
asks the IPP interface for the printer's attributes and its eSCL scanner capabilities, and the
LEDM interface for its resource list. It is read-only, never sending a print or scan job. The
answers land in `probe-output/`.

## Checks

```
just test   # pytest
just lint   # ruff, ty
```
