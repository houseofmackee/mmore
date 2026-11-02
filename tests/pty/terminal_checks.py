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


#: Longest any single run may take before it is killed and counted as a failure.
RUN_TIMEOUT = 30


def finish(pid, fd, out=b""):
    """Wait for the child to exit, reading its output meanwhile (a session leader
    on macOS cannot exit while its terminal output is unread). Kills the process
    group after RUN_TIMEOUT. Returns (exit code or None if killed, output)."""
    deadline = time.time() + RUN_TIMEOUT
    while True:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try:
                out += os.read(fd, 65536)
            except OSError:
                pass
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            return os.waitstatus_to_exitcode(status), out
        if time.time() > deadline:
            try:
                os.killpg(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.waitpid(pid, 0)
            return None, out


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
    "WAIT" pauses (for background work), "PAUSE" waits briefly, and "RESIZE"
    applies `resize` (rows, cols).
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
        elif k == "PAUSE":
            time.sleep(0.7)
            drain(0.3)
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
    code, out = finish(pid, fd, out)
    if code is None:
        check(f"finished within {RUN_TIMEOUT}s: {cmd}", False)
    return out.decode(errors="replace"), code, snapshots


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
            pass
        finish(pid, fd)
    return bool(done)


def early_key(cmd, key, delay, rows=10, cols=80):
    """Send `key` `delay` seconds after starting, before the first screen is up,
    then quit. Returns all output."""
    pid, fd = spawn(cmd, clean_env(), rows, cols)
    out = b""
    start = time.time()
    sent = False
    while time.time() - start < 2.0:
        if not sent and time.time() - start >= delay:
            os.write(fd, key)
            sent = True
        r, _, _ = select.select([fd], [], [], 0.01)
        if r:
            try:
                out += os.read(fd, 65536)
            except OSError:
                break
    try:
        os.write(fd, b"q")
    except OSError:
        pass
    _, out = finish(pid, fd, out)
    return out.decode(errors="replace")


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
    _, out = finish(pid, fd, out)
    return out, elapsed


def check(name, condition, detail=""):
    print(("PASS " if condition else "FAIL ") + name + (f"  [{detail}]" if detail else ""), flush=True)
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
    # Keys pressed right away (while mmore asks the terminal about its
    # background) must not be lost.
    check("Enter pressed at startup is not lost", "--More--(10%)" in early_key(f"seq 1 100 | {BIN}", b"\r", 0.01))
    check("Space pressed at startup is not lost", "--More--(18%)" in early_key(f"seq 1 100 | {BIN}", b" ", 0.0))
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
    out, code, snaps = run(f"{BIN} --always-page {links}", ["r", "q"], rows=10, cols=60)
    check("r shows the raw source", "# Links" in after(snaps, 0) and "Showing raw source" in after(snaps, 0))
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


def less_keys(tmp, n100):
    # A count before g, %, and j: go to a line, a percentage, or move N lines.
    out, code, snaps = run(f"{BIN} {n100}", ["4", "2", "g", "5", "0", "%", "q"], rows=10, cols=40)
    check("42g goes to line 42", "\x1b[1;1H\x1b[2K42" in after(snaps, 2))
    check("50% goes halfway", "\x1b[1;1H\x1b[2K51" in after(snaps, 5))
    check("a count shows while typing", "\x1b[10;1H\x1b[2K4" in after(snaps, 0))

    # -S cuts long lines off; Right scrolls sideways; w wraps again.
    wide = os.path.join(tmp, "wide.txt")
    with open(wide, "w") as f:
        f.write("left" + "." * 50 + "right\n" + "short\n")
    # Each Right scrolls half a screen: two reach the end of a 59-column line.
    out, code, snaps = run(f"{BIN} -S --always-page {wide}", ["\x1b[C", "\x1b[C", "w", "q"], rows=10, cols=30)
    first = snaps[0].decode(errors="replace")
    check("-S cuts long lines off", "left" in first and "right" not in first)
    check("Right scrolls sideways", "right" in after(snaps, 1) and "from column" in after(snaps, 1))
    check("w wraps long lines again", "Long lines wrap" in after(snaps, 2))

    # &pattern shows only matching lines.
    log = os.path.join(tmp, "app.log")
    with open(log, "w") as f:
        f.write("".join(f"{'ERROR' if i % 10 == 0 else 'info'} event {i}\n" for i in range(1, 101)))
    out, code, snaps = run(f"{BIN} {log}", ["&", "E", "R", "R", "\r", "q"], rows=12, cols=40)
    filtered = after(snaps, 4)
    check("&ERR shows only matching lines", "ERROR event 10" in filtered and "info event" not in filtered and "[&ERR: 10 lines]" in filtered)

    # v opens the editor at the current line, and the file is reloaded after.
    bin_dir = os.path.join(tmp, "editor-bin")
    os.mkdir(bin_dir)
    fake = os.path.join(bin_dir, "vim")
    record = os.path.join(tmp, "editor-args.txt")
    with open(fake, "w") as f:
        f.write(f'#!/bin/sh\necho "$@" > {record}\nfor last; do :; done\necho "added by the editor" >> "$last"\n')
    os.chmod(fake, 0o755)
    edited = os.path.join(tmp, "edit-me.txt")
    with open(edited, "w") as f:
        f.write("".join(f"{i}\n" for i in range(1, 31)))
    # WAIT: a freshly written script can take a moment to start the first time.
    out, code, snaps = run(f"{BIN} {edited}", ["1", "5", "g", "v", "WAIT", "G", "q"], rows=10, cols=40, env={"EDITOR": fake})
    args = open(record).read().strip() if os.path.exists(record) else ""
    check("v runs the editor at the current line", args == f"+15 {edited}", args)
    handover = snaps[5][len(snaps[3]):].decode(errors="replace")
    check("the terminal is handed to the editor and taken back", ALT_OFF in handover and ALT_ON in handover)
    check("the file is reloaded after editing", "added by the editor" in after(snaps, 5) and code == 0)


def input_colors(tmp):
    log = os.path.join(tmp, "colors.log")
    with open(log, "w") as f:
        for i in range(1, 31):
            f.write(f"\x1b[31mred {i}\x1b[0m and \x1b[1;32mgreen\x1b[0m\x1b[K\n")
    out, code, snaps = run(f"{BIN} {log}", ["/", "r", "e", "d", " ", "2", "\r", "q"], rows=10, cols=40)
    first = snaps[0].decode(errors="replace")
    check("colors in the input are shown in the pager", "\x1b[31mred 1\x1b[0m" in first and "\x1b[1;32mgreen" in first)
    check("no escape codes are shown as text", "^[" not in out)
    check("search highlights combine with input colors", "\x1b[7;31mred 2" in after(snaps, 6))
    out, code, _ = run(f"{BIN} --show-escapes {log}", ["q"], rows=10, cols=60)
    check("--show-escapes shows the codes", "^[[31mred 1^[[0m" in out)


def streaming():
    # A producer that pauses: the first lines must show before it finishes.
    slow = f"(seq 1 5; sleep 3; seq 6 40) | {BIN}"
    out, code, snaps = run(slow, ["PAUSE", "WAIT", "WAIT", "q"], rows=10, cols=40)
    # Within about a second, well before the producer's pause ends.
    first = snaps[1].decode(errors="replace")
    check(
        "piped input is paged before it ends",
        ALT_ON in first and "\x1b[5;1H\x1b[2K5" in first and "(waiting for input)" in first,
    )
    later = snaps[3][len(snaps[1]):].decode(errors="replace")
    check("later input appears, then the status shows a percentage", "--More--(" in later and code == 0)

    # A producer that is still running (like tail -f): quitting must not wait for
    # it. The screen is restored right after q, while the producer still sleeps.
    out, code, snaps = run(f"(seq 1 3; sleep 4) | {BIN}", ["q"], rows=10, cols=40)
    check("q quits while input is still open", ALT_OFF in after(snaps, 0) and code == 0)

    # F follows new input at the end.
    follow = f"(seq 1 20; sleep 1; seq 21 60; sleep 4) | {BIN}"
    out, code, snaps = run(follow, ["F", "WAIT", "q"], rows=10, cols=40)
    followed = after(snaps, 1)
    check(
        "F keeps the newest input in view",
        "Following new input" in after(snaps, 0) and "\x1b[9;1H\x1b[2K60" in followed,
    )

    # Printing (output is a pipe): lines are copied as they arrive.
    out, code, snaps = run(f"(echo first; sleep 2; echo second) | {BIN} | cat", [], rows=10, cols=40)
    early = snaps[0].decode(errors="replace")
    check("printing copies piped input before it ends", "first" in early and "second" not in early)
    out, code, _ = run(f"(echo first; sleep 1; echo second) | {BIN} | cat", ["WAIT"], rows=10, cols=40)
    check("printing finishes when the input ends", "first" in out and "second" in out and code == 0)

    # Process substitution (a pipe given as a file name) streams the same way.
    subst = f"bash -c '{BIN} <(seq 1 5; sleep 3; seq 6 40)'"
    out, code, snaps = run(subst, ["PAUSE", "WAIT", "WAIT", "q"], rows=10, cols=40)
    first = snaps[1].decode(errors="replace")
    check(
        "<(command) is paged before it ends",
        ALT_ON in first and "\x1b[5;1H\x1b[2K5" in first and "(waiting for input)" in first,
    )
    check("<(command) shows the rest when it ends", "--More--(" in snaps[3][len(snaps[1]):].decode(errors="replace"))

    # Short piped input is still printed directly, without the pager.
    out, code, _ = run(f"printf 'a\\nb\\n' | {BIN}", [], rows=10, cols=40)
    check("short piped input is printed without the pager", ALT_ON not in out and "b" in out)


# A tiny shell with job control: runs mmore as a foreground job in its own
# process group, reports when it stops, then resumes it like `fg`.
JOB_SHELL = r"""
import os, signal, sys
signal.signal(signal.SIGTTOU, signal.SIG_IGN)
pid = os.fork()
if pid == 0:
    os.setpgid(0, 0)
    signal.signal(signal.SIGTTOU, signal.SIG_DFL)
    os.execvp(sys.argv[1], sys.argv[1:])
os.setpgid(pid, pid)
os.tcsetpgrp(0, pid)
while True:
    _, status = os.waitpid(pid, os.WUNTRACED)
    if os.WIFSTOPPED(status):
        os.tcsetpgrp(0, os.getpgrp())
        print("STOPPED", flush=True)
        sys.stdin.readline()
        print("RESUMING", flush=True)
        os.tcsetpgrp(0, pid)
        os.killpg(pid, signal.SIGCONT)
        continue
    print("EXIT=%d" % os.waitstatus_to_exitcode(status), flush=True)
    break
"""


def suspend(n100):
    job = os.path.join(tempfile.gettempdir(), f"mmore-job-shell-{os.getpid()}.py")
    with open(job, "w") as f:
        f.write(JOB_SHELL)
    try:
        cmd = f"python3 -I {job} {BIN} {n100}"
        out, code, snaps = run(cmd, ["\x1a", "\r", "j", "q"], rows=10, cols=40)
    finally:
        os.remove(job)
    stopped = after(snaps, 0)
    check("Ctrl-Z restores the terminal and stops mmore", ALT_OFF in stopped and "STOPPED" in stopped)
    resumed = after(snaps, 1)
    check("fg resumes the pager and redraws", "RESUMING" in resumed and ALT_ON in resumed and "--More--" in resumed)
    check("pager works after resuming", "\x1b[1;1H\x1b[2K2" in after(snaps, 2) and "EXIT=0" in out)

    # Without job control (no shell to resume it), Ctrl-Z must not hang mmore.
    out, code, snaps = run(f"{BIN} {n100}", ["\x1a", "q"], rows=10, cols=40)
    check("Ctrl-Z without job control keeps paging", ALT_ON in after(snaps, 0) and code == 0)


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
        less_keys(tmp, n100)
        input_colors(tmp)
        streaming()
        suspend(n100)
        background()
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print(f"\n{len(failed)} failed" if failed else "\nAll checks passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
