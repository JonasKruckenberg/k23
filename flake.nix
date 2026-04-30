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
        in
        {
          inherit rustToolchain;
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
                  hash = "sha256:706233d79d0906ad15c29be2b5fa50584050a4f65b59cadfae9a5b651fa2a3d5";
                };
                "x86_64-darwin" = {
                  target = "x86_64-apple-darwin";
                  hash = "sha256:e55bbab3b1727e12273cbac9b5b6658cf2422f9fefc38ae2421830c04864b80b";
                };
                "aarch64-linux" = {
                  target = "aarch64-unknown-linux-gnu";
                  hash = "sha256:90e8023cad2e9eb1a3fd315cd17b3147d8979931cdebdca9c57718d5f5b02d69";
                };
                "x86_64-linux" = {
                  target = "x86_64-unknown-linux-gnu";
                  hash = "sha256:f8d9e5d6f9576e2ff6e61bff802a297b5fb472be364364cdbe78adbcdb13cad6";
                };
              };
              info = targets.${pkgs.system};
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
                  hash = "sha256:9d5a8edb6d21a953e04da323548dc37292d1d43ca7aa35b6db2a20b417b8f5e4";
                };
                "x86_64-darwin" = {
                  target = "x86_64-apple-darwin";
                  hash = "sha256:3eaa7a204eb2a4d6b43dc1e8473ba3777bbab0040b9ee4dfaa88f59f4891e5fb";
                };
                "aarch64-linux" = {
                  target = "aarch64-unknown-linux-gnu";
                  hash = "sha256:5faf8f1cddd16510b5473a6ecd666894905050cfeb7dec013cf70cf06d24dd06";
                };
                "x86_64-linux" = {
                  target = "x86_64-unknown-linux-gnu";
                  hash = "sha256:69a801375a159454d737a0a24accb62bdb5f0c668b4b4220a4364eaba809daaf";
                };
              };
              info = targets.${pkgs.system};
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
                  hash = "sha256:90e8023cad2e9eb1a3fd315cd17b3147d8979931cdebdca9c57718d5f5b02d69";
                };
                "x86_64-linux" = {
                  target = "x86_64-unknown-linux-gnu";
                  hash = "sha256:823306ab272e835159eef5fd1772f5745c7302574ca649551b4158b701328674";
                };
              };
              info = targets.${pkgs.system};
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
        in
        {
          default =
            with pkgs;
            mkShell {
              name = "k23-dev";
              buildInputs = [
                # compilers
                rustToolchain

                # build system
                buck2
                reindeer
                supertd
                rust-project

                # version control
                jujutsu

                # devtools
                just
                mdbook
                wabt
                wasm-tools
                typos
                dtc
                cargo-nextest

                # for testing the kernel
                qemu
                socat

                # To profile the code or benchmarks
                samply
              ];
            };
        }
      );
    };
}
