#!/usr/bin/env bash
#
# Validate the buck2 content-based-path fix for cranelift's ISLE_DIR.
#
# Background
# ----------
# cranelift-codegen's build script writes the ISLE-generated Rust into OUT_DIR
# and emits `cargo:rustc-env=ISLE_DIR=<OUT_DIR>`. OUT_DIR is a content-addressed
# buck2 output, so it lives at two kinds of path:
#
#   .../__cranelift-codegen-0.117-build-script-run__/output_artifacts/OUT_DIR  (working dir)
#   .../__cranelift-codegen-0.117-build-script-run__/<content-hash>/OUT_DIR     (canonical)
#
# The consuming compile resolves OUT_DIR (via `$(location [out_dir])`) to the
# *canonical* hashed path and tracks it as a dependency, so it is always
# materialized. The unpatched buildscript_run.py instead baked ISLE_DIR as the
# *working* path (`$(abspath .../output_artifacts/OUT_DIR)`), which is neither
# the canonical path nor a tracked dependency. Under `materializations =
# deferred`, when the build-script-run is served from cache and materialized
# only at its hashed path, `output_artifacts/OUT_DIR` is absent and rustc fails:
#
#   error: couldn't read .../output_artifacts/OUT_DIR/isle_riscv64.rs: No such file or directory
#
# The fix (prelude/rust/tools/{buildscript_run.py,rustc_action.py}) re-anchors
# OUT_DIR-internal paths to a ${OUT_DIR} sentinel that resolves to the canonical,
# tracked OUT_DIR the consumer already receives via --path-env.
#
# What this script proves
# -----------------------
#  A. STATIC  — which buck2 is active: does the generated rustc_flags use the
#               ${OUT_DIR} sentinel (fixed) or a baked output_artifacts path
#               (unpatched)?
#  B. DYNAMIC — the actual failure trigger, made deterministic: with the
#               build-script-run cached and its `output_artifacts/` working dir
#               deleted, does a *fresh* library compile still succeed?
#                 fixed     -> ISLE_DIR -> canonical hashed OUT_DIR (present) -> PASS
#                 unpatched -> ISLE_DIR -> output_artifacts (deleted)   -> reproduces bug
#
# Usage
# -----
#   build/validate-isle-content-path.sh
#
# Env overrides:
#   BUCK2            buck2 binary to test           (default: buck2 on PATH)
#   PLATFORM         target platform                (default: //platforms:riscv64)
#   LIB              cranelift library target       (default: root//third-party:cranelift-codegen-0.117)
#   BSR              build-script-run target        (default: <LIB>-build-script-run)
#   REBUILD_TARGET   target whose fresh compile reads ISLE_DIR
#                    (default: $LIB; set to //sys/kernel:kernel if the bare
#                     library target won't configure standalone)
#
set -uo pipefail

BUCK2="${BUCK2:-buck2}"
PLATFORM="${PLATFORM:-//platforms:riscv64}"
LIB="${LIB:-root//third-party:cranelift-codegen-0.117}"
BSR="${BSR:-${LIB}-build-script-run}"
REBUILD_TARGET="${REBUILD_TARGET:-$LIB}"

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_ROOT"

BSR_BASE="buck-out/v2/art/root/third-party/__cranelift-codegen-0.117-build-script-run__"
ARTIFACTS="$BSR_BASE/output_artifacts"

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
red() { printf '\033[31m%s\033[0m\n' "$*"; }
yellow() { printf '\033[33m%s\033[0m\n' "$*"; }
hr() { printf '%s\n' "----------------------------------------------------------------------"; }

fail() { red "FAIL: $*"; exit 1; }

bold "buck2:           $($BUCK2 --version 2>/dev/null || echo '??? (not found)')"
bold "platform:        $PLATFORM"
bold "library target:  $LIB"
bold "build-script:    $BSR"
bold "rebuild target:  $REBUILD_TARGET"
hr

# ---------------------------------------------------------------------------
# Phase 0 — clean slate so "build-script-run cached, library fresh" is exact.
# ---------------------------------------------------------------------------
bold "[0] buck2 clean (full reset)"
$BUCK2 clean >/dev/null 2>&1 || true

# ---------------------------------------------------------------------------
# Phase 1 — build ONLY the build-script-run's out_dir.
#           This runs the build script (creating output_artifacts/OUT_DIR and
#           promoting to the canonical hashed path) but does NOT compile the
#           library, so the library compile in phase 3 is guaranteed fresh.
# ---------------------------------------------------------------------------
bold "[1] build only ${BSR}[out_dir]"
if ! $BUCK2 build "${BSR}[out_dir]" --target-platforms "$PLATFORM" >/dev/null 2>&1; then
    fail "could not build ${BSR}[out_dir] under $PLATFORM.
      Check the BSR target name / platform, or build it as part of the kernel."
fi

[ -d "$BSR_BASE" ] || fail "build-script-run artifact dir not found: $BSR_BASE"

# Canonical (content-hashed) OUT_DIR copies — anything that is NOT output_artifacts.
mapfile -t HASHED_OUT_DIRS < <(find "$BSR_BASE" -mindepth 2 -maxdepth 2 -type d -name OUT_DIR \
                                 -not -path "*/output_artifacts/*" 2>/dev/null)
if [ "${#HASHED_OUT_DIRS[@]}" -eq 0 ]; then
    fail "no content-hashed OUT_DIR found under $BSR_BASE (only output_artifacts?)."
fi
green "    canonical OUT_DIR(s): ${#HASHED_OUT_DIRS[@]}"
for d in "${HASHED_OUT_DIRS[@]}"; do
    [ -f "$d/isle_riscv64.rs" ] || fail "canonical $d is missing isle_riscv64.rs"
    printf '      %s  (isle_riscv64.rs present)\n' "$d"
done

# ---------------------------------------------------------------------------
# Phase A (static) — inspect the generated rustc_flags: ${OUT_DIR} vs baked path.
# ---------------------------------------------------------------------------
hr
bold "[A] STATIC: how is ISLE_DIR encoded in the generated rustc_flags?"
mapfile -t FLAG_FILES < <(find "$BSR_BASE" -name rustc_flags -type f 2>/dev/null)
FIXED=0 UNPATCHED=0
for f in "${FLAG_FILES[@]}"; do
    line="$(grep -E 'ISLE_DIR' "$f" 2>/dev/null || true)"
    [ -n "$line" ] || continue
    if printf '%s' "$line" | grep -q '\${OUT_DIR}'; then
        FIXED=1; printf '    [fixed]     %s\n      %s\n' "$f" "$line"
    elif printf '%s' "$line" | grep -q 'output_artifacts'; then
        UNPATCHED=1; printf '    [unpatched] %s\n      %s\n' "$f" "$line"
    else
        printf '    [other]     %s\n      %s\n' "$f" "$line"
    fi
done
if [ "$FIXED" = 1 ] && [ "$UNPATCHED" = 0 ]; then
    green "    => active buck2 uses the \${OUT_DIR} sentinel (PATCHED)."
elif [ "$UNPATCHED" = 1 ]; then
    yellow "    => active buck2 still bakes output_artifacts (UNPATCHED). Expect the"
    yellow "       dynamic phase below to REPRODUCE the bug."
else
    yellow "    => could not classify rustc_flags; continuing to dynamic phase."
fi

# ---------------------------------------------------------------------------
# Phase 2 — delete the untracked working dir. The canonical hashed copies stay.
# ---------------------------------------------------------------------------
hr
bold "[2] delete the untracked working dir: $ARTIFACTS"
if [ -d "$ARTIFACTS" ]; then
    rm -rf "$ARTIFACTS"
    green "    removed."
else
    yellow "    output_artifacts already absent (fine — that is the adverse state)."
fi
# Sanity: canonical copies must survive.
for d in "${HASHED_OUT_DIRS[@]}"; do
    [ -f "$d/isle_riscv64.rs" ] || fail "canonical OUT_DIR vanished with output_artifacts: $d"
done
green "    canonical OUT_DIR copies survived (as expected — they are tracked)."

# ---------------------------------------------------------------------------
# Phase B (dynamic) — fresh library compile with output_artifacts absent.
# ---------------------------------------------------------------------------
hr
bold "[B] DYNAMIC: fresh compile of $REBUILD_TARGET (build-script-run is cached)"
BUILD_LOG="$(mktemp)"
$BUCK2 build "$REBUILD_TARGET" --target-platforms "$PLATFORM" >"$BUILD_LOG" 2>&1
RC=$?

# Guard: if the build-script-run re-ran, output_artifacts reappears and the test
# is inconclusive (config of phase-1 BSR didn't match the one this target uses).
RERAN=0
[ -d "$ARTIFACTS" ] && RERAN=1

echo
if [ "$RC" -eq 0 ]; then
    if [ "$RERAN" = 1 ]; then
        yellow "INCONCLUSIVE: build succeeded but output_artifacts was recreated, meaning"
        yellow "the build-script-run RE-RAN (its config differs from phase 1). The adverse"
        yellow "condition was not actually exercised. Try REBUILD_TARGET=//sys/kernel:kernel"
        yellow "so phase 1 and phase B configure the build-script-run identically."
        exit 2
    fi
    green   "PASS: library compiled with output_artifacts absent."
    green   "      ISLE_DIR resolved to the canonical, materialized OUT_DIR — the fix works."
    rm -f "$BUILD_LOG"
    exit 0
else
    if grep -q "isle_riscv64.rs.*No such file\|couldn't read.*OUT_DIR/isle" "$BUILD_LOG"; then
        red "BUG REPRODUCED: fresh compile failed reading isle_riscv64.rs from the"
        red "                deleted output_artifacts working dir:"
        echo
        grep -E "couldn't read|isle_riscv64|output_artifacts" "$BUILD_LOG" | sed 's/^/    /' | head -8
        echo
        if [ "$FIXED" = 1 ]; then
            red "                rustc_flags used \${OUT_DIR} yet it still failed — the"
            red "                rustc_action.py half of the fix is not taking effect."
        else
            yellow "                The active buck2 is UNPATCHED — rebuild/install buck2 from"
            yellow "                the fork (the prelude is bundled into the binary)."
        fi
        echo "    full log: $BUILD_LOG"
        exit 1
    fi
    red "Build failed for a different reason (not the ISLE_DIR bug):"
    tail -30 "$BUILD_LOG" | sed 's/^/    /'
    echo "    full log: $BUILD_LOG"
    exit 1
fi
