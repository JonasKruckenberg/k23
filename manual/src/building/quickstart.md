# How to Build and Run K23

## Prerequisites

Building k23 needs a lot of tools (rustc, buck2, qemu, and more) at specific versions [^1]. Rather than ask you to chase all of these down by hand, we lean on Nix to pin them all.

This makes Nix the one thing you *do* have to install yourself. Grab it via the [Determinate Installer](https://docs.determinate.systems/getting-started/individuals) or [upstream nix](https://nixos.org/download/), and enable flakes (`experimental-features = nix-command flakes` in `~/.config/nix/nix.conf`. The Determinate Installer sets this for you).

Linux and macOS are supported on x86_64 and aarch64; on Windows you'll want to develop from inside WSL2 as Nix doesn't run natively there.

## Entering the dev shell

With Nix installed, running `nix develop -c $SHELL` drops you into a shell with every required tool in `PATH` [^2].

## Running

Inside the dev shell run `just run //sys:k23-qemu-riscv64` which builds k23 for riscv64 and boots it under QEMU.

Type `just` (no args) to list every recipe available. This includes convenient recipes for running linters, tests and more. Every recipe is a thin wrapper around `buck2` (`just run //sys:k23-qemu-riscv64` is roughly `buck2 run //sys:k23-qemu-riscv64`).

[^1]: We've had issues with _wildly_ outdated QEMU versions in linux package repositories for example.
[^2]: The `-c $SHELL` part instructs nix to use your current shell binary, otherwise it defaults to bash, yuck.
