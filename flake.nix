{
  description = "Buck2 toolchain flake";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      inherit (nixpkgs) lib;
      defaultSystems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
      forAllSystems =
        fn:
        lib.genAttrs defaultSystems (
          system:
          let
            pkgs = import nixpkgs {
              inherit system;
              overlays = [ (import rust-overlay) ];
            };
          in
          fn pkgs
        );
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          rustToolchain = with pkgs; rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

          # libstdc++ (linux) / libc++ (darwin) shared by stdenv's cc.
          # The cxx toolchain bakes its lib dir into binaries as -rpath
          # so they can find it at runtime.
          cxxRuntimeLib = pkgs.stdenv.cc.cc.lib;

          # Target-agnostic LLVM binutils, used to manipulate cross-compiled
          # ELFs (e.g. riscv64) from the host without a cross-binutils.
          llvmBintools_20 = pkgs.llvmPackages_20.bintools-unwrapped;
        in
        {
          inherit rustToolchain cxxRuntimeLib llvmBintools_20;
          inherit (pkgs)
            bash
            python3
            lld_20
            clang_20
            mdbook
            qemu
            ;
        }
      );

      devShells = forAllSystems (
        pkgs:
        let
          rustToolchain = with pkgs; rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

          buck2 =
            let
              targets = {
                "aarch64-darwin" = {
                  target = "aarch64-apple-darwin";
                  hash = "sha256:e43db992790715f26d5ca4a34e5cc89fbb0d2bd182e36882aa2275743005ea0b";
                };
                "x86_64-darwin" = {
                  target = "x86_64-apple-darwin";
                  hash = "sha256:7c640b53bda8fc09b3528b9215f5bec4da5981b621b488696f0505e45a41a70f";
                };
                "aarch64-linux" = {
                  target = "aarch64-unknown-linux-gnu";
                  hash = "sha256:20cf12e00eb84155d83a9436954db97c52b2d9851d7de1344a0537c46bc5e324";
                };
                "x86_64-linux" = {
                  target = "x86_64-unknown-linux-gnu";
                  hash = "sha256:80c09f42155ac08ffa7d88e8d25043fdd4b1491163a8422b8cc228147b391d77";
                };
              };
              info = targets.${pkgs.stdenv.hostPlatform.system};
            in
            pkgs.stdenvNoCC.mkDerivation {
              pname = "buck2";
              version = "2026-10-25";

              src = pkgs.fetchurl {
                url = "https://github.com/JonasKruckenberg/buck2/releases/download/2026-10-25/buck2-${info.target}.zst";
                hash = info.hash;
              };

              nativeBuildInputs = [
                pkgs.zstd
              ]
              ++ lib.optionals pkgs.stdenvNoCC.hostPlatform.isLinux [ pkgs.autoPatchelfHook ];
              buildInputs = lib.optionals pkgs.stdenvNoCC.hostPlatform.isLinux [
                pkgs.stdenv.cc.cc.lib
              ];

              dontUnpack = true;

              installPhase = ''
                zstd -d "$src" -o buck2
                install -Dm755 buck2 "$out/bin/buck2"
              '';
            };

          rust-project =
            let
              targets = {
                "aarch64-darwin" = {
                  target = "aarch64-apple-darwin";
                  hash = "sha256:cf6b71f59dd61aa8b0eb1459cd85c2d0bf85f330334c4a080686a8665078ea4f";
                };
                "x86_64-darwin" = {
                  target = "x86_64-apple-darwin";
                  hash = "sha256:bd9e5bc77fd038cb59edb610dd7ddaca27ee9307de17b1fccba3bdeefc2f7b5d";
                };
                "aarch64-linux" = {
                  target = "aarch64-unknown-linux-gnu";
                  hash = "sha256:f428b424aabf71bf3f16483bc9987c8714abb345ca5ed99dd5c758032728b880";
                };
                "x86_64-linux" = {
                  target = "x86_64-unknown-linux-gnu";
                  hash = "sha256:ac067e8a3f2bc421e3623e8897f41da2a51dede03fb2e6871dd13df8999dc27d";
                };
              };
              info = targets.${pkgs.stdenv.hostPlatform.system};
            in
            pkgs.stdenvNoCC.mkDerivation {
              pname = "rust-project";
              version = "2026-10-25";

              src = pkgs.fetchurl {
                url = "https://github.com/JonasKruckenberg/buck2/releases/download/2026-10-25/rust-project-${info.target}.zst";
                hash = info.hash;
              };

              nativeBuildInputs = [
                pkgs.zstd
              ]
              ++ lib.optionals pkgs.stdenvNoCC.hostPlatform.isLinux [ pkgs.autoPatchelfHook ];
              buildInputs = lib.optionals pkgs.stdenvNoCC.hostPlatform.isLinux [
                pkgs.stdenv.cc.cc.lib
              ];

              dontUnpack = true;

              installPhase = ''
                zstd -d "$src" -o rust-project
                install -Dm755 rust-project "$out/bin/rust-project"
              '';
            };

          reindeer =
            let
              src = pkgs.fetchFromGitHub {
                owner = "JonasKruckenberg";
                repo = "reindeer";
                rev = "86a2896136faf1582b9acf45c0b0be38b74a3ca8";
                hash = "sha256-lbS5OPZZLP7eP8IuwVFUQtJ5wkF3UeZYR15vXy/hxTU=";
              };
            in
            pkgs.reindeer.overrideAttrs (old: {
              inherit src;

              cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
                inherit src;
                hash = "sha256-MB/TQnOB96x86uAQJNt6IiTUmy74Dom4xtqWGga6GkA=";
              };

              checkFlags = (old.checkFlags or [ ]) ++ [
                "--skip=rlimit::tests::raise_does_not_lower_limit"
              ];
            });

          # Tools every current CI job needs. Anything outside this list
          # is interactive-only; keeping it small shrinks the closure that
          # cold CI runners have to fetch and realise.
          #
          # rust-project and typos are listed here only because the
          # justfile resolves them via `require()` at parse time, so
          # every `just <recipe>` invocation needs them in PATH.
          ciInputs = with pkgs; [
            rustToolchain
            buck2
            reindeer
            rust-project
            jujutsu
            just
            cargo-deny
            typos
            jq
            zstd
          ];

          # Extra tooling for jobs that exercise the kernel on-target.
          ciTestInputs = with pkgs; [
            qemu
          ];

          # Tools only useful in an interactive shell.
          devOnlyInputs = with pkgs; [
            mdbook
            wabt
            wasm-tools
            dtc
            cargo-nextest
            samply
            socat
          ];
        in
        {
          default = pkgs.mkShell {
            name = "k23-dev";
            buildInputs = ciInputs ++ ciTestInputs ++ devOnlyInputs;
          };

          ci = pkgs.mkShell {
            name = "k23-ci";
            buildInputs = ciInputs;
          };

          ci-test = pkgs.mkShell {
            name = "k23-ci-test";
            buildInputs = ciInputs ++ ciTestInputs;
          };
        }
      );
    };
}
