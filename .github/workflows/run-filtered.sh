#!/usr/bin/env bash
# Dispatcher for BTD-filtered CI jobs.
#
# Usage: run-filtered.sh <recipe> <btd_result> <btd_mode>
#   recipe:     name of the just recipe to run (check, clippy, unittests, miri, loom)
#   btd_result: GitHub Actions `needs.btd.result` (success | skipped | ...)
#   btd_mode:   when btd_result=success, the `mode` output: filter | all | noop
set -euo pipefail

recipe="$1"
btd_result="${2:-skipped}"
btd_mode="${3:-}"

# Push to main / btd skipped → run the full suite.
if [ "$btd_result" = "skipped" ]; then
    exec nix develop . --command just "$recipe"
fi

case "$btd_mode" in
    all)
        echo "BTD: pessimistic fallback — running full suite"
        exec nix develop . --command just "$recipe"
        ;;
    noop)
        echo "BTD: no impacted targets — skipping $recipe"
        exit 0
        ;;
    filter)
        if [ ! -s impacted-targets.txt ]; then
            echo "BTD: mode=filter but impacted-targets.txt is empty — skipping $recipe"
            exit 0
        fi
        impacted=$(tr '\n' ' ' < impacted-targets.txt)
        echo "BTD: filtered run — targets: $impacted"
        exec nix develop . --command just "$recipe" "$impacted"
        ;;
    *)
        echo "::error::Unknown btd_mode='$btd_mode' (btd_result=$btd_result)" >&2
        exit 1
        ;;
esac
