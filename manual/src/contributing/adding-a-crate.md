# Adding a Crate

This document guides you through adding a new crate to the project.

## Decide where it lives

The first step is deciding where the crate should live:
- `lib/` for _standalone libraries_ that could plausibly be useful outside of k23
- `sys/` for subsystem crates that only make sense as part of k23 (e.g. kernel subsystems like the virtual memory subsystem)
- `build/` for tools that run as part of the build process (e.g. disk image creation). These should generally only be simple tools, any complicated logic likely belongs into `lib/`

If you're unsure about where to put a crate, default to `lib/`.

## Crate Layout

Crates generally look the same under [buck2] as they do under [Cargo]: a `src` folder containing your Rust code, a `src/lib.rs` or `src/main.rs` entrypoint. The biggest difference is the `BUCK` file (written in [Starlark]): It is our equivalent of `Cargo.toml` and where you declare all the crates metadata to the build system.

```starlark
# declare the crate so the build system knows about it
rust_library(
    # the name of the crate. rust code imports from this name.
    # the convention is to match the crate dir
    name = "mycrate",
    # buck2 requires you to explicitly declare all source files
    srcs = glob(["**/*.rs"]),
    # and dependencies
    deps = [
        "//lib/util:util",
        "//third-party:cfg-if",
    ],
    # we also require you explicitly list which targets provide tests for this crate 
    # (see below)
    tests = [":mycrate_unittests"],
    # mark this crate as visible to others in this project (so we can depend on it)
    visibility = ["PUBLIC"],
)

# make the unit-tests in this crate visible to buck2 as well.
# without it `just unittest` wont run the unit tests for this crate
rust_test(
    name = "mycrate_unittests",
    srcs = glob(["**/*.rs"]),
    deps = [
        "//lib/util:util",
        "//third-party:cfg-if",
        "//third-party:proptest",  # or whatever the tests need
    ],
    visibility = ["PUBLIC"],
)
```

Of course files like `README.md` or `CHANGELOG.md` belong into the crate directory.

## Depending on your crate

To pull your crate into a consumer, simply add your crates buck path to the consumers `deps` array:

```starlark
deps = [
    "//lib/mycrate:mycrate",
    ...
]
```

## Verify your changes

- Check your Rust code by running `just check //lib/mycrate:mycrate`. This is the equivalent of running `cargo check -p mycrate`.
- Run the new tests you added by running `just unittests //lib/mycrate:mycrate`
- `just preflight` will run as much of the full CI suite locally. Run this before you push! You can also run the full suite for just your crate by running `just preflight //lib/mycrate:mycrate`.

## Conventions & Tips

If your crate has architecture specific dependencies, you can gate them using [`select()`][select]

```starlark
# and dependencies
deps = [
    "//lib/util:util",
    "//third-party:cfg-if",
] + select({
    "prelude//cpu/constraints:riscv64": ["//lib/riscv:riscv"], # if the riscv64 constraint matches, add the riscv dependency
    "DEFAULT": [] # otherwise nothing
})
```


If your crate has special features depending on whether its used in the `kernel` or `loader` (tends to happen sometimes) you can also use [`select()`][select]:

```starlark
features = select({
    "constraints//:env[kernel]": ["thread-local"], # when running inside the kernel thread-locals are available, so lets use them
    "DEFAULT": [] # otherwise we use some fallback mechanism
})
```

## Removing a crate

When you remove a crate, simply delete its directory and remove the crate from any consumers `deps`. You can use the following buck2 query command to list all direct dependents of your crate: `buck2 uquery "rdeps(//..., //lib/mycrate:mycrate, 1)"`.

You will also want to regenerate the `rust-project.json` file by running `just rust-project` so your [rust-analyzer] suggestions are up-to-date.

[buck2]: https://buck2.build/
[Cargo]: https://doc.rust-lang.org/cargo/
[Starlark]: https://github.com/bazelbuild/starlark
[select]: https://buck2.build/docs/rule_authors/configurations/
[rust-analyzer]: https://rust-analyzer.github.io/
