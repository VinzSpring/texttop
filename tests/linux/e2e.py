"""End-to-end check of texttop on Linux: run the real binary in a
pseudo-terminal, click and type like a user, and read the screen back.

    python3 tests/linux/e2e.py target/release/texttop [out-dir]

Every step dumps the screen to <out-dir>/NN_step.txt. Exit status is the
number of failed checks. Set E2E_NO_NETWORK=1 to skip the Contribute clone.
Meant to run inside tests/linux/Dockerfile (needs an X display for the
browser engine — xvfb-run provides one).
"""
import fcntl
import os
import pty
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time

import pyte

COLS, ROWS = 150, 45
EXE = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/texttop")
OUT = sys.argv[2] if len(sys.argv) > 2 else "/out"
os.makedirs(OUT, exist_ok=True)

# A throwaway profile and a small project to open.
SANDBOX = tempfile.mkdtemp(prefix="texttop-e2e-")
HOME = os.path.join(SANDBOX, "home")
PROJ = os.path.join(SANDBOX, "proj")
os.makedirs(HOME)
os.makedirs(os.path.join(PROJ, "sub"))
with open(os.path.join(PROJ, "README.md"), "w") as f:
    f.write("# Hello from e2e\n\nThis file was opened inside the IDE.\n")
with open(os.path.join(PROJ, "notes.txt"), "w") as f:
    f.write("notes\n")
with open(os.path.join(PROJ, "page.html"), "w") as f:
    f.write("<html><head><title>E2E Page</title></head>"
            "<body style='background:#fff'><h1>Rendered by WebKitGTK</h1></body></html>")
with open(os.path.join(PROJ, "sub", "inner.txt"), "w") as f:
    f.write("inner\n")

screen = pyte.Screen(COLS, ROWS)
stream = pyte.ByteStream(screen)
lock = threading.Lock()

env = dict(os.environ)
env.update({
    "TEXTTOP_HOME": HOME,
    "HOME": HOME,
    "TERM": "xterm-256color",
    "COLORTERM": "truecolor",
    "TEXTTOP_TRUECOLOR": "1",
    "SHELL": env.get("SHELL", "/bin/bash"),
})

pid, fd = pty.fork()
if pid == 0:
    fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    os.chdir(PROJ)
    os.execve(EXE, [EXE, PROJ], env)


def reader():
    while True:
        try:
            data = os.read(fd, 65536)
        except OSError:
            return
        if not data:
            return
        with lock:
            stream.feed(data)


threading.Thread(target=reader, daemon=True).start()

# ── helpers ──────────────────────────────────────────────────────────────────

failures = []
step_no = [0]


def lines():
    with lock:
        return list(screen.display)


def dump(name):
    step_no[0] += 1
    path = os.path.join(OUT, f"{step_no[0]:02d}_{name}.txt")
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines()))


def find(needle, min_x=0, min_y=0, max_x=None):
    for y, line in enumerate(lines()):
        if y < min_y:
            continue
        x = line.find(needle, min_x)
        if x >= 0 and (max_x is None or x < max_x):
            return x, y
    return None


def wait_for(needle, timeout=15.0, **kw):
    end = time.time() + timeout
    while time.time() < end:
        pos = find(needle, **kw)
        if pos:
            return pos
        time.sleep(0.2)
    return None


def check(name, ok, detail=""):
    dump(name)
    mark = "✓" if ok else "✗"
    print(f"  {mark} {name}{(' — ' + detail) if detail and not ok else ''}", flush=True)
    if not ok:
        failures.append(name)
    return ok


def write(s):
    os.write(fd, s.encode())


def click(x, y, button=0, double=False):
    for _ in range(2 if double else 1):
        write(f"\x1b[<{button};{x + 1};{y + 1}M")
        time.sleep(0.04)
        write(f"\x1b[<{button};{x + 1};{y + 1}m")
        time.sleep(0.08)
    time.sleep(0.3)


def click_text(needle, dx=1, button=0, double=False, **kw):
    pos = find(needle, **kw)
    if pos:
        click(pos[0] + dx, pos[1], button, double)
    return pos


def start_menu(query):
    click(2, ROWS - 1)
    time.sleep(0.6)
    write(query)
    time.sleep(0.4)
    write("\r")


F11 = "\x1b[23~"

# ── the run ──────────────────────────────────────────────────────────────────

print(f"texttop e2e · {EXE} · {COLS}x{ROWS}", flush=True)
try:
    check("desktop draws its taskbar", wait_for("Start", 30) is not None)
    time.sleep(2)
    check("IDE icon on the desktop", find("IDE", max_x=30) is not None)
    check("Files opened the project", wait_for("README.md", 10, min_x=30) is not None)

    # Files → right-click → Run app here → IDE
    pos = find("notes.txt", min_x=30)
    if pos:
        click(pos[0] + 2, pos[1], button=2)
        time.sleep(0.6)
        click_text("Run app here")
        time.sleep(1)
    chooser = wait_for("App groups", 5)
    check("Run app here lists app groups", chooser is not None)
    if chooser:
        click_text("IDE", min_y=chooser[1] + 1, min_x=30)
    ide = wait_for("Open folder/file", 15)
    check("IDE starts from Run app here", ide is not None)
    check("IDE runs in the project folder", find("IDE · proj") is not None)
    write(F11)
    time.sleep(1.5)
    prompt = wait_for("proj", 15, min_y=ROWS - 14)
    check("terminal frame starts in the project", prompt is not None)

    # Explorer → README.md opens as a tab.
    pos = find("README.md", max_x=40, min_y=3)
    if pos:
        click(pos[0] + 2, pos[1], double=True)
    check("file opens in a tab", wait_for("Hello from e2e", 10) is not None)
    check("tab strip shows the file", find("README.md  ✕") is not None or find("README.md ✕") is not None)

    # Directory watcher: a file created outside shows up by itself.
    with open(os.path.join(PROJ, "watched.txt"), "w") as f:
        f.write("new\n")
    check("explorer notices new files", wait_for("watched.txt", 6, max_x=40) is not None)

    # Web page in a tab: the WebKitGTK engine loads and titles it.
    pos = find("page.html", max_x=40, min_y=3)
    if pos:
        click(pos[0] + 2, pos[1], double=True)
    check("web page renders in a tab", wait_for("E2E Page", 40) is not None)
    check("library warnings stay off the screen", find("ccessibility") is None and find("WARNING") is None)

    # Open folder/file → picker → sub → Use this folder.
    click_text("Open folder/file")
    picker = wait_for("Use this folder", 5)
    check("folder picker opens", picker is not None)
    head = find("Open folder or file")
    pos = head and find("📁   sub", min_x=head[0] - 4, min_y=head[1] + 1)
    if pos:
        click(pos[0] + 1, pos[1], double=True)
        time.sleep(1)
    click_text("Use this folder", dx=2)
    check("IDE moves to the picked folder", wait_for("IDE · sub", 8) is not None)
    check("explorer follows", wait_for("inner.txt", 5, max_x=40) is not None)

    # App Groups settings.
    click_text("⚙ Settings")
    editor = wait_for("Frames", 8)
    check("App Groups settings open", editor is not None and find("File explorer") is not None)
    tri = find("▶ Try it")
    if tri:
        click_text("Close", min_x=tri[0], min_y=tri[1])
    time.sleep(0.8)

    # Contribute.
    start_menu("contrib")
    check("Contribute opens", wait_for("Get the source code", 8) is not None)
    public = None
    if not os.environ.get("E2E_NO_NETWORK"):
        probe = subprocess.run(
            ["git", "ls-remote", "--heads", "https://github.com/VinzSpring/texttop.git"],
            env=dict(os.environ, GIT_TERMINAL_PROMPT="0"),
            capture_output=True, timeout=60,
        )
        public = probe.returncode == 0
    if public is None:
        print("  · clone skipped (E2E_NO_NETWORK)")
    elif not public:
        # Anonymous clone is refused (private repository): Contribute must
        # say so instead of hanging on a password prompt.
        click_text("Get the source code")
        check("Contribute explains a refused clone", wait_for("wants a login", 90) is not None)
        # git's own "fatal: could not read Username" in the log is fine; a
        # live "Username for '…':" prompt on the screen is not.
        prompt = [l for l in lines() if "Username for" in l and "fatal" not in l]
        check("screen stays clean (no git prompt)", not prompt)
    else:
        click_text("Get the source code")
        cloned = wait_for("IDE · texttop", 300)
        check("Contribute clones and opens the IDE", cloned is not None)
        check("IDE shows the source", wait_for("Cargo.toml", 10) is not None)
finally:
    dump("final")
    try:
        os.kill(pid, 15)
    except ProcessLookupError:
        pass
    shutil.rmtree(SANDBOX, ignore_errors=True)

print(f"{len(failures)} failed" if failures else "all checks passed", flush=True)
print(f"screen dumps: {OUT}", flush=True)
sys.exit(len(failures))
