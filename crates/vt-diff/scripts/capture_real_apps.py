#!/usr/bin/env python3
"""Capture real-app byte streams into the vt-diff corpus (one-off tool).

Spawns each app in a real PTY (80x24), feeds scripted keystrokes, records the
raw output bytes, and writes them to corpus/real_apps/<name>/input.esc using
the escaped-byte convention shared with crates/spike/tests/fixtures/replay
(\\e, \\n, \\r, \\t, \\\\, \\xHH; printable ASCII literal).

The captures are checked in; tests never re-run this script. Re-running it
regenerates the corpus files (content will differ — that's fine, both engines
always see identical bytes).
"""

import fcntl
import os
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

COLS, ROWS = 80, 24
HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS = os.path.join(HERE, "..", "corpus", "real_apps")


def esc_encode(data: bytes) -> str:
    out = []
    for b in data:
        if b == 0x1B:
            out.append("\\e")
        elif b == 0x0A:
            out.append("\\n")
        elif b == 0x0D:
            out.append("\\r")
        elif b == 0x09:
            out.append("\\t")
        elif b == 0x5C:
            out.append("\\\\")
        elif 0x20 <= b <= 0x7E:
            out.append(chr(b))
        else:
            out.append("\\x%02x" % b)
    return "".join(out)


def write_case(name: str, data: bytes) -> None:
    case_dir = os.path.join(CORPUS, name)
    os.makedirs(case_dir, exist_ok=True)
    with open(os.path.join(case_dir, "input.esc"), "w") as f:
        f.write(esc_encode(data))
    with open(os.path.join(case_dir, "size.txt"), "w") as f:
        f.write(f"{COLS} {ROWS}\n")
    print(f"{name}: {len(data)} raw bytes")


def capture_pty(argv, keys, timeout=15.0, settle=0.6):
    """Run argv in a PTY, send each (delay, bytes) key chunk, return output."""
    pid, fd = pty.fork()
    if pid == 0:  # child
        os.environ["TERM"] = "xterm-256color"
        os.environ["LINES"] = str(ROWS)
        os.environ["COLUMNS"] = str(COLS)
        try:
            os.execvp(argv[0], argv)
        finally:
            os._exit(127)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))

    out = bytearray()
    deadline = time.time() + timeout
    pending = list(keys)
    next_key_at = time.time() + settle
    exited = False
    while time.time() < deadline:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            if not data:
                break
            out.extend(data)
        if pending and time.time() >= next_key_at:
            delay, chunk = pending.pop(0)
            os.write(fd, chunk)
            next_key_at = time.time() + delay
        if not pending:
            done, _ = os.waitpid(pid, os.WNOHANG)
            if done:
                exited = True
                # drain whatever is left
                while True:
                    r, _, _ = select.select([fd], [], [], 0.2)
                    if not r:
                        break
                    try:
                        data = os.read(fd, 65536)
                    except OSError:
                        break
                    if not data:
                        break
                    out.extend(data)
                break
    os.close(fd)
    if not exited:
        try:
            os.kill(pid, 9)
        except ProcessLookupError:
            pass
        os.waitpid(pid, 0)
    return bytes(out)


def sample_file(lines=120):
    f = tempfile.NamedTemporaryFile(
        "w", suffix=".txt", prefix="vtcorpus_", delete=False
    )
    for i in range(1, lines + 1):
        f.write(f"line {i:03d}: the quick brown fox jumps over the lazy dog\n")
    f.close()
    return f.name


def main():
    path = sample_file()

    # vim: open file, insert a line, save-quit. Full-screen redraws, status
    # line, tilde fringe, alt-screen enter/leave.
    out = capture_pty(
        ["vim", "-u", "NONE", "-i", "NONE", path],
        [(0.5, b"ihello from the vt corpus\x1b"), (0.5, b":wq!\r")],
    )
    write_case("vim_edit", out)

    # vim: open + quit without editing.
    out = capture_pty(
        ["vim", "-u", "NONE", "-i", "NONE", path],
        [(0.5, b":q!\r")],
    )
    write_case("vim_open_quit", out)

    # less: page down, jump to end, quit.
    out = capture_pty(
        ["less", path],
        [(0.4, b" "), (0.4, b"G"), (0.4, b"q")],
    )
    write_case("less_page", out)

    # git log with color, piped (SGR-heavy but no cursor addressing).
    repo = os.path.join(HERE, "..", "..", "..")
    log = subprocess.run(
        ["git", "-c", "color.ui=always", "log", "--oneline", "--decorate", "-n", "15"],
        cwd=repo,
        stdout=subprocess.PIPE,
        check=True,
    )
    write_case("git_log_color", log.stdout)

    os.unlink(path)


if __name__ == "__main__":
    sys.exit(main())
