{
  description = "hp-mfp-m130a: every tool the justfile needs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    # pico-sdk 2.3.1 bundles TinyUSB 0.18; the RP2350 host fixes are in 0.21.0.
    tinyusb = {
      url = "github:hathach/tinyusb/0.21.0";
      flake = false;
    };
  };

  outputs =
    { nixpkgs, tinyusb, ... }:
    let
      # nixpkgs-unstable no longer builds for Intel Macs (x86_64-darwin).
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forEachSystem = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forEachSystem (
        pkgs:
        let
          # With cyw43-driver and lwIP, which the WiFi milestone needs.
          picoSdk = pkgs.pico-sdk.override { withSubmodules = true; };
        in
        {
          default = pkgs.mkShell {
            packages = [
              # The USB probe (just probe). nixpkgs' pyusb is patched to load nixpkgs' libusb, so
              # nothing comes from Homebrew or the system Python.
              (pkgs.python313.withPackages (ps: [
                ps.pyusb
                ps.pytest
              ]))
              # Lint, format and type check (just lint).
              pkgs.ruff
              pkgs.ty
              # Runs the recipes in the justfile.
              pkgs.just
              # The firmware (just fw-build, just fw-test).
              pkgs.gcc-arm-embedded
              pkgs.cmake
              pkgs.ninja
              pkgs.picotool
              # Flash through the Debug Probe (just flash) and read the Pico's UART (just logs).
              pkgs.probe-rs-tools
              pkgs.tio
            ];

            PICO_SDK_PATH = "${picoSdk}/lib/pico-sdk";
            PICO_TINYUSB_PATH = "${tinyusb}";
            # pico-sdk builds picotool from source unless it finds this one.
            picotool_DIR = "${pkgs.picotool}/lib/cmake/picotool";
          };
        }
      );
    };
}
