{
  description = "Flake providing a development shell for k23";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
        url = "github:oxalica/rust-overlay";
        inputs = {
            nixpkgs.follows = "nixpkgs";
        };
    };
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        rustToolchain = with pkgs; rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      in
      {
        devShells.default = with pkgs; mkShell rec {
          name = "k23-dev";
          buildInputs = [
            # compilers
            rustToolchain
            clang

            # devtools
            just
            mdbook
            socat
            wabt
            dtc
            cargo-nextest
            cargo-fuzz
            cargo-deny
            typos
            wasm-tools
            jujutsu

            # for testing the kernel
            qemu
          ];

          LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
          LD_LIBRARY_PATH = "${lib.makeLibraryPath buildInputs}";
        };
      }
    );
}
