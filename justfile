set unstable

platform := ""
_platform_args := if platform != "" { "--target-platforms " + platform } else { "" }

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
@check targets=_all_rust_targets() *buck2_args:
    {{ _buck2 }} build {{append("[check]", targets)}} {{_platform_args}} {{buck2_args}}

# run all lints and tests on a crate or the entire workspace.
preflight targets=_all_rust_targets() *buck2_args: (lint targets buck2_args) (unittests targets buck2_args) (miri targets buck2_args) # (loom targets buck2_args)

# run linters on a crate or the entire workspace.
lint targets=_all_rust_targets() *buck2_args: (clippy targets buck2_args) (check-fmt targets buck2_args) (typos)

# ===== linting =====

# run clippy on a crate or the entire workspace.
@clippy targets=_all_rust_targets() *buck2_args:
    {{ _buck2 }} build {{append("[clippy.txt]", targets)}} {{_platform_args}} {{buck2_args}}

# check the workspace for typos
@typos:
    {{ _typos }}

# ===== testing =====

# run unit tests for a crate or the entire workspace.
@unittests targets=_all_rust_targets() *buck2_args:
    {{ _buck2 }} test {{_unit_tests(targets)}} {{_platform_args}} {{buck2_args}}

# run miri tests for a crate or the entire workspace.
@miri targets=_all_rust_targets() *buck2_args:
    {{ _buck2 }} test {{append("[miri]", _unit_tests(targets))}} {{_platform_args}} {{buck2_args}}

# run loom tests for a crate or the entire workspace.
@loom targets=_all_rust_targets() *buck2_args:
    {{ _buck2 }} test {{_loom_tests(targets)}} {{_platform_args}} {{buck2_args}}

# ===== formatting =====

# check formatting for a crate or the entire workspace.
@check-fmt targets=_all_rust_targets() *buck2_args:
    {{ _buck2 }} run 'toolchains//:rust_toolchain[rustfmt]' -- --edition 2024 --check {{ _source_files(targets) }} {{buck2_args}}

# format a crate or the entire workspace.
@format targets=_all_rust_targets() *buck2_args:
    {{ _buck2 }} run 'toolchains//:rust_toolchain[rustfmt]' -- --edition 2024 {{ _source_files(targets) }} {{buck2_args}}

# ===== documentation =====

# build the documentation for a crate or the entire workspace.
@doc targets=_all_rust_targets() *buck2_args:
    {{ _buck2 }} build {{append("[doc]", targets)}} --show-output {{_platform_args}} {{buck2_args}}

manual:
    {{ _buck2 }} run //manual:manual

# ===== benchmarking =====

benchmark targets=_all_rust_targets() *buck2_args:
    # echo "{{_micro_benchmarks(targets)}}"
    {{ _buck2 }} run {{_micro_benchmarks(targets)}} {{_platform_args}} {{buck2_args}}

# ===== target set construction helpers =====

_rust_targets_query := "kind(rust_binary, '//...') + kind(rust_library, '//...') except '//third-party/...'"
_all_rust_targets() := _replace_newlines(shell('buck2 uquery "$1"', _rust_targets_query))

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

# turns the buck2 default newline-delimited output into space-delimited
_replace_newlines(str) := replace_regex(str, "(\r\n|\r|\n)", " ")

# obtain all the rust tests from an input set of targets
_rust_tests(targets) := _replace_newlines(shell('buck2 uquery "kind(rust_test, set($1)) + kind(rust_test, testsof(set($1)))"', targets))

# filter an input set of targets down to only the regular rust unit tests
_unit_tests(targets) := _replace_newlines(shell('buck2 uquery "nattrfilter(labels, loom, (set($1)))"', _rust_tests(targets)))

# filter an input set of targets down to only the loom tests
_loom_tests(targets) := _replace_newlines(shell('buck2 uquery "attrfilter(labels, loom, (set($1)))"', _rust_tests(targets)))

# filter an input set of targets down to only the micro-benchmarks
_micro_benchmarks(targets) := _replace_newlines(shell('buck2 uquery "kind(rust_benchmark, set($1)) + kind(rust_benchmark, testsof(set($1)))"', targets))

_source_files(targets) := _replace_newlines(shell('buck2 uquery "inputs(set($1))"', targets))
