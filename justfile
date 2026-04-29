set unstable

platform := ""
_platform_args := if platform != "" { f"--target-platforms {{platform}}" } else { "" }

_buck2 := require("buck2")
_typos := require("typos")
_supertd := require("supertd")

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
preflight targets="" *buck2_args: (lint targets buck2_args) (unittests targets buck2_args) (miri targets buck2_args) # (loom targets buck2_args)

# run linters on a crate or the entire workspace.
lint targets="" *buck2_args: (clippy targets buck2_args) (check-fmt targets buck2_args) (typos)

# ===== linting =====

# run clippy on a crate or the entire workspace.
@clippy targets="" *buck2_args:
    {{ _buck2 }} build {{append("[clippy.txt]", _uquery(_q_buildables(_targets_query(targets))))}} {{_platform_args}} {{buck2_args}}

# check the workspace for typos
@typos:
    {{ _typos }}

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
fuzz targets="" *buck2_args:
    {{ _buck2 }} test {{_uquery(_q_fuzz_tests(_targets_query(targets)))}} {{_platform_args}} {{if fuzz_args == "" { "" } else { "-- " + fuzz_args }}}

# ===== formatting =====

# check formatting for a crate or the entire workspace.
@check-fmt targets="" *buck2_args:
    {{ _buck2 }} run 'toolchains//:rust_toolchain[rustfmt]' -- --edition 2024 --check {{ _uquery(_q_inputs(_q_buildables(_targets_query(targets)))) }} {{buck2_args}}

# format a crate or the entire workspace.
@format targets="" *buck2_args:
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
_q_fuzz_tests(q) := f"kind(rust_fuzz, ({{q}}))"
_q_benchmarks(q) := f"kind(_rust_benchmark_runner, {{q}})"
_q_inputs(q)     := f"inputs({{q}})"

# Resolve a query expression into a space-separated list of targets.
_uquery(q) := _replace_newlines(shell('buck2 uquery "$1"', q))

# Turn buck2's newline-delimited output into space-delimited.
_replace_newlines(str) := replace_regex(str, "(\r\n|\r|\n)", " ")
