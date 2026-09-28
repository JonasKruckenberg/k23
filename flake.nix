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
                  hash = "sha256:e1e1c1772cefd73865418f8b74d974fa059e400e334afd4d086e903070985e35";
                };
                "x86_64-darwin" = {
                  target = "x86_64-apple-darwin";
                  hash = "sha256:bb7badf2a5d32fe2f22dad11d149f3d16be4a149bbe40f3f8be3b54b56e3204c";
                };
                "aarch64-linux" = {
                  target = "aarch64-unknown-linux-gnu";
                  hash = "sha256:99a158d8744a7ebee0a0feca1c3de775b8246f48597dfa262f50dba6c5f2dc77";
                };
                "x86_64-linux" = {
                  target = "x86_64-unknown-linux-gnu";
                  hash = "sha256:41d797c5e705126d977b91eeeb83313e5c4324785c9aec6dddff3d82105423fa";
                };
              };
              info = targets.${pkgs.stdenv.hostPlatform.system};
            in
            pkgs.stdenvNoCC.mkDerivation {
              pname = "buck2";
              version = "2026-09-02";

              src = pkgs.fetchurl {
                url = "https://github.com/JonasKruckenberg/buck2/releases/download/2026-09-02/buck2-${info.target}.zst";
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
                  hash = "sha256:e802c97f60bb2339ae6929682cddf0bdbf91fe1a5cd21123906981409f6a23e9";
                };
                "x86_64-darwin" = {
                  target = "x86_64-apple-darwin";
                  hash = "sha256:fb86c2cc1479b7ad8d56026d1eaae6d5531db53e0d35c81d7f9d4eb51c7f0f75";
                };
                "aarch64-linux" = {
                  target = "aarch64-unknown-linux-gnu";
                  hash = "sha256:de47fd2c68a5e24d04fc7099b31af0cec7749bf7e5b5866d599c6b42c0724366";
                };
                "x86_64-linux" = {
                  target = "x86_64-unknown-linux-gnu";
                  hash = "sha256:483a6baae7f77c194bf3fe452a423908f100203ce54f1245e655f0fc1ea6cdc2";
                };
              };
              info = targets.${pkgs.stdenv.hostPlatform.system};
            in
            pkgs.stdenvNoCC.mkDerivation {
              pname = "rust-project";
              version = "2026-09-02";

              src = pkgs.fetchurl {
                url = "https://github.com/JonasKruckenberg/buck2/releases/download/2026-09-02/rust-project-${info.target}.zst";
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
