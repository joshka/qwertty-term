#!/usr/bin/env bash
# bench-vtebench.sh — run Alacritty's vtebench suite inside a real terminal
# window and collect the results.
#
# vtebench (the tool upstream Ghostty uses for terminal comparisons) measures
# PTY *read* throughput: it generates escape-sequence payloads and times how
# fast the terminal drains them. It must run INSIDE the terminal under test.
#
# Usage:
#   scripts/bench-vtebench.sh                      # bench qwertty-term (default)
#   scripts/bench-vtebench.sh --terminal ghostty   # bench real Ghostty.app
#   scripts/bench-vtebench.sh --max-secs 5         # shorter per-suite cap
#   scripts/bench-vtebench.sh --terminal ghostty \
#       --app-path ~/local/ghostty-main/macos/build/ReleaseLocal/Ghostty.app \
#       --label ghostty-main                       # drive an alternate bundle
#
# Flags:
#   --app-path <path>  ghostty only: point at a specific Ghostty.app bundle (or
#                      its inner MacOS/ghostty binary) instead of the default
#                      /Applications/Ghostty.app. Lets you A/B multiple builds.
#   --label <name>     override the target/vtebench/<name>/ output subdir so
#                      several ghostty builds (1.3.1, main) don't clobber each
#                      other. Defaults to the --terminal value.
#
# Outputs land in target/vtebench/<label>/:
#   results.dat   gnuplot-compatible per-sample times, ms (all suites)
#   summary.txt   per-suite mean/stddev derived from results.dat
#   grid.txt      `stty size` inside the terminal (fairness check)
#   exit-code.txt vtebench exit status
#
# NOTE: vtebench writes its escape-sequence payload to stdout — that IS the
# benchmark — so the runner must not redirect stdout; results are collected
# via `--silent --dat` instead.
#
# How each terminal is driven non-interactively:
#   qwertty-term  QWERTTY_TERM_COMMAND env override (crates/qwertty-term/src/
#               termio.rs) runs `/bin/sh -c <runner>` instead of $SHELL; the
#               app quits when the child exits. QWERTTY_TERM_SMOKE_MS is set as
#               a hard-timeout backstop.
#   ghostty     /Applications/Ghostty.app binary launched directly with
#               `--command=<runner> --quit-after-last-window-closed=true`,
#               window sized to match qwertty-term's default 80x24 grid.
#
# The vtebench checkout is pinned (VTEBENCH_COMMIT below) and lives at
# work/vtebench-upstream — a git-ignored scratch dir, auto-cloned if missing.
# An empty `[workspace]` table is appended to its Cargo.toml so cargo does not
# try to adopt it into the qwertty-term workspace.

set -euo pipefail

VTEBENCH_REPO="https://github.com/alacritty/vtebench"
VTEBENCH_COMMIT="ead80032e57dee2e75f0b51f2ea67528647d9944"
GHOSTTY_APP_BUNDLE="/Applications/Ghostty.app/Contents/MacOS/ghostty"

TERMINAL="qwertty-term"
MAX_SECS=10
APP_PATH=""
LABEL=""
while [[ $# -gt 0 ]]; do
    case "$1" in
    --terminal)
        TERMINAL="$2"
        shift 2
        ;;
    --max-secs)
        MAX_SECS="$2"
        shift 2
        ;;
    --app-path)
        APP_PATH="$2"
        shift 2
        ;;
    --label)
        LABEL="$2"
        shift 2
        ;;
    -h | --help)
        sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    *)
        echo "unknown argument: $1 (see --help)" >&2
        exit 2
        ;;
    esac
done

case "$TERMINAL" in
qwertty-term | ghostty) ;;
*)
    echo "--terminal must be 'qwertty-term' or 'ghostty', got '$TERMINAL'" >&2
    exit 2
    ;;
esac

# Resolve an --app-path override to the inner ghostty binary. Accept either the
# .app bundle or the binary itself.
if [[ -n "$APP_PATH" ]]; then
    if [[ "$TERMINAL" != "ghostty" ]]; then
        echo "--app-path only applies to --terminal ghostty" >&2
        exit 2
    fi
    if [[ -d "$APP_PATH" && "$APP_PATH" == *.app ]]; then
        GHOSTTY_APP_BUNDLE="$APP_PATH/Contents/MacOS/ghostty"
    else
        GHOSTTY_APP_BUNDLE="$APP_PATH"
    fi
fi

# Output subdir label; defaults to the terminal name.
LABEL="${LABEL:-$TERMINAL}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Locate (or clone) the pinned vtebench checkout. Checkouts live under work/
# next to the jj workspaces; from a workspace checkout that is ../, from the
# repo root it is work/.
VTEBENCH_DIR="${VTEBENCH_DIR:-}"
if [[ -z "$VTEBENCH_DIR" ]]; then
    for candidate in "$REPO_ROOT/../vtebench-upstream" "$REPO_ROOT/work/vtebench-upstream"; do
        if [[ -d "$candidate" ]]; then
            VTEBENCH_DIR="$(cd "$candidate" && pwd)"
            break
        fi
    done
fi
if [[ -z "$VTEBENCH_DIR" ]]; then
    VTEBENCH_DIR="$REPO_ROOT/../vtebench-upstream"
    echo "==> cloning vtebench @ $VTEBENCH_COMMIT into $VTEBENCH_DIR"
    git clone "$VTEBENCH_REPO" "$VTEBENCH_DIR"
fi
git -C "$VTEBENCH_DIR" checkout --quiet "$VTEBENCH_COMMIT" 2>/dev/null || {
    git -C "$VTEBENCH_DIR" fetch --quiet origin "$VTEBENCH_COMMIT"
    git -C "$VTEBENCH_DIR" checkout --quiet "$VTEBENCH_COMMIT"
}
# Keep the scratch checkout out of the qwertty-term cargo workspace.
grep -q '^\[workspace\]' "$VTEBENCH_DIR/Cargo.toml" ||
    printf '\n[workspace]\n' >>"$VTEBENCH_DIR/Cargo.toml"

echo "==> building vtebench (release)"
cargo build --release --quiet --manifest-path "$VTEBENCH_DIR/Cargo.toml"
VTEBENCH_BIN="$VTEBENCH_DIR/target/release/vtebench"

OUT_DIR="$REPO_ROOT/target/vtebench/$LABEL"
mkdir -p "$OUT_DIR"
rm -f "$OUT_DIR"/{summary.txt,results.dat,grid.txt,exit-code.txt,stderr.txt}

# The runner executes INSIDE the terminal under test. It records the grid,
# runs the full default suite, and writes its own exit code (the terminal
# process's exit status is about window teardown, not the bench). stdout is
# NOT redirected — the payload written to the tty is the benchmark.
RUNNER="$OUT_DIR/runner.sh"
cat >"$RUNNER" <<EOF
#!/bin/sh
stty size >"$OUT_DIR/grid.txt" 2>&1
"$VTEBENCH_BIN" -b "$VTEBENCH_DIR/benchmarks" \\
    --max-secs "$MAX_SECS" \\
    --silent \\
    --dat "$OUT_DIR/results.dat" \\
    2>"$OUT_DIR/stderr.txt"
echo \$? >"$OUT_DIR/exit-code.txt"
EOF
chmod +x "$RUNNER"

# Hard timeout backstop: 12 suites x max-secs, plus generous headroom for
# payload generation (shell loops emitting >=1 MiB each), warmup passes, and
# app startup/teardown.
BUDGET_SECS=$((12 * MAX_SECS + 300))

# run_with_timeout <secs> <cmd...> — macOS has no coreutils `timeout`.
run_with_timeout() {
    local secs="$1"
    shift
    "$@" &
    local pid=$!
    (
        sleep "$secs"
        kill "$pid" 2>/dev/null
    ) &
    local watchdog=$!
    local status=0
    wait "$pid" || status=$?
    kill "$watchdog" 2>/dev/null
    wait "$watchdog" 2>/dev/null || true
    return "$status"
}

echo "==> running vtebench inside $TERMINAL (budget ${BUDGET_SECS}s)"
case "$TERMINAL" in
qwertty-term)
    echo "==> building qwertty-term (release)"
    cargo build --release --quiet -p qwertty-term \
        --manifest-path "$REPO_ROOT/Cargo.toml"
    QWERTTY_TERM_COMMAND="/bin/sh $RUNNER" \
        QWERTTY_TERM_SMOKE_MS=$((BUDGET_SECS * 1000)) \
        run_with_timeout $((BUDGET_SECS + 30)) \
        "$REPO_ROOT/target/release/qwertty-term" || true
    ;;
ghostty)
    [[ -x "$GHOSTTY_APP_BUNDLE" ]] || {
        echo "real Ghostty not found at $GHOSTTY_APP_BUNDLE" >&2
        exit 1
    }
    # Match qwertty-term's default 80x24 grid; quit when the command exits.
    run_with_timeout "$BUDGET_SECS" "$GHOSTTY_APP_BUNDLE" \
        --command="/bin/sh $RUNNER" \
        --window-width=80 --window-height=24 \
        --quit-after-last-window-closed=true \
        --confirm-close-surface=false \
        --window-save-state=never \
        --shell-integration=none || true
    ;;
esac

if [[ ! -s "$OUT_DIR/results.dat" ]]; then
    echo "FAIL: no vtebench output collected (see $OUT_DIR)" >&2
    exit 1
fi
echo "==> results ($OUT_DIR/results.dat, grid: $(cat "$OUT_DIR/grid.txt" 2>/dev/null))"
python3 - "$OUT_DIR/results.dat" <<'PY' | tee "$OUT_DIR/summary.txt"
import statistics, sys

with open(sys.argv[1]) as f:
    names = f.readline().split()
    cols = [[] for _ in names]
    for line in f:
        for i, v in enumerate(line.split()):
            if v != "_":
                cols[i].append(int(v))

print(f"{'suite':<30} {'samples':>7} {'mean ms':>8} {'stddev':>7}")
for name, samples in zip(names, cols):
    mean = statistics.fmean(samples)
    stddev = statistics.stdev(samples) if len(samples) > 1 else 0.0
    print(f"{name:<30} {len(samples):>7} {mean:>8.1f} {stddev:>7.1f}")
PY
