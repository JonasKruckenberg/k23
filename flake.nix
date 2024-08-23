# forked from https://github.com/tosc-rs/mnemos/blob/main/flake.nix
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

    outputs = { nixpkgs, flake-utils, rust-overlay, ... }:
        flake-utils.lib.eachDefaultSystem (system:
            let
                overlays = [ (import rust-overlay) ];
                pkgs = import nixpkgs { inherit system overlays; };
                # use the Rust toolchain specified in the project's rust-toolchain.toml
                rustToolchain = pkgs.pkgsBuildHost.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
            in
            {
                devShell = with pkgs; mkShell rec {
                    name = "k23-dev";
                    nativeBuildInputs = [
                        # compilers
                        rustToolchain
                        clang

                        # devtools
                        just
                        nushell
                        cargo-binutils
                        mdbook
                        lz4
                        openssl
                        coreutils

                        # for testing the kernel
                        qemu
                    ];
                    buildInputs = [];

                    LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
                    LD_LIBRARY_PATH = "${lib.makeLibraryPath buildInputs}";
                };
            }
        );
}