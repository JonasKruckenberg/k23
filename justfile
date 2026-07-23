set unstable

platform := ""
# --skip-incompatible-targets drops riscv-only and host-only targets that
# don't match the active platform instead of erroring out.
[private]
_platform_args := "--skip-incompatible-targets" + if platform != "" { f" --target-platforms {{platform}}" } else { "" }

[private]
_buck2 := require("buck2")
[private]
_typos := require("typos")
[private]
_reindeer := require("reindeer")
[private]
_rust_project := require("rust-project")
[private]
_cargo_deny := require("cargo-deny")
[private]
_jq := require("jq")

[private]
_docstring := "
justfile for k23
see https://just.systems/man/en/
"

# default recipe to display help information
_default:
    @echo '{{ _docstring }}'
    @just --list

run target buck2_args="" *qemu_args="":
    {{ _buck2 }} run {{ target }} {{ _platform_args }} {{ buck2_args }} {{ qemu_args }}

build target *buck2_args="":
    {{ _buck2 }} build {{ target }} {{ _platform_args }} {{ buck2_args }}

# quick check for development.
# The prelude's [diag.json] action is infallible by design; gate on the
# rendered diagnostics ourselves.
check targets="//..." *buck2_args:
    #!/usr/bin/env bash
    set -euo pipefail
    out=$({{ _buck2 }} build {{ append("[diag.json]", _uquery(_q_buildables(_targets_query(targets)))) }} {{ _platform_args }} {{ buck2_args }} --show-simple-output | xargs {{ _jq }} -r 'select(.level=="error") | .rendered')
    [ -z "$out" ] || { printf '%s' "$out" >&2; exit 1; }

# Ordered cheapest-first so an obvious failure doesn't wait behind qemu.
#
# run every check CI runs; defaults to the targets your changes affect
preflight targets="" *buck2_args:
    #!/usr/bin/env bash
    set -euxo pipefail
    j="{{ just_executable() }}"
    f=$(mktemp); trap 'rm -f "$f"' EXIT
    t='{{ targets }}'; [ -n "$t" ] || { "$j" changed-targets > "$f"; t=$(cat "$f"); }
    "$j" typos; "$j" check-fmt "$t"; "$j" check-license-headers
    for a in riscv64 aarch64 x86_64; do "$j" platform=//platforms:$a clippy "$t" {{ buck2_args }}; done
    for r in unittests miri loom selftests; do "$j" $r "$t" {{ buck2_args }}; done
    "$j" buck2-audit; "$j" reindeer-clean; "$j" cargo-deny

# run linters on a crate or the entire workspace.
lint targets="//..." *buck2_args: (clippy targets buck2_args) (check-fmt targets buck2_args) typos

# ===== linting =====

# run clippy on a crate or the entire workspace.
# The prelude's [clippy.json] action is infallible by design; gate on the
# rendered diagnostics ourselves.
clippy targets="//..." *buck2_args:
    #!/usr/bin/env bash
    set -euo pipefail
    out=$({{ _buck2 }} build {{ append("[clippy.json]", _uquery(_q_buildables(_targets_query(targets)))) }} {{ _platform_args }} {{ buck2_args }} --show-simple-output | xargs {{ _jq }} -r 'select(.level=="error") | .rendered')
    [ -z "$out" ] || { printf '%s' "$out" >&2; exit 1; }

# check the workspace for typos
@typos:
    {{ _typos }}

# Generate rust-project.json so rust-analyzer can index the workspace.
# rust-analyzer auto-loads rust-project.json from the repo root.
# Re-run after adding/removing crates or changing BUCK deps.
rust-project arch="riscv64":
    {{ _rust_project }} develop --pretty --prefer-rustup-managed-toolchain '--rustc-target={{ _rustc_target(arch) }}' '--mode=--target-platforms=//platforms:{{ arch }}' 'root//sys/...' 'root//lib/...'

# ===== testing =====

# run unit tests for a crate or the entire workspace.
@unittests targets="//..." *buck2_args:
    {{ _buck2 }} test {{ _uquery(_q_unit_tests(_targets_query(targets))) }} {{ _platform_args }} {{ buck2_args }}

# run miri tests for a crate or the entire workspace.
@miri targets="//..." *buck2_args:
    {{ _buck2 }} test {{ append("[miri]", _uquery(_q_unit_tests(_targets_query(targets)))) }} {{ _platform_args }} {{ buck2_args }}

# run loom tests for a crate or the entire workspace.
@loom targets="//..." *buck2_args:
    {{ _buck2 }} test {{ _uquery(_q_loom_tests(_targets_query(targets))) }} {{ _platform_args }} {{ buck2_args }}

# Override `fuzz_args` to forward flags to each fuzz binary; pass complete
# `--test-arg=…` items (one per binary arg). Example:
#   just fuzz_args='--test-arg=-max_total_time=60' fuzz <targets>
fuzz_args := ""

# run fuzz tests for a crate or the entire workspace.
@fuzz targets="//..." *buck2_args:
    {{ _buck2 }} test {{ _uquery(_q_fuzz_tests(_targets_query(targets))) }} {{ _platform_args }} {{ buck2_args }} {{ if fuzz_args == "" { "" } else { "-- " + fuzz_args } }}

# run kernel selftests under qemu. Pinned to riscv64 for now; long-term this
# should loop over every supported arch.
@selftests targets="//..." *buck2_args:
    {{ _buck2 }} test {{ _uquery(_q_qemu_tests(_targets_query(targets))) }} --target-platforms //platforms:riscv64 {{ buck2_args }}

# ===== formatting =====

# rustfmt reads stdin when handed no files, hence the `$# -eq 0` bail-out.
# check formatting for a crate or the entire workspace.
@check-fmt targets="//..." *buck2_args:
    set -- {{ _uquery(_q_inputs(_q_buildables(_targets_query(targets)))) }}; [ $# -eq 0 ] || {{ _buck2 }} run 'toolchains//:rust_toolchain[rustfmt]' -- --edition 2024 --check "$@" {{ buck2_args }}

# format a crate or the entire workspace.
@fmt targets="//..." *buck2_args:
    set -- {{ _uquery(_q_inputs(_q_buildables(_targets_query(targets)))) }}; [ $# -eq 0 ] || {{ _buck2 }} run 'toolchains//:rust_toolchain[rustfmt]' -- --edition 2024 "$@" {{ buck2_args }}

# ===== documentation =====

# build the documentation for a crate or the entire workspace.
@doc targets="//..." *buck2_args:
    {{ _buck2 }} build {{ append("[doc]", _uquery(_q_buildables(_targets_query(targets)))) }} --show-output {{ _platform_args }} {{ buck2_args }}

manual:
    {{ _buck2 }} run //manual:manual

# ===== benchmarking =====

benchmark targets="//..." *buck2_args:
    #!/usr/bin/env bash
    set -euo pipefail
    for t in {{ _uquery(_q_benchmarks(_targets_query(targets))) }}; do
        {{ _buck2 }} run "$t" {{ _platform_args }} {{ buck2_args }}
    done

# ===== audit / freshness =====

# audit the buck2 graph: cell config plus visibility/providers for top-level kernel targets.
@buck2-audit:
    {{ _buck2 }} audit cell

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
    out=$({{ _buck2 }} uquery "kind(rust_library, //third-party/...) except deps({{ _default_query }})" {{ buck2_args }})
    [ -z "$out" ] || { echo "$out" >&2; exit 1; }

# fail if any first-party Rust source file lacks the canonical license header.
# Exclusions (vendored crates, build/VCS dirs) live in //build/license-header-linter.
@check-license-headers *buck2_args:
    {{ _buck2 }} run //build/license-header-linter:license-header-linter {{ buck2_args }} -- {{ justfile_directory() }}

# prepend the canonical license header to any first-party file missing it.
@fix-license-headers *buck2_args:
    {{ _buck2 }} run //build/license-header-linter:license-header-linter {{ buck2_args }} -- --fix {{ justfile_directory() }}

# ===== changed targets =====

# Files that define the graph beyond any single package: a `PACKAGE` applies to
# a whole subtree, a `.bzl` is loaded by packages it never names, `.buckconfig`
# defines the cells. The two BUCK files are cell roots (`toolchains//` and
# `constraints//`), so their packages aren't at the paths their paths suggest.
[private]
_graph_files := '(^|/)PACKAGE$|\.bzl$|^\.buckconfig$|^build/(toolchains|constraints)/BUCK$'

# Only the file list needs a base revision; the impact of those files is read off
# the current graph. git, not jj, so both work the same — `ls-files --others`
# is what catches new files jj auto-tracks but git has never seen.
#
# print the targets affected since <base>; `//...` means all, empty means none
changed-targets base="main":
    #!/usr/bin/env bash
    set -euo pipefail
    merge_base=$(git merge-base {{ base }} HEAD)
    # A deleted file has no owner, so no query can map it back to a target.
    deleted=$(git diff --name-only --diff-filter=D "$merge_base")
    files=$(git diff --name-only --diff-filter=d "$merge_base"; git ls-files --others --exclude-standard)
    # A BUCK file has no owner either, but it does have a scope: its own package.
    pkgs=$(grep -E '(^|/)BUCK$' <<<"$files" | sed 's|BUCK$||; s|/$||; s|^|//|; s|$|:|' | tr '\n' ' ' || true)
    # Union the owners into one query rather than using buck2's `%s` substitution:
    # `%s` re-runs the whole rdeps traversal once per file, which costs minutes on a
    # large change and silently yields nothing at all past ~200 of them.
    seeds=$(sed -n "s|..*|owner('&')|p" <<<"$files" | paste -sd+ -)
    if [ -n "$deleted" ] || grep -qE '{{ _graph_files }}' <<<"$files"; then
        echo '//...'
    elif [ -n "$seeds" ]; then
        {{ _buck2 }} uquery "{{ _default_query }} intersect rdeps({{ _default_query }}, $seeds${pkgs:+ + set($pkgs)})" | tr '\n' ' '
    fi

_rustc_target(arch) := if arch == "riscv64" { "riscv64gc-unknown-none-elf" } else if arch == "aarch64" { "aarch64-unknown-none" } else { "x86_64-unknown-none" }

# ===== query helpers =====
#
# Recipes accept `targets` as a space-separated list of buck2 target patterns and
# default to `//...`, the whole workspace. An *empty* list therefore unambiguously
# means no targets, and buck2 exits 0 having done nothing — which is what lets
# `changed-targets` return nothing without any caller special-casing it.
#
# Helpers compose buck2 query expressions as strings, and `_uquery` resolves the
# final expression in a single `buck2 uquery` call — one shell-out per recipe
# regardless of how many filters are stacked.

# Default workspace target set: rust binaries, libraries, and benchmark runners (no third-party).
# _default_query := "(kind(rust_binary, '//...') + kind(rust_library, '//...') + kind(_rust_benchmark_runner, '//...')) except '//third-party/...'"
[private]
_default_query := "'//...' except '//third-party/...'"

# Build a query expression from the recipe's `targets` argument.
# Third-party is filtered out: those targets only build under the host exec
# config and break under non-host --target-platforms; they still get built
# transitively as deps of first-party crates.
_targets_query(targets) := f"(set({{targets}})) except '//third-party/...'"

# Refinements: each takes a query expression and returns a more specific one.
# Proc-macros are routed via their `rust_proc_macro_alias`, which exec-configures
# the underlying `rust_library`. Building the underlying directly would fail
# under non-host --target-platforms.
_q_buildables(q) := f"kind(rust_binary, {{q}}) + (kind(rust_library, {{q}}) except attrfilter(proc_macro, True, {{q}})) + kind(rust_proc_macro_alias, {{q}})"
_q_tests(q) := f"kind(rust_test, {{q}}) + kind(rust_test, testsof({{q}}))"
_q_unit_tests(q) := f"nattrfilter(labels, loom, ({{_q_tests(q)}}))"
_q_loom_tests(q) := f"attrfilter(labels, loom, ({{_q_tests(q)}}))"
_q_fuzz_tests(q) := f"kind(rust_fuzz, {{q}}) + kind(rust_fuzz, testsof({{q}}))"
_q_qemu_tests(q) := f"kind(qemu_test, {{q}})"
_q_benchmarks(q) := f"kind(_rust_benchmark_runner, {{q}}) + kind(_rust_benchmark_runner, testsof({{q}}))"
_q_inputs(q) := f"inputs({{q}})"

# Resolve a query expression into a space-separated list of targets.
_uquery(q) := _replace_newlines(shell('buck2 uquery "$1"', q))

# Turn buck2's newline-delimited output into space-delimited.
_replace_newlines(str) := replace_regex(str, "(\r\n|\r|\n)", " ")
