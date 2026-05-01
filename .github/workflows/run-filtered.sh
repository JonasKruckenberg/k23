#!/usr/bin/env bash
# Dispatcher for changed-targets-filtered CI jobs.
#
# Usage: run-filtered.sh <recipe> <targets_result> <targets_mode> [extra_just_args...]
#   recipe:           name of the just recipe to run (check, clippy, unittests, miri, loom, fuzz)
#   targets_result:   GitHub Actions `needs.changed-targets.result` (success | skipped | ...)
#   targets_mode:     when targets_result=success, the `mode` output: filter | all | noop
#   extra_just_args:  prepended to the `just` invocation as variable
#                     assignments, e.g. `fuzz_args=-max_total_time=60`
set -euo pipefail

recipe="$1"
targets_result="${2:-skipped}"
targets_mode="${3:-}"
shift $(( $# < 3 ? $# : 3 ))

# Push to main / changed-targets skipped → run the full suite.
if [ "$targets_result" = "skipped" ]; then
    exec nix develop . --command just "$@" "$recipe"
fi

case "$targets_mode" in
    all)
        echo "changed-targets: pessimistic fallback — running full suite"
        exec nix develop . --command just "$@" "$recipe"
        ;;
    noop)
        echo "changed-targets: no impacted targets — skipping $recipe"
        exit 0
        ;;
    filter)
        if [ ! -s impacted-targets.txt ]; then
            echo "changed-targets: mode=filter but impacted-targets.txt is empty — skipping $recipe"
            exit 0
        fi
        impacted=$(tr '\n' ' ' < impacted-targets.txt)
        echo "changed-targets: filtered run — targets: $impacted"
        exec nix develop . --command just "$@" "$recipe" "$impacted"
        ;;
    *)
        echo "::error::Unknown targets_mode='$targets_mode' (targets_result=$targets_result)" >&2
        exit 1
        ;;
esac
