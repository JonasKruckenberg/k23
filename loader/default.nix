{
  lib,
  craneLib,
  individualCrateArgs,
  fileSetForCrate,
  cargoVendorDir,
  CARGO_BUILD_TARGET,
  KERNEL,
}:
craneLib.buildPackage (
  individualCrateArgs
  // {
    inherit cargoVendorDir;

    pname = "loader";
    cargoExtraArgs = "-p loader -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem";

    src = fileSetForCrate ./.;
    strictDeps = true;

    # Don't try to patch ELF binaries - these are bare metal
    dontPatchELF = true;
    dontFixup = true;

    CARGO_BUILD_TARGET = CARGO_BUILD_TARGET;
    KERNEL = KERNEL + /bin/kernel;
  }
)
