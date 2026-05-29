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
                  hash = "sha256:7d4ef790f16a74978efd3436361d6d6d3742fdde61eeee8e6658cfb92c3d0441";
                };
                "x86_64-darwin" = {
                  target = "x86_64-apple-darwin";
                  hash = "sha256:fcc0df4347acde32ce87261d00680efaa80aa653bc5ef16667255f79339d1f3e";
                };
                "aarch64-linux" = {
                  target = "aarch64-unknown-linux-gnu";
                  hash = "sha256:0650eb4cf7c617a017617c699b2970e56dfdf149724238d5dbbfadc24030a233";
                };
                "x86_64-linux" = {
                  target = "x86_64-unknown-linux-gnu";
                  hash = "sha256:84b0d88554a04c885071abcf27026ef72e26282a310101891c3196180f10edd5";
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
                  hash = "sha256:5914aa111bce7961456437faa97606941a93156e18fb2e19092bd4dd163d9654";
                };
                "x86_64-darwin" = {
                  target = "x86_64-apple-darwin";
                  hash = "sha256:7b037be2061cf0fe3a2b91e94f6c7708cbf39cfc53205ecb4b469dc9193e3c62";
                };
                "aarch64-linux" = {
                  target = "aarch64-unknown-linux-gnu";
                  hash = "sha256:29823d55ef811ad8f2d39c4756897642fb08c67680d0997fd2654a171f927ea3";
                };
                "x86_64-linux" = {
                  target = "x86_64-unknown-linux-gnu";
                  hash = "sha256:7cf69a229ec30358dfd1c8c7bcb04259a045a884988fa756a6839f3bce708aa1";
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
