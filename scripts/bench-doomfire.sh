#!/usr/bin/env bash
# bench-doomfire.sh — measure DOOM-fire fps inside a real terminal window.
#
# DOOM-fire (github.com/const-void/DOOM-fire-zig) paints a full-screen fire
# animation as fast as the terminal drains the pty and prints a cumulative
# `[ N fps ]` counter into every frame. This is the whole-stack fps lane used
# for qwertty-term vs Ghostty comparisons (pty drain + parse + render, same
# method as upstream's public comparisons).
#
# Usage:
#   scripts/bench-doomfire.sh                        # bench qwertty-term (builds --release)
#   scripts/bench-doomfire.sh --terminal ghostty     # bench real Ghostty.app
#   scripts/bench-doomfire.sh --binary PATH --label lig-engine
#                                                    # bench a prebuilt binary (bisect lane)
#   scripts/bench-doomfire.sh --secs 15 --runs 5     # longer/more samples
#
# Outputs land in target/doomfire/<label>/:
#   fps.txt        one fps value per run + summary line (median, mean, loadavg)
#   grid.txt       `stty size` inside the terminal (fairness check)
#   capture.raw    raw byte stream of the LAST run (earlier runs' captures are
#                  deleted after fps extraction unless --keep-captures)
#
# How it works: the runner executes INSIDE the terminal under test and wraps
# DOOM-fire in script(1) so the byte stream (which contains the fps counter)
# is captured while still flowing to the real tty. A pipe would not work:
# DOOM-fire does TIOCGWINSZ on stdout and exits if it is not a tty. DOOM-fire
# never exits on its own; the runner kills it after --secs. The fps counter is
# frames / elapsed since fire start (startup animation excluded), so the last
# value in the capture is the run's average fps.
#
# The runner is wired up so ONE invocation works across the bisect window
# (the app was renamed ghostty-app -> qwertty-term mid-history):
#   new binaries   QWERTTY_TERM_COMMAND runs the runner; QWERTTY_TERM_SMOKE_MS backstop
#   old binaries   no command override existed; SHELL=<runner> makes the app
#                  exec the runner as its "shell"; GHOSTTY_APP_SMOKE_MS backstop
#   ghostty        --command=<runner> --quit-after-last-window-closed
#
# DOOM-fire binary: ~/local/DOOM-fire-zig/zig-out/bin/DOOM-fire (override with
# DOOM_FIRE_BIN). Build recipe if missing: clone the repo and build with a
# pinned Zig 0.14 toolchain (`zig build -Doptimize=ReleaseFast`); newer Zig
# breaks it.
#
# Noise: this is a whole-machine measurement. The script records the 1-min
# loadavg next to every sample; discard runs taken under load. Use --runs 5
# and compare medians.

set -euo pipefail

DOOM_FIRE_BIN="${DOOM_FIRE_BIN:-$HOME/local/DOOM-fire-zig/zig-out/bin/DOOM-fire}"
GHOSTTY_APP_BUNDLE="/Applications/Ghostty.app/Contents/MacOS/ghostty"

TERMINAL="qwertty-term"
BINARY=""
LABEL=""
SECS=15
RUNS=3
FONT_SIZE=8
KEEP_CAPTURES=0
while [[ $# -gt 0 ]]; do
    case "$1" in
    --terminal)
        TERMINAL="$2"
        shift 2
        ;;
    --binary)
        BINARY="$2"
        shift 2
        ;;
    --label)
        LABEL="$2"
        shift 2
        ;;
    --secs)
        SECS="$2"
        shift 2
        ;;
    --runs)
        RUNS="$2"
        shift 2
        ;;
    --font-size)
        FONT_SIZE="$2"
        shift 2
        ;;
    --keep-captures)
        KEEP_CAPTURES=1
        shift
        ;;
    -h | --help)
        sed -n '2,46p' "$0" | sed 's/^# \{0,1\}//'
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

[[ -x "$DOOM_FIRE_BIN" ]] || {
    echo "DOOM-fire binary not found/executable at $DOOM_FIRE_BIN" >&2
    echo "(clone github.com/const-void/DOOM-fire-zig, build with Zig 0.14)" >&2
    exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

LABEL="${LABEL:-$TERMINAL}"
OUT_DIR="$REPO_ROOT/target/doomfire/$LABEL"
mkdir -p "$OUT_DIR"
rm -f "$OUT_DIR"/{fps.txt,grid.txt,capture.raw,inner-grid.txt}

# Controlled config: a small font makes the fixed 800x480pt window a >=120-col
# grid (DOOM-fire warns and degrades below 120), and pointing the config dir
# here isolates the bench from the user's real config. Both env var epochs are
# set so pre-rename bisect binaries pick it up too.
CONFIG_DIR="$OUT_DIR/config"
mkdir -p "$CONFIG_DIR"
printf 'font-size = %s\n' "$FONT_SIZE" >"$CONFIG_DIR/config.toml"

if [[ "$TERMINAL" == "qwertty-term" && -z "$BINARY" ]]; then
    echo "==> building qwertty-term (release)"
    cargo build --release --quiet -p qwertty-term \
        --manifest-path "$REPO_ROOT/Cargo.toml"
    BINARY="$REPO_ROOT/target/release/qwertty-term"
fi

# The fire script runs inside script(1)'s nested pty: start DOOM-fire with
# endless newlines on stdin (it pauses for a keypress at the small-terminal
# warning and again after its termcap demo; EOF there makes it exit instead
# of burning — 'q' would quit, newline is safe), let it burn for the full
# budget, then kill it (it never exits on its own).
FIRE="$OUT_DIR/fire.sh"
cat >"$FIRE" <<EOF
#!/bin/sh
stty size >"$OUT_DIR/inner-grid.txt" 2>&1
yes '' | "$DOOM_FIRE_BIN" &
pid=\$!
sleep "$SECS"
kill "\$pid" 2>/dev/null
wait "\$pid" 2>/dev/null
exit 0
EOF
chmod +x "$FIRE"

# The runner executes inside the terminal under test (as QWERTTY_TERM_COMMAND,
# as $SHELL for pre-rename binaries, or as Ghostty's --command).
RUNNER="$OUT_DIR/runner.sh"
cat >"$RUNNER" <<EOF
#!/bin/sh
stty size >"$OUT_DIR/grid.txt" 2>&1
script -q "$OUT_DIR/capture.raw" /bin/sh "$FIRE" >/dev/null 2>&1
exit 0
EOF
chmod +x "$RUNNER"

# Wall budget per run: DOOM-fire's startup termcap demo (~5s of fixed-sleep
# marquee animation) + the fire itself + app startup/teardown headroom.
BUDGET_SECS=$((SECS + 40))

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

extract_fps() {
    # Last cumulative fps value in the capture; text is interleaved with
    # escape sequences so grep binary-safe.
    LC_ALL=C grep -aEo '\[ [0-9]+\.[0-9]+ fps \]' "$OUT_DIR/capture.raw" |
        tail -1 | LC_ALL=C grep -aEo '[0-9]+\.[0-9]+' || true
}

echo "==> DOOM-fire x$RUNS inside $TERMINAL (label: $LABEL, ${SECS}s burn per run)"
for run in $(seq 1 "$RUNS"); do
    rm -f "$OUT_DIR/capture.raw"
    case "$TERMINAL" in
    qwertty-term)
        QWERTTY_TERM_COMMAND="/bin/sh $RUNNER" \
            QWERTTY_TERM_SMOKE_MS=$((BUDGET_SECS * 1000)) \
            GHOSTTY_APP_SMOKE_MS=$((BUDGET_SECS * 1000)) \
            QWERTTY_TERM_CONFIG_DIR="$CONFIG_DIR" \
            GHOSTTY_RS_CONFIG_DIR="$CONFIG_DIR" \
            SHELL="$RUNNER" \
            run_with_timeout $((BUDGET_SECS + 15)) "$BINARY" >/dev/null 2>&1 || true
        ;;
    ghostty)
        [[ -x "$GHOSTTY_APP_BUNDLE" ]] || {
            echo "real Ghostty not found at $GHOSTTY_APP_BUNDLE" >&2
            exit 1
        }
        # Window size flags are in grid cells; match the grid qwertty-term
        # gets from its fixed 800x480pt window at this font size (check
        # grid.txt from a qwertty-term run and adjust if comparing).
        run_with_timeout $((BUDGET_SECS + 15)) "$GHOSTTY_APP_BUNDLE" \
            --command="/bin/sh $RUNNER" \
            --font-size="$FONT_SIZE" \
            --window-width=145 --window-height=42 \
            --quit-after-last-window-closed=true \
            --confirm-close-surface=false \
            --window-save-state=never \
            --shell-integration=none >/dev/null 2>&1 || true
        ;;
    esac

    fps="$(extract_fps)"
    load="$(sysctl -n vm.loadavg | awk '{print $2}')"
    if [[ -z "$fps" ]]; then
        echo "run $run: FAILED (no fps in capture; see $OUT_DIR)" | tee -a "$OUT_DIR/fps.txt" >&2
        continue
    fi
    echo "run $run: $fps fps (loadavg $load)" | tee -a "$OUT_DIR/fps.txt"
    if [[ "$KEEP_CAPTURES" == 1 ]]; then
        mv "$OUT_DIR/capture.raw" "$OUT_DIR/capture-$run.raw"
    elif [[ "$run" -lt "$RUNS" ]]; then
        rm -f "$OUT_DIR/capture.raw"
    fi
done

python3 - "$OUT_DIR/fps.txt" <<'PY' | tee -a "$OUT_DIR/fps.txt"
import statistics, sys, re

vals = []
with open(sys.argv[1]) as f:
    for line in f:
        m = re.match(r"run \d+: ([0-9.]+) fps", line)
        if m:
            vals.append(float(m.group(1)))
if not vals:
    print("summary: NO SAMPLES")
    sys.exit(1)
print(
    f"summary: median {statistics.median(vals):.1f} fps, "
    f"mean {statistics.fmean(vals):.1f}, "
    f"n={len(vals)}, spread {min(vals):.1f}-{max(vals):.1f}"
)
PY
echo "==> grid: $(cat "$OUT_DIR/grid.txt" 2>/dev/null | tr -d '\n') (inner: $(cat "$OUT_DIR/inner-grid.txt" 2>/dev/null | tr -d '\n'))"
