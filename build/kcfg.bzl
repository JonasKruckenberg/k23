# Strongly-typed wrappers around Buck2's buckconfig system.
#
# Two-layer design:
#
#   1. Type descriptors  (kcfg.string / kcfg.bool / kcfg.int / kcfg.enum /
#                         kcfg.list / kcfg.set)
#      Declare the shape, doc, and default of a config option.  No buckconfig
#      location is attached yet.  Primitive descriptors are also used as the
#      `item` argument to kcfg.list / kcfg.set.
#
#   2. kcfg.declare(ty, section, key)
#      Binds a type descriptor to a concrete buckconfig location and returns a
#      struct with:
#        .read()                     – coerced value at analysis time
#        .config_setting(name, value)– macro to declare a config_setting target
#
# Usage:
#
#   load("//build:kcfg.bzl", "kcfg")
#
#   # Declare types (e.g. in a central options file)
#   LogLevel = enum("error", "warn", "info", "debug", "trace")
#
#   LOG_LEVEL  = kcfg.declare(kcfg.enum(LogLevel, default = LogLevel("warn")), "k23", "log_level")
#   STACK_SIZE = kcfg.declare(kcfg.int(default = 512),                          "k23", "stack_size_kb")
#   HOSTS      = kcfg.declare(kcfg.list(kcfg.string(), default = ["localhost"]), "k23", "hosts")
#   FEATURES   = kcfg.declare(kcfg.set(kcfg.string()),                           "k23", "features")
#
#   # In a BUCK file, create selectable targets:
#   LOG_LEVEL.config_setting(name = "log_level_debug", value = "debug")
#
#   # In a rule implementation:
#   def _impl(ctx):
#       level = LOG_LEVEL.read()   # -> LogLevel("warn") or whatever is set

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _ck(section, key):
    return "{}.{}".format(section, key)

# ---------------------------------------------------------------------------
# Type descriptors — primitives
# ---------------------------------------------------------------------------

def _string(doc = "", default = None):
    """Type descriptor for a plain string buckconfig value."""
    return struct(
        type_name = "string",
        doc       = doc,
        default   = default,
        parse     = lambda raw: raw,
        serialize = lambda val: val,
    )

def _bool(doc = "", default = None):
    """Type descriptor for a boolean buckconfig value ('true' / 'false')."""
    def _parse(raw):
        if raw == "true":
            return True
        if raw == "false":
            return False
        fail("expected 'true' or 'false', got '{}'".format(raw))

    return struct(
        type_name = "bool",
        doc       = doc,
        default   = default,
        parse     = _parse,
        serialize = lambda val: "true" if val else "false",
    )

def _int(doc = "", default = None):
    """Type descriptor for an integer buckconfig value."""
    return struct(
        type_name = "int",
        doc       = doc,
        default   = default,
        parse     = int,
        serialize = str,
    )

def _enum(enum_type, doc = "", default = None):
    """Type descriptor for an enum buckconfig value.

    enum_type – the Starlark enum created with enum("val1", "val2", ...)
    config_setting value must be the string key (e.g. "warn").
    """
    return struct(
        type_name = "enum",
        doc       = doc,
        default   = default,
        parse     = enum_type,
        serialize = lambda val: val if type(val) == type("") else str(val),
    )

# ---------------------------------------------------------------------------
# Type descriptors — composites (list / set)
#
# `parse` receives the full raw buckconfig string (whitespace-separated tokens)
# and returns the typed collection.
# `serialize` converts a typed collection back to a whitespace-separated string
# for use in config_setting values.
# ---------------------------------------------------------------------------

def _list(item, doc = "", default = None):
    """Type descriptor for a list buckconfig value.

    item – a primitive type descriptor (kcfg.string(), kcfg.int(), …)

    In .buckconfig, items are whitespace-separated:
        [section]
          key = foo bar baz
          # or with backslash continuation across lines
    """
    def _parse(raw):
        return [item.parse(t) for t in raw.split()]

    def _serialize(val):
        inner = ", ".join(sorted([item.serialize(v) for v in val]))
        return f"[{inner}]"

    return struct(
        type_name = "list[{}]".format(item.type_name),
        doc       = doc,
        default   = default,
        parse     = _parse,
        serialize = _serialize,
    )

def _set(item, doc = "", default = None):
    """Type descriptor for a set buckconfig value.

    Same whitespace-separated INI format as list, but parse() validates that no
    token appears more than once before returning a set[T].
    config_setting matches the sorted, space-joined representation.
    """
    def _parse(raw):
        tokens = raw.split()
        seen = []
        for t in tokens:
            if t in seen:
                fail("duplicate item '{}' in buckconfig value".format(t))
            seen.append(t)
        return set([item.parse(t) for t in tokens])

    def _serialize(val):
        inner = ", ".join(sorted([item.serialize(v) for v in val]))
        return f"[{inner}]"

    return struct(
        type_name = "set[{}]".format(item.type_name),
        doc       = doc,
        default   = default,
        parse     = _parse,
        serialize = _serialize,
    )

# ---------------------------------------------------------------------------
# kcfg.declare — bind a type descriptor to a buckconfig location
# ---------------------------------------------------------------------------

def _declare(section: str = "", key: str = "", type = None, doc: str = ""):
    """Bind a type descriptor to a concrete buckconfig section + key.

    Returns a struct with:
      .read()                        – returns the typed value at analysis time
      .config_setting(name, value)   – macro: declares a config_setting target
                                       that matches when [section] key == value
    """
    ck = _ck(section, key)

    default_str = type.serialize(type.default) if type.default != None else "-"

    def _read():
        raw = read_config(section, key, None)
        if raw == None:
            return type.default
        return type.parse(raw)

    def _config_setting(name, value, visibility = ["PUBLIC"]):
        native.config_setting(
            name = name,
            values = {ck: type.serialize(value)},
            visibility = visibility,
        )

    return struct(
        section        = section,
        key            = key,
        doc            = doc or type.doc,
        type_name      = type.type_name,
        default_str    = default_str,
        read           = _read,
        config_setting = _config_setting,
    )

# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------

kcfg = struct(
    # Primitive type descriptors
    string  = _string,
    bool    = _bool,
    int     = _int,
    enum    = _enum,
    # Composite type descriptors (item is a primitive type descriptor)
    list    = _list,
    set     = _set,
    # Bind a descriptor to a buckconfig location
    declare = _declare,
)

def _kcfg_docs_impl(ctx):
    lines = [
        "# Configuration Reference",
        "",
        "All values are configured on the command line with `--config SECTION.NAME=VALUE`",
        "",
        "| Key | Type | Default | Description |",
        "| --- | ---- | ------- | ----------- |",
    ]

    entries = [json.decode(raw) for raw in ctx.attrs.entries]

    for e in entries:
        default_value = "`{}`".format(e["default"]) if len(e.get("default")) > 0 else "-"
        doc_str     = e.get("doc", "")

        lines.append("| `{}.{}` | `{}` |  {} | {} |".format(e["section"], e["key"], e["type_name"], default_value, doc_str))

    lines += ["", "## Details", ""]

    for e in entries:
        lines.append("### `{}.{}`".format(e["section"], e["key"]))
        lines.append("")
        if e.get("doc"):
            lines.append(e["doc"])
            lines.append("")

        lines.append("- **Type:** `{}`".format(e["type_name"]))
        lines.append("- **Default:** `{}`".format(e["default"] if e.get("default") != None else "-"))
        lines.append("")
        lines.append("Configure with `--config {}.{}=VALUE`".format(e["section"], e["key"]))

    out = ctx.actions.declare_output(ctx.attrs.out)
    ctx.actions.write(out, "\n".join(lines))
    return [DefaultInfo(default_output = out)]

_kcfg_docs = rule(
    impl = _kcfg_docs_impl,
    attrs = {
        "entries": attrs.list(attrs.any()),
        "out":     attrs.string(default = "config.md"),
    },
)

def kcfg_docs(name, entries, out = "config.md", **kwargs):
    """Declare a target that writes a Markdown configuration reference.

    entries – list of kcfg.declare() return values
    out     – output filename (default: config.md)
    """
    serialized = []
    for decl in entries:
        e = {"section": decl.section, "key": decl.key, "type_name": decl.type_name}
        if decl.doc:
            e["doc"] = decl.doc
        if decl.default_str != None:
            e["default"] = decl.default_str
        serialized.append(json.encode(e))
    _kcfg_docs(name = name, entries = serialized, out = out, **kwargs)
