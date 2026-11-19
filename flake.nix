{
  description = "harbor-mcp: semver-correct, dependency-ordered version bumps across Rust and Python repositories";

  # Pinned to harbor's root revisions: harbor imports this flake as a
  # component and its lock-alignment check compares these pins.
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/8edc0c72e3a38faf5434e40d1f19431125fe4b30";
    harbor-meta = {
      url = "github:caniko/harbor-meta/b494b56e8174557b27956108d3ed7dd0e6425d0b";
      flake = false;
    };
    harbor-rs = {
      url = "github:caniko/harbor-rs/82206bdc5be73b81992917ce765073db0f446f01";
      flake = false;
    };
    crane.url = "github:ipetkov/crane/eb35abda9f232cc6610b1d1e3200d15c49b7ac54";
    rust-overlay = {
      url = "github:oxalica/rust-overlay/c1cfa7a2c809d364ac1a7e9b3ec418afa3f19793";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    treefmt-nix = {
      url = "github:numtide/treefmt-nix/27b3b12a8e6375f28ebe122f07d230ca5459bbfa";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs @ {
    self,
    nixpkgs,
    ...
  }: let
    lib = nixpkgs.lib;
    systems = ["x86_64-linux" "aarch64-linux" "aarch64-darwin"];
    rust = import (inputs.harbor-rs + "/lib") {
      inherit (inputs) crane;
      osxcross = {};
    };
    per = lib.genAttrs systems (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [inputs.rust-overlay.overlays.default];
      };
      toolchain = rust.mkToolchain {
        inherit pkgs;
        withRustAnalyzer = false;
        cache.enable = false;
      };
      inherit (toolchain) craneLib;
      common = {
        src = craneLib.cleanCargoSource ./.;
        strictDeps = true;
      };
      cargoArtifacts = craneLib.buildDepsOnly common;
      unwrapped = craneLib.buildPackage (common
        // {
          inherit cargoArtifacts;
          # The end-to-end tests build fixture Git repositories and run griffe.
          nativeCheckInputs = [pkgs.git pkgs.python3Packages.griffe];
        });
      # The delegated checkers ship with the binary; cargo comes from the
      # repository being planned, since semver-checks builds its rustdoc.
      package = pkgs.symlinkJoin {
        name = "harbor-mcp-${unwrapped.version}";
        paths = [unwrapped];
        nativeBuildInputs = [pkgs.makeWrapper];
        postBuild = ''
          wrapProgram $out/bin/harbor-mcp --suffix PATH : ${lib.makeBinPath [pkgs.git pkgs.cargo-semver-checks pkgs.python3Packages.griffe]}
        '';
        meta.mainProgram = "harbor-mcp";
      };
      treefmtArgs = {
        harbor-meta.treefmtModules = {
          nix = "${inputs.harbor-meta}/nix/treefmt/nix.nix";
          toml = "${inputs.harbor-meta}/nix/treefmt/toml.nix";
        };
        harbor-rs.treefmtModules.rust = "${inputs.harbor-rs}/nix/treefmt/rust.nix";
        rustfmtPackage = toolchain.rustToolchain;
      };
      format = inputs.treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix {
        inherit (treefmtArgs) harbor-meta harbor-rs rustfmtPackage;
      });
    in {
      packages = {
        default = package;
        harbor-mcp = package;
      };
      checks = {
        harbor-mcp = unwrapped;
        clippy = craneLib.cargoClippy (common
          // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });
        formatting = format.config.build.check self;
      };
      formatter = format.config.build.wrapper;
      devShells.default = pkgs.mkShell {
        packages = [
          toolchain.rustToolchain
          format.config.build.wrapper
          pkgs.cargo-semver-checks
          pkgs.python3Packages.griffe
          pkgs.git
        ];
      };
    });
  in {
    packages = lib.mapAttrs (_: s: s.packages) per;
    checks = lib.mapAttrs (_: s: s.checks) per;
    formatter = lib.mapAttrs (_: s: s.formatter) per;
    devShells = lib.mapAttrs (_: s: s.devShells) per;
  };
}
