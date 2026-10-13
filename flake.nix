{
  description = "hp-mfp-m130a: every tool the justfile needs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    { nixpkgs, ... }:
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
      devShells = forEachSystem (pkgs: {
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
          ];
        };
      });
    };
}
