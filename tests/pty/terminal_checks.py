#!/usr/bin/env python3
"""End-to-end checks that run mmore in a pseudo-terminal.

These cover what unit tests cannot: raw mode, the alternate screen, real key and
mouse input, resizing, and the terminal background query. Run after building:

    cargo build && python3 tests/pty/terminal_checks.py [path/to/mmore]

The default binary is target/debug/mmore. Exits with status 1 if a check fails.
Unix only (uses the pty module).
"""

import fcntl
import os
import pty
import re
import select
import shutil
import signal
import struct
import sys
import tempfile
import termios
import time

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BIN = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else os.path.join(REPO, "target", "debug", "mmore"))
FIXTURES = os.path.join(REPO, "tests", "fixtures")
SRC = os.path.join(REPO, "src", "main.rs")
MD = os.path.join(FIXTURES, "markdown", "basic.md")
SHORT_MD = os.path.join(FIXTURES, "sample.md")
HELLO_RS = os.path.join(FIXTURES, "hello.rs")

ALT_ON, ALT_OFF = "\x1b[?1049h", "\x1b[?1049l"
failed = []


def clean_env(extra=None):
    """A fixed environment, so the user's own settings cannot change results."""
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": os.environ.get("HOME", "/tmp"),
        "LANG": "en_US.UTF-8",
        "TERM": "xterm-256color",
    }
    env.update(extra or {})
    return env


def set_size(fd, rows, cols):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def spawn(cmd, env, rows, cols):
    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe("sh", ["sh", "-c", cmd], env)
    set_size(fd, rows, cols)
    return pid, fd


def run(cmd, keys, rows=10, cols=80, resize=None, env=None):
    """Run `cmd`, send `keys` one at a time, and return (output, exit code, snapshots).

    After each key, output is read until it has been quiet for a moment. A
    snapshot of everything read so far is taken at the start and after each key.
    "WAIT" pauses (for background work), "RESIZE" applies `resize` (rows, cols).
    """
    pid, fd = spawn(cmd, clean_env(env), rows, cols)
    out = b""

    def drain(first=2.0, quiet=0.3):
        nonlocal out
        start = time.time()
        got = False
        while True:
            wait = quiet if got else max(0.05, first - (time.time() - start))
            r, _, _ = select.select([fd], [], [], wait)
            if not r:
                return
            try:
                data = os.read(fd, 65536)
            except OSError:
                return
            if not data:
                return
            out += data
            got = True

    drain(3.0)
    snapshots = [out]
    for k in keys:
        if k == "WAIT":
            time.sleep(1.5)
            drain(0.5)
        elif k == "RESIZE":
            set_size(fd, *resize)
            os.kill(pid, signal.SIGWINCH)
            drain()
        else:
            try:
                os.write(fd, k.encode())
            except OSError:
                break
            drain()
        snapshots.append(out)
    _, status = os.waitpid(pid, 0)
    return out.decode(errors="replace"), os.waitstatus_to_exitcode(status), snapshots


def after(snaps, i):
    """Output that arrived between snapshot i and i + 1 (sliced as bytes)."""
    return snaps[i + 1][len(snaps[i]):].decode(errors="replace")


def exits_after(cmd, keys, rows=10, cols=80, env=None):
    """Send keys; report whether the process then exited on its own."""
    pid, fd = spawn(cmd, clean_env(env), rows, cols)

    def drain(t):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.05)
            if r:
                try:
                    if not os.read(fd, 65536):
                        return
                except OSError:
                    return

    drain(0.6)
    for k in keys:
        try:
            os.write(fd, k.encode())
        except OSError:
            break
        drain(0.3)
    time.sleep(0.5)
    done, _ = os.waitpid(pid, os.WNOHANG)
    if not done:
        try:
            os.write(fd, b"q")
        except OSError:
            os.waitpid(pid, 0)
            return True
        drain(1.0)
        os.waitpid(pid, 0)
    return bool(done)


def answering_terminal(reply, env=None):
    """Run mmore on a terminal that answers the OSC 11 query with `reply`.

    `reply` is raw bytes (or None for a terminal that never answers). Afterwards
    the shell prints whether the terminal is back in canonical mode.
    Returns (output bytes, seconds taken).
    """
    cmd = f"{BIN} --no-pager {HELLO_RS}; stty -a | grep -o -- '-\\?icanon'; echo DONE"
    pid, fd = spawn(cmd, clean_env({"COLORTERM": "truecolor", **(env or {})}), 30, 100)
    out = b""
    start = time.time()
    answered = False
    while time.time() - start < 5:
        r, _, _ = select.select([fd], [], [], 0.01)
        if not r:
            continue
        try:
            data = os.read(fd, 65536)
        except OSError:
            break
        if not data:
            break
        out += data
        if not answered and b"\x1b]11;?" in out and reply is not None:
            os.write(fd, reply)
            answered = True
        if b"DONE" in out:
            break
    elapsed = time.time() - start
    os.waitpid(pid, 0)
    return out, elapsed


def check(name, condition, detail=""):
    print(("PASS " if condition else "FAIL ") + name + (f"  [{detail}]" if detail else ""))
    if not condition:
        failed.append(name)


def pager_basics(tmp):
    n100 = os.path.join(tmp, "n.txt")
    with open(n100, "w") as f:
        f.write("".join(f"{i}\n" for i in range(1, 101)))

    out, code, snaps = run(f"{BIN} {n100}", [" ", "G", "g", "q"])
    s = [x.decode(errors="replace") for x in snaps]
    check("enters alternate screen", ALT_ON in s[0])
    check("initial status 9%", "--More--(9%)" in s[0])
    check("space pages to 18%", "--More--(18%)" in after(snaps, 0))
    check("G shows (END)", "(END)" in after(snaps, 1) and "100" in after(snaps, 1))
    check("g back to top", "--More--(9%)" in after(snaps, 2))
    check("q exits 0 and restores screen", code == 0 and ALT_OFF in out)

    out, code, _ = run(f"seq 1 100 | {BIN}", ["\r", "q"])
    check("piped stdin pages with keys from tty", "--More--(10%)" in out and code == 0)
    out, code, _ = run(f"seq 1 5 | {BIN}", [])
    check("short input prints without pager", ALT_ON not in out and "5" in out and code == 0)
    out, code, _ = run(f"seq 1 5 | {BIN} --always-page", ["q"])
    check("--always-page pages short input", ALT_ON in out and "(END)" in out)
    out, code, _ = run(f"seq 1 25 | {BIN} -e", [" ", " ", " ", "q"])
    check("-e exits after paging past end", code == 0 and ALT_OFF in out)
    out, code, snaps = run(f"seq 1 100 | {BIN}", ["RESIZE", "q"], resize=(20, 80))
    check("resize redraws with taller viewport (19%)", "--More--(19%)" in after(snaps, 0))
    out, code, _ = run(f"seq 1 100 | {BIN}", ["\x03"])
    check("ctrl-c quits cleanly", code == 0 and ALT_OFF in out)
    check("-e exits after 3 spaces on 25 lines", exits_after(f"seq 1 25 | {BIN} -e", [" ", " ", " "]))
    check("without -e stays open at end", not exits_after(f"seq 1 25 | {BIN}", [" ", " ", " "]))
    return n100


def colors():
    out, code, _ = run(f"{BIN} {SRC}", ["WAIT", "q"], rows=20)
    check("pager highlights code in 256 colors (lazily)", "38;5;" in out and code == 0)
    out, code, _ = run(f"{BIN} {SRC}", ["WAIT", "q"], rows=20, env={"COLORTERM": "truecolor"})
    check("pager uses truecolor when advertised", "38;2;" in out)
    out, code, _ = run(f"{BIN} {SRC}", ["q"], rows=20, env={"NO_COLOR": "1"})
    check("NO_COLOR disables colors in pager", "38;5;" not in out and "38;2;" not in out and "mod ansi;" in out)
    out, code, _ = run(f"{BIN} {SRC}", ["G", "q"], rows=20)
    check("styles reset at end of each row", "\x1b[0m" in out)


def markdown():
    out, code, _ = run(f"{BIN} {MD}", ["q"], rows=20, cols=80)
    check("markdown is rendered in the pager", "═" * 80 in out and "# mmore" not in out and code == 0)
    out, code, snaps = run(f"{BIN} {MD}", ["RESIZE", "q"], rows=20, cols=80, resize=(20, 40))
    resized = after(snaps, 0)
    check("resize re-renders markdown at new width", "═" * 40 in resized and "═" * 41 not in resized)
    out, code, _ = run(f"{BIN} {SHORT_MD}", [], rows=20, cols=40)
    check("short markdown is rendered without pager", ALT_ON not in out and "═" * 40 in out and "# Title" not in out)
    out, code, _ = run(f"{BIN} --plain {SHORT_MD}", [], rows=20, cols=40)
    check("--plain shows markdown source on a terminal", "# Title" in out)


def hyperlinks(tmp):
    links = os.path.join(tmp, "links.md")
    with open(links, "w") as f:
        f.write("# Links\n\nSee [the site](https://example.com) and [a file](docs/x.md).\n")
    iterm = {"TERM_PROGRAM": "iTerm.app"}
    out, code, _ = run(f"{BIN} --always-page {links}", ["q"], rows=20, env=iterm)
    check(
        "pager emits OSC 8 on a supporting terminal",
        "\x1b]8;id=mmore0;https://example.com\x1b\\" in out and "docs/x.md\x1b\\" not in out,
    )
    out, code, _ = run(f"{BIN} --always-page {links}", ["q"], rows=20, env={"TERM_PROGRAM": "Apple_Terminal"})
    check("no OSC 8 on Apple Terminal", "\x1b]8;" not in out and "the site" in out)
    out, code, _ = run(f"{BIN} {links}", [], rows=20, env=iterm)
    check("short file printed directly still gets OSC 8", ALT_ON not in out and "\x1b]8;id=mmore0;" in out)
    return links


def pager_features(n100, links):
    out, code, snaps = run(f"{BIN} {n100}", ["/", "4", "2", "\r", "n", "q"], rows=10, cols=40)
    check("search prompt is shown", "/" in after(snaps, 0) and "\x1b[?25h" in after(snaps, 0))
    check("search jumps and highlights in reverse video", "\x1b[7m42\x1b[0m" in after(snaps, 3) and code == 0)
    out, code, _ = run(f"{BIN} {n100}", ["/", "7", "7", "\r", "q"], rows=10, cols=40, env={"NO_COLOR": "1"})
    check("search highlight works with NO_COLOR", "\x1b[7m77\x1b[0m" in out and "38;5" not in out)
    out, code, snaps = run(f"{BIN} {n100} {links}", [":", "n", "q"], rows=10, cols=60)
    # The status line is cut to the terminal width, so long temp paths may not fit.
    check(":n switches to the next file", "[2/2] " in after(snaps, 1) and "Links" in after(snaps, 1))
    out, code, snaps = run(f"{BIN} --always-page {links}", ["m", "q"], rows=10, cols=60)
    check("m shows the raw source", "# Links" in after(snaps, 0) and "Showing raw source" in after(snaps, 0))
    out, code, snaps = run(f"{BIN} {n100}", ["h", "q", "q"], rows=30)
    check("help screen opens and q returns before quitting", "mmore keys" in after(snaps, 0) and code == 0)
    out, code, _ = run(f"{BIN} -n {n100}", ["q"], rows=10, cols=40)
    check("-n shows a line number gutter", re.search(r"  1 (\x1b\[0m)?1", out) is not None)


def mouse(n100):
    wheel_down, wheel_up = "\x1b[<65;10;5M", "\x1b[<64;10;5M"
    out, code, snaps = run(f"{BIN} --mouse {n100}", [wheel_down, wheel_down, wheel_up, "q"], rows=10, cols=40)
    check(
        "--mouse turns on button reporting (not motion)",
        "\x1b[?1000h\x1b[?1006h" in out and "?1003h" not in out and "?1002h" not in out,
    )
    check(
        "wheel down scrolls 3 lines, twice",
        "\x1b[1;1H\x1b[2K4" in after(snaps, 0) and "\x1b[1;1H\x1b[2K7" in after(snaps, 1),
    )
    check("wheel up scrolls back", "\x1b[1;1H\x1b[2K4" in after(snaps, 2))
    check(
        "--mouse turns reporting off on exit",
        out.rfind("\x1b[?1006l\x1b[?1000l") > out.rfind("\x1b[?1000h") and code == 0,
    )
    out, code, _ = run(f"{BIN} {n100}", ["q"], rows=10, cols=40)
    check("without --mouse no mouse sequences at all", "?1000" not in out and "?1006" not in out)


def background():
    # base16-ocean.dark colors the `fn` keyword like this; InspiredGitHub does not.
    dark_fn = b"180;142;173"
    da1 = b"\x1b[?62;22c"
    out, t = answering_terminal(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\" + da1)
    check("light background reply selects the light theme", dark_fn not in out and b"38;2;" in out, f"{t:.2f}s")
    out, t = answering_terminal(b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07" + da1)
    check("dark background reply keeps the dark theme", dark_fn in out, f"{t:.2f}s")
    out, t = answering_terminal(da1)
    check("terminal without OSC 11 (DA1 only): dark, no waiting", dark_fn in out and t < 1.0, f"{t:.2f}s")
    out, t = answering_terminal(None)
    check("terminal that never answers: times out quickly, dark", dark_fn in out and t < 1.2, f"{t:.2f}s")
    check("background query is sent when allowed", b"\x1b]11;?" in out)
    light = b"\x1b]11;rgb:ffff/ffff/ffff\x07" + da1
    out, _ = answering_terminal(light, {"SSH_CONNECTION": "1 2 3 4"})
    check("no background query over SSH", b"\x1b]11;?" not in out and dark_fn in out)
    out, _ = answering_terminal(light, {"COLORFGBG": "0;15"})
    check("COLORFGBG wins without a query", b"\x1b]11;?" not in out and dark_fn not in out)
    out, _ = answering_terminal(light)
    check("terminal is back in canonical mode afterwards", b"-icanon" not in out and b"icanon" in out)
    out, _ = answering_terminal(light, {"NO_COLOR": "1"})
    check("no background query without colors", b"\x1b]11;?" not in out)


def main():
    if not os.path.exists(BIN):
        sys.exit(f"mmore binary not found at {BIN}; run cargo build first")
    tmp = tempfile.mkdtemp(prefix="mmore-pty-")
    try:
        n100 = pager_basics(tmp)
        colors()
        markdown()
        links = hyperlinks(tmp)
        pager_features(n100, links)
        mouse(n100)
        background()
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print(f"\n{len(failed)} failed" if failed else "\nAll checks passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
