{
  description = "Flake providing a development shell for k23";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
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

            # version control
            jujutsu

            # devtools
            just
            cargo-nextest
            cargo-deny
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
          ] ++ lib.optionals pkgs.stdenv.isLinux [
            # To profile the code or benchmarks
            perf

            # For valgrind
            valgrind
            cargo-valgrind
          ];

          LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
          LD_LIBRARY_PATH = "${lib.makeLibraryPath buildInputs}";
        };
      }
    );
}
