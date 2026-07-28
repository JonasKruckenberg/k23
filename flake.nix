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
                  hash = "sha256:4c5e084193ee57a6db9dd21501f7d41e6f59cf90f4172c7c3e1399153885164f";
                };
                "x86_64-darwin" = {
                  target = "x86_64-apple-darwin";
                  hash = "sha256:ed6797240fc3e597ff13d4449dba7e4efd04243afc7a264c272ae410280cf241";
                };
                "aarch64-linux" = {
                  target = "aarch64-unknown-linux-gnu";
                  hash = "sha256:06e24015b193560a594960cb7cd14d0fcd664aa29ed92ab7e5fcb8d72d1f9306";
                };
                "x86_64-linux" = {
                  target = "x86_64-unknown-linux-gnu";
                  hash = "sha256:1704c249c817d1025ff240fd36252b21299f89fa2eeefa9090c2d5712476784f";
                };
              };
              info = targets.${pkgs.stdenv.hostPlatform.system};
            in
            pkgs.stdenvNoCC.mkDerivation {
              pname = "buck2";
              version = "latest";

              src = pkgs.fetchurl {
                url = "https://github.com/JonasKruckenberg/buck2/releases/download/latest/buck2-${info.target}.zst";
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
                  hash = "sha256:7486d204ce56a7f4aad6d02812ffbd4d59e4a54f345c94e37c2ac40fe29aab9b";
                };
                "x86_64-darwin" = {
                  target = "x86_64-apple-darwin";
                  hash = "sha256:305835cd9ad6e09ac4d17107644aa84457930e24a5970e1722e819bf58248e52";
                };
                "aarch64-linux" = {
                  target = "aarch64-unknown-linux-gnu";
                  hash = "sha256:f661f68a2ebb3dd136fff2e5cae7f2ea0c897d5c20e557c86cf6ae175c437756";
                };
                "x86_64-linux" = {
                  target = "x86_64-unknown-linux-gnu";
                  hash = "sha256:42166db3e3253fa33bc721987a483dfcacb0302bff8680096899f3d112ed6be0";
                };
              };
              info = targets.${pkgs.stdenv.hostPlatform.system};
            in
            pkgs.stdenvNoCC.mkDerivation {
              pname = "rust-project";
              version = "latest";

              src = pkgs.fetchurl {
                url = "https://github.com/JonasKruckenberg/buck2/releases/download/latest/rust-project-${info.target}.zst";
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

          supertd =
            let
              targets = {
                "aarch64-darwin" = {
                  target = "aarch64-apple-darwin";
                  hash = "sha256:4128307dd64c31c5d932ea67498d98dfeed02d8b8d88ae826f6b2323e75b3c78";
                };
                "x86_64-darwin" = {
                  target = "x86_64-apple-darwin";
                  hash = "sha256:89f7cb0510470fe37069372a4a5a2ad730807e0bac24e0c68adb666bc6502da9";
                };
                "aarch64-linux" = {
                  target = "aarch64-unknown-linux-gnu";
                  hash = "sha256:7058814f403ac56c19910749b7240234d08c897da1d934b2a648e94deb355a4b";
                };
                "x86_64-linux" = {
                  target = "x86_64-unknown-linux-gnu";
                  hash = "sha256:ed7617a0e5d45d929f34a40a88f03040bdc24b1351a606d4cb8edf8da84c1820";
                };
              };
              info = targets.${pkgs.stdenv.hostPlatform.system};
            in
            pkgs.stdenvNoCC.mkDerivation {
              pname = "supertd";
              version = "latest";

              src = pkgs.fetchurl {
                url = "https://github.com/JonasKruckenberg/buck2-change-detector/releases/download/latest/supertd-${info.target}.zst";
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
                zstd -d "$src" -o supertd
                install -Dm755 supertd "$out/bin/supertd"
              '';
            };
          # Upstream reindeer's rlimit test fails on Darwin sandboxes
          # where the soft RLIMIT_NOFILE starts above the hard limit.
          reindeer = pkgs.reindeer.overrideAttrs (old: {
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
            supertd
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
