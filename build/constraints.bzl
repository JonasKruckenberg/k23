# Shared `target_compatible_with` constraint lists.

# Bare-metal target platforms (os=none), any cpu. A target marked with this is
# reported as incompatible — rather than silently built for the host — unless a
# bare-metal platform such as `--target-platforms //platforms:riscv64` (or
# aarch64/x86_64) is selected. Used by the kernel, the loader binaries, and the
# portable no_std loader libraries they share.
BARE_METAL = ["prelude//os/constraints:none"]
