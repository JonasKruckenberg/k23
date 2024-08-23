# `kconfig` Build Configuration

Every operating system has a myriad of knobs and switches which control its behavior. Some are configured at runtime,
others are constants that are set at compile time. `kconfig` is k23's build configuration system, which is used to
manage **compile-time** configuration options.

`kconfig` options are configured in a `.toml` file that has to be provided to the build system.

## Declare a Configuration Symbol

Options that are set through `kconfig` are called *Symbols* and are declared using the `kconfig_declare::symbol`
attribute proc-macro.

```rust
/// The size of the kernel stack in pages.
#[kconfig_declare::symbol("kernel.stack-size-pages")]
pub const STACK_SIZE_PAGES: usize = 32;
```

The above will declare a configuration symbol named `STACK_SIZE_PAGES`. It will read `kernel.stack-size-pages` key from
the provided `.toml` file. `32` is the default value if the key is not found in the `.toml` file.

```toml
[kernel]
stack-size-pages = 32 # The kconfig symbol will be read from this key
```

## Fallback Paths

You can provide multiple paths to search for the key in the `.toml` file. The first path that is found will be used.

```rust
#[kconfig_declare::symbol("kernel.stack-size-pages", "kernel.stack-size")]
pub const STACK_SIZE_PAGES: usize = 32;
```

## Custom Parse Function

By default `kconfig` only works with the basic toml types (`bool`, `int`, `float`, `String`, `Array`, `Table`) and their
Rust equivalents (`kconfig` will attempt to coerce `int` and `float` types to the Rust type you provided).
If you need to parse a custom type, you can provide a custom parse function.

```rust
#[kconfig_declare::symbol({
    paths: ["kernel.log-level"],
    parse: parse_log_lvl
})]
pub const LOG_LEVEL: LogLevel = LogLevel::Trace;

const fn parse_log_lvl(s: &str) -> log::Level {
    match s.as_bytes() {
        b"error" => log::Level::Error,
        b"warn" => log::Level::Warn,
        b"info" => log::Level::Info,
        b"debug" => log::Level::Debug,
        b"trace" => log::Level::Trace,
        _ => panic!(),
    }
}
```

The above symbol will attempt to read a `String` value from the `kernel.log-level` key in the `.toml` file. It will then
call the `parse_log_lvl` function to parse the value into a `log::Level`.

> **Note:** The `parse` function must be a `const fn` since it is evaled at compile time. If you need to do more complex
> parsing consider offloading as much work to the compile-time parse fn as possible and perform the rest at runtime.