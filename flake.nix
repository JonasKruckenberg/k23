{
  description = "debugger for Hubris";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    crane.url = "github:ipetkov/crane";

    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      advisory-db,
      crane,
      flake-utils,
      rust-overlay,
      treefmt-nix,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      hostSystem:
      let
        pkgs = import nixpkgs {
          system = hostSystem;
          overlays = [ (import rust-overlay) ];
        };

        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        workspaceFiles = nixpkgs.lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          (craneLib.fileset.commonCargoSources ./libs)
          (craneLib.fileset.commonCargoSources ./build)
          (craneLib.fileset.commonCargoSources ./kernel)
          (craneLib.fileset.commonCargoSources ./loader)

          # keep all target json files from the configurations folder
          (nixpkgs.lib.fileset.fileFilter (file: file.hasExt "json") ./configurations)
          ./loader/riscv64-qemu.ld # required for loader
          ./deny.toml # required for cargo-deny
          ./libs/wast/tests/parse-fail # wast test fixtures
        ];
        src = nixpkgs.lib.fileset.toSource {
          root = ./.;
          fileset = workspaceFiles;
        };

        # Arguments shared by all workspace crates
        workspaceArgs = {
          inherit src;
          strictDeps = true;
        };

        # Build *just* the cargo dependencies (of the entire workspace),
        # so we can reuse all of that work (e.g. via cachix) when running in CI
        # It is *highly* recommended to use something like cargo-hakari to avoid
        # cache misses when building individual top-level-crates
        cargoArtifacts = craneLib.buildDepsOnly workspaceArgs;

        individualCrateArgs = workspaceArgs // {
          inherit cargoArtifacts;
          inherit (craneLib.crateNameFromCargoToml { inherit src; }) version;
          # NB: we disable tests since we'll run them all via cargo-nextest
          doCheck = false;
        };

        fileSetForCrate =
          crate:
          nixpkgs.lib.fileset.toSource {
            root = ./.;
            fileset = nixpkgs.lib.fileset.unions [
              workspaceFiles
              (craneLib.fileset.commonCargoSources crate)
            ];
          };

        makeCrossArgs = target: {
          cargoVendorDir = craneLib.vendorMultipleCargoDeps {
            inherit (craneLib.findCargoFiles src) cargoConfigs;
            cargoLockList = [
              ./Cargo.lock
              "${rustToolchain.passthru.availableComponents.rust-src}/lib/rustlib/src/rust/library/Cargo.lock"
            ];
          };
          CARGO_BUILD_TARGET = target;
        };

        treefmtEval = treefmt-nix.lib.evalModule pkgs {
          projectRootFile = "flake.nix";

          programs = {
            nixfmt.enable = true; # nix
            rustfmt = {
              # Rust
              enable = true;
              package = rustToolchain;
            };
            taplo.enable = true; # toml
            yamlfmt.enable = true; # yaml
          };
        };
      in
      {
        formatter = treefmtEval.config.build.wrapper;

        packages = rec {
          kernel-cross-riscv64gc-k23-none-kernel = pkgs.callPackage ./kernel {
            inherit (makeCrossArgs "configurations/riscv64/riscv64gc-k23-none-kernel.json")
              cargoVendorDir
              CARGO_BUILD_TARGET
              ;
            inherit craneLib individualCrateArgs fileSetForCrate;
          };

          loader-cross-riscv64gc-unknown-none-elf = pkgs.callPackage ./loader {
            inherit (makeCrossArgs "riscv64gc-unknown-none-elf") cargoVendorDir CARGO_BUILD_TARGET;
            inherit craneLib individualCrateArgs fileSetForCrate;
            KERNEL = kernel-cross-riscv64gc-k23-none-kernel;
          };
        };

        checks = {
          formatting = treefmtEval.config.build.check self;

          clippy = craneLib.cargoClippy (
            workspaceArgs
            // {
              inherit cargoArtifacts;

              cargoClippyExtraArgs = "--workspace --exclude loader --exclude kernel --all-targets -- --deny warnings";
            }
          );

          doc = craneLib.cargoDoc (
            workspaceArgs
            // {
              inherit cargoArtifacts;

              cargoDocExtraArgs = "--workspace --exclude loader --exclude kernel";

              # This can be commented out or tweaked as necessary, e.g. set to
              # `--deny rustdoc::broken-intra-doc-links` to only enforce that lint
              env.RUSTDOCFLAGS = "--deny warnings";
            }
          );

          audit = craneLib.cargoAudit {
            inherit src advisory-db;
          };

          deny = craneLib.cargoDeny {
            inherit src;
          };

          nextest = craneLib.cargoNextest (
            workspaceArgs
            // {
              inherit cargoArtifacts;
              cargoNextestExtraArgs = "--workspace --exclude loader --exclude kernel";
              partitions = 1;
              partitionType = "count";
              cargoNextestPartitionsExtraArgs = "--no-tests=pass";
            }
          );
        };

        devShells.default = craneLib.devShell {
          # Inherit inputs from checks.
          checks = self.checks.${hostSystem};

          # Additional dev-shell environment variables can be set directly
          # MY_CUSTOM_DEVELOPMENT_VAR = "something else";

          # Extra inputs can be added here; cargo and rustc are provided by default.
          packages =
            with pkgs;
            [
              # devtools
              just
              typos
              dtc
              cargo-fuzz

              # for manual
              mdbook

              # wasm tooling
              wabt
              wasm-tools

              # for testing the kernel
              qemu
              socat

              # To profile the code or benchmarks
              samply
            ]
            ++ lib.optionals pkgs.stdenv.isLinux [
              # To profile the code or benchmarks
              perf

              # For valgrind
              valgrind
              cargo-valgrind
            ];
        };
      }
    );
}
