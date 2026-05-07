set unstable

platform := ""
_platform_args := if platform != "" { f"--target-platforms {{platform}}" } else { "" }

_buck2 := require("buck2")
_typos := require("typos")
_supertd := require("supertd")
_reindeer := require("reindeer")
_rust_project := require("rust-project")
_cargo_deny := require("cargo-deny")

_docstring := "
justfile for k23
see https://just.systems/man/en/
"

# default recipe to display help information
_default:
    @echo '{{ _docstring }}'
    @just --list

run target buck2_args="" *qemu_args="":
    {{ _buck2 }} run {{target}} {{buck2_args}} {{qemu_args}}

# quick check for development
@check targets="" *buck2_args:
    {{ _buck2 }} build {{append("[check]", _uquery(_q_buildables(_targets_query(targets))))}} {{_platform_args}} {{buck2_args}}

# run all lints and tests on a crate or the entire workspace.
preflight targets="" *buck2_args: (lint targets buck2_args) (unittests targets buck2_args) (miri targets buck2_args) (loom targets buck2_args) (selftests buck2_args) buck2-audit cargo-deny reindeer-clean check-license-headers

# run linters on a crate or the entire workspace.
lint targets="" *buck2_args: (clippy targets buck2_args) (check-fmt targets buck2_args) (typos)

# ===== linting =====

# run clippy on a crate or the entire workspace.
@clippy targets="" *buck2_args:
    {{ _buck2 }} build {{append("[clippy.txt]", _uquery(_q_buildables(_targets_query(targets))))}} {{_platform_args}} {{buck2_args}}

# check the workspace for typos
@typos:
    {{ _typos }}

# regenerate third-party/BUCK from third-party/Cargo.toml via reindeer
@buckify:
    {{ _reindeer }} buckify

# Generate rust-project.json so rust-analyzer can index the workspace.
# rust-analyzer auto-loads rust-project.json from the repo root.
# Re-run after adding/removing crates or changing BUCK deps.
@rust-project:
    {{ _rust_project }} develop --pretty --prefer-rustup-managed-toolchain 'root//sys/...' 'root//lib/...'

# ===== testing =====

# run unit tests for a crate or the entire workspace.
@unittests targets="" *buck2_args:
    {{ _buck2 }} test {{_uquery(_q_unit_tests(_targets_query(targets)))}} {{_platform_args}} {{buck2_args}}

# run miri tests for a crate or the entire workspace.
@miri targets="" *buck2_args:
    {{ _buck2 }} test {{append("[miri]", _uquery(_q_unit_tests(_targets_query(targets))))}} {{_platform_args}} {{buck2_args}}

# run loom tests for a crate or the entire workspace.
@loom targets="" *buck2_args:
    {{ _buck2 }} test {{_uquery(_q_loom_tests(_targets_query(targets)))}} {{_platform_args}} {{buck2_args}}

# Override `fuzz_args` to forward flags to each fuzz binary; pass complete
# `--test-arg=…` items (one per binary arg). Example:
#   just fuzz_args='--test-arg=-max_total_time=60' fuzz <targets>
fuzz_args := ""

# run fuzz tests for a crate or the entire workspace.
@fuzz targets="" *buck2_args:
    {{ _buck2 }} test {{_uquery(_q_fuzz_tests(_targets_query(targets)))}} {{_platform_args}} {{buck2_args}} {{if fuzz_args == "" { "" } else { "-- " + fuzz_args }}}

# run kernel selftests under qemu.
# Pass kernel bootargs args after `--`, e.g.
#   just selftests -- --format=json
@selftests *buck2_args:
    {{ _buck2 }} test //sys:k23-qemu-riscv64-tests {{buck2_args}}

# ===== formatting =====

# check formatting for a crate or the entire workspace.
@check-fmt targets="" *buck2_args:
    {{ _buck2 }} run 'toolchains//:rust_toolchain[rustfmt]' -- --edition 2024 --check {{ _uquery(_q_inputs(_q_buildables(_targets_query(targets)))) }} {{buck2_args}}

# format a crate or the entire workspace.
@fmt targets="" *buck2_args:
    {{ _buck2 }} run 'toolchains//:rust_toolchain[rustfmt]' -- --edition 2024 {{ _uquery(_q_inputs(_q_buildables(_targets_query(targets)))) }} {{buck2_args}}

# ===== documentation =====

# build the documentation for a crate or the entire workspace.
@doc targets="" *buck2_args:
    {{ _buck2 }} build {{append("[doc]", _uquery(_q_buildables(_targets_query(targets))))}} --show-output {{_platform_args}} {{buck2_args}}

manual:
    {{ _buck2 }} run //manual:manual

# ===== benchmarking =====

benchmark targets="" *buck2_args:
    #!/usr/bin/env bash
    set -euo pipefail
    for t in {{_uquery(_q_benchmarks(_targets_query(targets)))}}; do
        {{ _buck2 }} run "$t" {{_platform_args}} {{buck2_args}}
    done

# ===== audit / freshness =====

# audit the buck2 graph: cell config plus visibility/providers for top-level kernel targets.
@buck2-audit:
    {{ _buck2 }} audit cell
    {{ _buck2 }} audit visibility //sys:k23-riscv64 //sys:k23-qemu-riscv64 //sys/kernel:kernel //sys/loader:loader
    {{ _buck2 }} audit providers //sys:k23-riscv64 //sys:k23-qemu-riscv64 //sys/kernel:kernel //sys/loader:loader

# run cargo-deny against the third-party Cargo workspace.
@cargo-deny:
    {{ _cargo_deny }} --manifest-path third-party/Cargo.toml check

# Fail if third-party/BUCK is out of sync with third-party/Cargo.toml.
@reindeer-clean:
    {{ _reindeer }} buckify --stdout | diff -u third-party/BUCK -

# fail if any third-party rust_library has no first-party (transitive) consumer.
@unused-third-party *buck2_args:
    #!/usr/bin/env bash
    set -euo pipefail
    out=$({{ _buck2 }} uquery "kind(rust_library, //third-party/...) except deps({{_default_query}})" {{buck2_args}})
    [ -z "$out" ] || { echo "$out" >&2; exit 1; }

# Space-separated buck target patterns whose source files are exempt from
# `check-license-headers` (e.g. vendored crates outside //third-party/...).
license_header_excluded := "//lib/range-tree: //lib/sharded-slab: //lib/wast:"
_license_header_excl := if license_header_excluded == "" { "" } else { f" except inputs(set({{license_header_excluded}}))" }

# fail if any first-party Rust source file lacks the canonical license header
# (build/license-header.txt) byte-for-byte at the start of the file.
@check-license-headers *buck2_args:
    #!/usr/bin/env bash
    set -euo pipefail
    n=$(wc -c < build/license-header.txt | tr -d ' ')
    files=$({{ _buck2 }} uquery "filter('\\.rs$', inputs({{_default_query}})){{_license_header_excl}}" {{buck2_args}})
    bad=$(for f in $files; do cmp -s <(head -c "$n" "$f") build/license-header.txt || echo "$f"; done)
    [ -z "$bad" ] || { echo "::error::license header missing or mismatched:" >&2; echo "$bad" >&2; exit 1; }

# ===== changed-targets (powered by buck2-change-detector) =====
#
# Files that, if changed, are too coarse for the detector to reason about and
# force a full-suite run instead. Most of these either drive third-party
# buckification (Cargo.toml/Cargo.lock + reindeer) or change toolchains/Buck
# cells globally.
_pessimistic_paths := "Cargo.toml Cargo.lock third-party/Cargo.toml third-party/Cargo.lock flake.nix flake.lock rust-toolchain.toml .buckconfig PACKAGE"

# emit the jj summary between BASE and the working copy (raw, for --changes input)
changed-targets-diff BASE:
    jj diff --summary --from {{BASE}} --to @

# print impacted Rust targets, or `__ALL__` if a pessimistic file changed
changed-targets CHANGES BASE_JSONL UNIVERSE='root//...':
    #!/usr/bin/env bash
    set -euo pipefail
    for p in {{_pessimistic_paths}}; do
        if awk -v p="$p" '$2 == p { found=1; exit } END { exit !found }' {{CHANGES}}; then
            echo __ALL__
            exit 0
        fi
    done
    {{ _supertd }} audit cell > cells.json
    {{ _supertd }} audit config > config.json
    {{ _supertd }} btd \
        --changes {{CHANGES}} \
        --base {{BASE_JSONL}} \
        --cells cells.json \
        --config config.json \
        --universe {{UNIVERSE}} \
      | awk '/^[[:space:]]+root\/\// { sub(/^[[:space:]]+/, ""); print }' \
      | sort -u \
      | tr '\n' ' '
    echo

# ===== query helpers =====
#
# Recipes accept `targets` as a space-separated list of buck2 target patterns;
# empty (the default) means the entire workspace. Helpers compose buck2 query
# expressions as strings, and `_uquery` resolves the final expression in a
# single `buck2 uquery` call — one shell-out per recipe regardless of how many
# filters are stacked.

# Default workspace target set: rust binaries, libraries, and benchmark runners (no third-party).
# _default_query := "(kind(rust_binary, '//...') + kind(rust_library, '//...') + kind(_rust_benchmark_runner, '//...')) except '//third-party/...'"
_default_query := "'//...' except '//third-party/...'"

# Build a query expression from the recipe's `targets` argument.
_targets_query(targets) := if targets == "" { _default_query } else { f"set({{targets}})" }

# Refinements: each takes a query expression and returns a more specific one.
_q_buildables(q) := f"kind(rust_binary, {{q}}) + kind(rust_library, {{q}})"
_q_tests(q)      := f"kind(rust_test, {{q}}) + kind(rust_test, testsof({{q}}))"
_q_unit_tests(q) := f"nattrfilter(labels, loom, ({{_q_tests(q)}}))"
_q_loom_tests(q) := f"attrfilter(labels, loom, ({{_q_tests(q)}}))"
_q_fuzz_tests(q) := f"kind(rust_fuzz, {{q}}) + kind(rust_fuzz, testsof({{q}}))"
_q_benchmarks(q) := f"kind(_rust_benchmark_runner, {{q}}) + kind(_rust_benchmark_runner, testsof({{q}}))"
_q_inputs(q)     := f"inputs({{q}})"

# Resolve a query expression into a space-separated list of targets.
_uquery(q) := _replace_newlines(shell('buck2 uquery "$1"', q))

# Turn buck2's newline-delimited output into space-delimited.
_replace_newlines(str) := replace_regex(str, "(\r\n|\r|\n)", " ")
