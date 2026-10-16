#!/usr/bin/env python3
"""Capture screenshots of texttop and compile them into the README and a handbook.

Drives the real binary inside a pseudo-terminal, reads the screen with pyte,
and renders every capture as an SVG (crisp quadrant pixels and box lines are
drawn as shapes, so images look right regardless of the viewer's font). Then:

  docs/screenshots/<scene>.svg   one file per scene
  docs/HANDBOOK.md               user handbook: every app, pictured and explained
  README.md                      gallery between <!-- screenshots:start/end -->

Requirements:  pip install pyte            (all platforms)
               pip install pywinpty        (Windows only)

Usage:
  python scripts/screenshots.py                 # build release if needed, capture all
  python scripts/screenshots.py --only studio_session,files
  python scripts/screenshots.py --skip web      # the web scene needs the webview engine
  python scripts/screenshots.py --docs-only     # regenerate markdown from existing SVGs
  python scripts/screenshots.py --list

Desktop state goes to a throwaway TEXTTOP_HOME (and HOME on Linux/macOS), so
your own layout and session are never touched. On Windows, apps that keep
data under the real home directory (browser history, Studio saves) are only
opened, never saved.
"""

from __future__ import annotations

import argparse
import html
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"
SHOTS = DOCS / "screenshots"
IS_WIN = os.name == "nt"

# ── keys ────────────────────────────────────────────────────────────────────

ENTER, ESC, TAB, SPACE = "\r", "\x1b", "\t", " "
UP, DOWN, RIGHT, LEFT = "\x1b[A", "\x1b[B", "\x1b[C", "\x1b[D"
PGDN = "\x1b[6~"
F1, F3, F4 = "\x1bOP", "\x1bOR", "\x1bOS"
F6, F11, F12 = "\x1b[17~", "\x1b[23~", "\x1b[24~"

# ── pseudo-terminal drivers ─────────────────────────────────────────────────


class Pty:
    """Minimal cross-platform PTY: spawn, write, background-read into pyte."""

    def __init__(self, argv: list[str], cols: int, rows: int, env: dict, cwd: Path):
        import pyte

        self.screen = pyte.Screen(cols, rows)
        self.stream = pyte.ByteStream(self.screen)
        self.lock = threading.Lock()
        self.alive = True
        if IS_WIN:
            import winpty

            self.proc = winpty.PtyProcess.spawn(
                argv, dimensions=(rows, cols), env=env, cwd=str(cwd)
            )
            self._write = lambda s: self.proc.write(s)
            self._read = self._read_win
        else:
            import fcntl
            import pty
            import termios

            pid, fd = pty.fork()
            if pid == 0:  # child
                os.chdir(cwd)
                os.execvpe(argv[0], argv, env)
            fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
            self.pid, self.fd = pid, fd
            self._write = lambda s: os.write(fd, s.encode())
            self._read = lambda: os.read(fd, 65536)
        threading.Thread(target=self._pump, daemon=True).start()

    def _read_win(self) -> bytes:
        data = self.proc.read(65536)
        return data.encode("utf-8", "surrogateescape") if isinstance(data, str) else data

    def _pump(self):
        while self.alive:
            try:
                data = self._read()
            except Exception:
                return
            if not data:
                time.sleep(0.01)
                continue
            with self.lock:
                self.stream.feed(data)

    def write(self, s: str):
        self._write(s)

    def lines(self) -> list[str]:
        """Screen rows where string index == terminal column.

        pyte's own `display` drops the empty cell after a wide glyph (emoji),
        which shifts every later column on the row by one.
        """
        with self.lock:
            s = self.screen
            return [
                "".join(s.buffer[y][x].data or "\x00" for x in range(s.columns))
                for y in range(s.lines)
            ]

    def buffer(self):
        """Snapshot of the cell grid: rows of pyte Char, wide-char tails as None."""
        with self.lock:
            s = self.screen
            grid = []
            for y in range(s.lines):
                row = s.buffer[y]
                grid.append([row[x] for x in range(s.columns)])
            return grid

    def close(self):
        self.alive = False
        try:
            if IS_WIN:
                self.proc.terminate(force=True)
            else:
                import signal

                os.kill(self.pid, signal.SIGKILL)
                os.waitpid(self.pid, 0)
        except Exception:
            pass


# ── scene driver ────────────────────────────────────────────────────────────


class Session:
    def __init__(self, exe: Path, cols: int, rows: int, arg: Path | None, home: Path):
        env = dict(os.environ)
        env.pop("NO_COLOR", None)  # would turn every capture monochrome
        env["TEXTTOP_HOME"] = str(home)
        env["TEXTTOP_TRUECOLOR"] = "1"
        env.setdefault("TERM", "xterm-256color")
        env["COLORTERM"] = "truecolor"
        if not IS_WIN:
            env["HOME"] = str(home)  # everything under ~ stays in the sandbox
        argv = [str(exe)] + ([str(arg)] if arg else [])
        self.pty = Pty(argv, cols, rows, env, ROOT)
        self.shots: dict[str, list] = {}

    def wait(self, s: float):
        time.sleep(s)

    def ready(self, settle: float = 1.5):
        """Wait for the first full frame (the taskbar), then let windows settle."""
        if not self.wait_for("Start", 20):
            raise RuntimeError("texttop never drew its taskbar")
        time.sleep(settle)

    def key(self, *keys: str, pause: float = 0.25):
        for k in keys:
            self.pty.write(k)
            time.sleep(pause)

    def type(self, text: str, pause: float = 0.03):
        for ch in text:
            self.pty.write(ch)
            time.sleep(pause)

    def click(self, x: int, y: int, button: int = 0):
        self.pty.write(f"\x1b[<{button};{x + 1};{y + 1}M")
        time.sleep(0.04)
        self.pty.write(f"\x1b[<{button};{x + 1};{y + 1}m")
        time.sleep(0.25)

    def dclick(self, x: int, y: int):
        for _ in range(2):
            self.pty.write(f"\x1b[<0;{x + 1};{y + 1}M")
            time.sleep(0.03)
            self.pty.write(f"\x1b[<0;{x + 1};{y + 1}m")
            time.sleep(0.07)
        time.sleep(0.4)

    def find(self, needle: str, max_x: int | None = None, min_y: int = 0):
        for y, line in enumerate(self.pty.lines()):
            if y < min_y:
                continue
            x = line.find(needle)
            if x >= 0 and (max_x is None or x < max_x):
                return x, y
        return None

    def wait_for(self, needle: str, timeout: float = 15.0, **kw) -> bool:
        end = time.time() + timeout
        while time.time() < end:
            if self.find(needle, **kw):
                return True
            time.sleep(0.2)
        return False

    def open_icon(self, label: str, settle: float = 1.2):
        """Double-click a desktop icon by its label (icons sit left of x=30)."""
        pos = self.find(label, max_x=30, min_y=1)
        if not pos:
            raise RuntimeError(f"desktop icon {label!r} not found")
        self.dclick(pos[0] + len(label) // 2, pos[1] - 1)
        self.wait(settle)

    def click_text(self, text: str, button: int = 0, **kw):
        pos = self.find(text, **kw)
        if not pos:
            raise RuntimeError(f"{text!r} not on screen")
        self.click(pos[0] + 1, pos[1], button)

    def right_click(self, x: int, y: int):
        self.click(x, y, button=2)

    def menu(self, x: int, y: int, item: str):
        """Right-click at (x, y) and pick `item` from the context menu."""
        self.right_click(x, y)
        self.wait(0.4)
        self.click_text(item)
        self.wait(0.6)

    def title_row(self, title: str) -> int:
        """Row of the window title bar containing `title`: it opens with the
        ╭ corner and ends with ✕ (either may be hidden under another window)."""
        for y, line in enumerate(self.pty.lines()):
            i = line.find(title)
            if i >= 0 and ("✕" in line[i:] or "╭" in line[max(0, i - 6):i]):
                return y
        raise RuntimeError(f"no window titled {title!r}")

    def close_window(self, title: str):
        y = self.title_row(title)
        line = self.pty.lines()[y]
        self.click(line.find("✕", line.find(title)), y)
        self.wait(0.5)

    def window_menu(self, title: str, item: str):
        """Right-click a window's title bar and pick a menu item (Snap left, …)."""
        y = self.title_row(title)
        self.menu(self.pty.lines()[y].find(title) + 1, y, item)

    def taskbar_row(self) -> int:
        return len(self.pty.lines()) - 1

    def snap(self, name: str, settle: float = 0.8):
        time.sleep(settle)
        self.shots[name] = self.pty.buffer()

    def close(self):
        self.pty.close()


# ── SVG rendering ───────────────────────────────────────────────────────────

CW, CH = 9, 18  # cell size in px
FONT = 15
PAD, BAR = 18, 30  # frame padding, title bar height
DEFAULT_FG, DEFAULT_BG = "#d4d4dc", "#0b0d14"

ANSI = {
    "black": "#1c1c24", "red": "#e05561", "green": "#8cc265", "brown": "#d18f52",
    "yellow": "#d18f52", "blue": "#4aa5f0", "magenta": "#c162de", "cyan": "#42b3c2",
    "white": "#d4d4dc", "brightblack": "#5c6370", "brightred": "#ff616e",
    "brightgreen": "#a5e075", "brightbrown": "#f0a45d", "brightyellow": "#f0a45d",
    "brightblue": "#4dc4ff", "brightmagenta": "#de73ff", "brightcyan": "#4cd1e0",
    "brightwhite": "#ffffff",
}

# 2x2 quadrant glyphs: (top-left, top-right, bottom-left, bottom-right)
QUADS = {
    "▘": (1, 0, 0, 0), "▝": (0, 1, 0, 0), "▀": (1, 1, 0, 0), "▖": (0, 0, 1, 0),
    "▌": (1, 0, 1, 0), "▞": (0, 1, 1, 0), "▛": (1, 1, 1, 0), "▗": (0, 0, 0, 1),
    "▚": (1, 0, 0, 1), "▐": (0, 1, 0, 1), "▜": (1, 1, 0, 1), "▄": (0, 0, 1, 1),
    "▙": (1, 0, 1, 1), "▟": (0, 1, 1, 1), "█": (1, 1, 1, 1),
}
EIGHTHS = {"▁": 1, "▂": 2, "▃": 3, "▅": 5, "▆": 6, "▇": 7}

# box drawing: which edges a glyph reaches (up, down, left, right), rounded?
BOX = {
    "─": "lr", "━": "lr", "═": "lr", "│": "ud", "┃": "ud", "║": "ud",
    "┌": "dr", "┐": "dl", "└": "ur", "┘": "ul", "╭": "dr~", "╮": "dl~",
    "╰": "ur~", "╯": "ul~", "├": "udr", "┤": "udl", "┬": "dlr", "┴": "ulr",
    "┼": "udlr", "╔": "dr", "╗": "dl", "╚": "ur", "╝": "ul",
}


def color(c: str, default: str) -> str:
    if not c or c == "default":
        return default
    if re.fullmatch(r"[0-9a-fA-F]{6}", c):
        return "#" + c.lower()
    return ANSI.get(c, default)


def cell_colors(ch) -> tuple[str, str]:
    fg, bg = color(ch.fg, DEFAULT_FG), color(ch.bg, DEFAULT_BG)
    if ch.reverse:
        fg, bg = bg, fg
    return fg, bg


def box_path(x: float, y: float, spec: str) -> str:
    cx, cy = x + CW / 2, y + CH / 2
    ends = {"u": (cx, y), "d": (cx, y + CH), "l": (x, cy), "r": (x + CW, cy)}
    dirs = spec.rstrip("~")
    if spec.endswith("~") and len(dirs) == 2:
        (ax, ay), (bx, by) = ends[dirs[0]], ends[dirs[1]]
        return f"M{ax:.1f},{ay:.1f} Q{cx:.1f},{cy:.1f} {bx:.1f},{by:.1f}"
    return " ".join(f"M{cx:.1f},{cy:.1f} L{ex:.1f},{ey:.1f}" for ex, ey in (ends[d] for d in dirs))


def render_svg(grid, title: str) -> str:
    rows, cols = len(grid), len(grid[0])
    w, h = cols * CW, rows * CH
    W, H = w + PAD * 2, h + PAD * 2 + BAR
    ox, oy = PAD, PAD + BAR
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}">',
        "<defs><filter id='s' x='-5%' y='-5%' width='110%' height='115%'>"
        "<feDropShadow dx='0' dy='6' stdDeviation='8' flood-opacity='0.45'/></filter></defs>",
        f'<rect x="4" y="4" width="{W - 8}" height="{H - 8}" rx="12" fill="#1a1b26" filter="url(#s)"/>',
    ]
    for i, c in enumerate(("#ff5f57", "#febc2e", "#28c840")):
        out.append(f'<circle cx="{PAD + 8 + i * 20}" cy="{PAD + 9}" r="6" fill="{c}"/>')
    out.append(
        f'<text x="{W / 2}" y="{PAD + 14}" fill="#8a8fa8" font-size="13" text-anchor="middle" '
        f'font-family="system-ui,-apple-system,Segoe UI,sans-serif">{html.escape(title)}</text>'
    )
    out.append(f'<g transform="translate({ox},{oy})" shape-rendering="crispEdges">')
    out.append(f'<rect width="{w}" height="{h}" fill="{DEFAULT_BG}"/>')

    shapes, lines, texts = [], [], []
    for y, row in enumerate(grid):
        py = y * CH
        # background runs
        x = 0
        while x < cols:
            _, bg = cell_colors(row[x])
            x2 = x + 1
            while x2 < cols and cell_colors(row[x2])[1] == bg:
                x2 += 1
            if bg != DEFAULT_BG:
                shapes.append(f'<rect x="{x * CW}" y="{py}" width="{(x2 - x) * CW}" height="{CH}" fill="{bg}"/>')
            x = x2
        # glyphs
        run, run_x, run_style = [], 0, None

        def flush():
            nonlocal run
            if not run:
                return
            fg, bold = run_style
            weight = ' font-weight="bold"' if bold else ""
            # One <text> per word, pinned to its cell: fonts disagree on the
            # width of a space, so never let a viewer lay out whitespace.
            cell = run_x
            word, word_x = [], cell
            for c in run + [" "]:
                if c == " ":
                    if word:
                        n = sum(2 if is_wide(w) else 1 for w in word)
                        texts.append(
                            f'<text x="{word_x * CW}" y="{py + CH * 0.76:.1f}" fill="{fg}"{weight} '
                            f'textLength="{n * CW}" lengthAdjust="spacingAndGlyphs">'
                            f"{html.escape(''.join(word))}</text>"
                        )
                        word = []
                    cell += 1
                    word_x = cell
                else:
                    word.append(c)
                    cell += 2 if is_wide(c) else 1
            run = []

        x = 0
        while x < cols:
            ch = row[x]
            data = ch.data
            fg, bg = cell_colors(ch)
            px = x * CW
            wide = x + 1 < cols and row[x + 1].data == ""
            if data in QUADS:
                flush()
                q = QUADS[data]
                hw, hh = CW / 2, CH / 2
                for i, on in enumerate(q):
                    if on:
                        shapes.append(
                            f'<rect x="{px + (i % 2) * hw:.1f}" y="{py + (i // 2) * hh:.1f}" '
                            f'width="{hw:.1f}" height="{hh:.1f}" fill="{fg}"/>'
                        )
            elif data in EIGHTHS:
                flush()
                eh = CH * EIGHTHS[data] / 8
                shapes.append(f'<rect x="{px}" y="{py + CH - eh:.1f}" width="{CW}" height="{eh:.1f}" fill="{fg}"/>')
            elif data in BOX:
                flush()
                width = 2.2 if data in "━┃═║" else 1.3
                lines.append(
                    f'<path d="{box_path(px, py, BOX[data])}" stroke="{fg}" stroke-width="{width}" fill="none"/>'
                )
            elif data == "":
                pass  # tail of a wide glyph
            else:
                style = (fg, bool(ch.bold))
                if run and (style != run_style or wide):
                    flush()
                if not run:
                    run_x, run_style = x, style
                run.append(data)
                if wide:
                    flush()
                    x += 1
            x += 1
        flush()

    out += shapes
    out.append('</g>')
    out.append(f'<g transform="translate({ox},{oy})" stroke-linecap="square">')
    out += lines
    out.append("</g>")
    out.append(
        f'<g transform="translate({ox},{oy})" font-size="{FONT}" xml:space="preserve" '
        'font-family="\'Cascadia Mono\',\'JetBrains Mono\',Menlo,Consolas,\'DejaVu Sans Mono\',monospace">'
    )
    out += texts
    out.append("</g></svg>")
    return "\n".join(out)


def is_wide(c: str) -> bool:
    import unicodedata

    return len(c) == 1 and unicodedata.east_asian_width(c) in "WF"


# ── fixtures ────────────────────────────────────────────────────────────────


def write_png(path: Path, w: int, h: int, pixel: Callable[[int, int], tuple[int, int, int]]):
    raw = b"".join(b"\x00" + bytes(v for x in range(w) for v in pixel(x, y)) for y in range(h))

    def chunk(tag, data):
        c = tag + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    path.write_bytes(png)


def plasma(x: int, y: int) -> tuple[int, int, int]:
    """A synthwave sunset: gradient sky, striped sun, perspective grid."""
    import math

    w, h = 480, 300
    horizon = h * 0.62
    if y < horizon:
        t = y / horizon
        r, g, b = int(40 + 200 * t), int(10 + 40 * t), int(90 + 60 * (1 - t))
        dx, dy = x - w / 2, y - horizon * 0.78
        if dx * dx + dy * dy < (h * 0.30) ** 2 and not (y > horizon * 0.62 and int(y) % 12 < 3):
            k = (y - (horizon * 0.78 - h * 0.30)) / (h * 0.6)
            return 255, int(230 - 160 * k), int(90 - 60 * k)
        return r, g, b
    d = (y - horizon) / (h - horizon)
    persp = (x - w / 2) / max(d, 0.02)
    grid_x = abs((persp / 60.0) % 1 - 0.5) < 0.04 + 0.05 * d
    grid_y = abs(math.log(max(d, 1e-3)) * 6 % 1 - 0.5) < 0.12
    if grid_x or grid_y:
        return 255, int(60 + 120 * d), 230
    return 20, 8, int(40 + 30 * d)


BLANK_STUDIO = '{"bpm":120,"tracks":[{"name":"Drums","kind":"drums"}],"clips":[],"session":[]}'

# ── scenes ──────────────────────────────────────────────────────────────────


@dataclass
class Scene:
    name: str
    title: str
    run: Callable[[Session], None] | None = None
    arg: Callable[[Path], Path | None] = lambda sandbox: None
    shots: list[str] = field(default_factory=list)  # extra shots taken by `run`
    # Opt-in scenes (reveal machine details: user, host, paths, PATH) run
    # only when named with --only.
    optional: bool = False


def s_desktop(s: Session):
    s.ready()
    s.snap("desktop")
    s.key(F1)
    s.snap("start_menu")


def s_files(s: Session):
    s.ready()
    s.key(F11, DOWN, DOWN, DOWN)
    s.snap("files")


def s_editor(s: Session):
    s.ready()
    s.key(F11)
    s.key(*[DOWN] * 30, pause=0.02)
    s.snap("editor")


def s_git(s: Session):
    s.ready()
    s.open_icon("Git", settle=3)
    s.key(F11)
    s.snap("git", settle=2)


def s_terminal(s: Session):
    s.ready()
    s.open_icon("Terminal", settle=3)
    s.key(F11)
    s.wait(1.5)
    # Keystrokes sent before the shell draws its prompt are dropped.
    end = time.time() + 25
    while time.time() < end and not any(s.find(p, min_y=1) for p in ("PS ", "$ ", "# ", "% ")):
        time.sleep(0.3)
    s.wait(1)
    # The shell starts in the home folder; show the project instead.
    s.type(f'cd "{ROOT}"; clear; git log --oneline --graph -12\r')
    s.snap("terminal", settle=2.5)


def s_ide(s: Session):
    s.ready()
    # The project is open in Files: start the IDE there with Run app here.
    pos = s.find("Cargo.toml", min_y=1)
    if not pos:
        raise RuntimeError("Files did not list the project")
    s.menu(pos[0] + 2, pos[1], "Run app here")
    s.click_text("🛠  IDE")
    s.wait(3)
    s.key(F11)
    s.wait(1.5)
    readme = s.find("README.md", min_y=2)
    if readme:
        s.dclick(readme[0] + 2, readme[1])
    s.snap("ide", settle=2.5)


def s_image(s: Session):
    s.ready()
    s.key(F11)
    s.snap("image", settle=1.5)


def s_studio(s: Session):
    s.ready()
    s.key(F11, "t")
    s.key("1", pause=0.6)  # House template
    s.key(ENTER, RIGHT, ENTER, RIGHT, ENTER, RIGHT, ENTER, pause=0.3)
    s.snap("studio_session", settle=3)
    s.key(LEFT, "3")
    s.snap("studio_piano", settle=1.5)
    s.key(ESC, "r", "l")  # record the scene into the arrangement
    s.wait(9)
    s.key("r", "2")
    s.snap("studio_arrange", settle=1.5)


def s_web(s: Session):
    s.ready()
    s.open_icon("Web", settle=2)
    s.key(F11)
    s.wait(14)  # webview engine cold start
    s.snap("web", settle=1)


def s_wikipedia(s: Session):
    s.ready()
    s.open_icon("Web", settle=2)
    s.key(F11)
    s.wait(10)  # webview engine cold start
    s.key(F4)
    s.type(WIKI_URL + "\r")
    # Wait for the article text (drawn as terminal characters), then images.
    if not s.wait_for("Aurora", 40, min_y=3):
        raise RuntimeError("Wikipedia did not load (offline?)")
    s.snap("wikipedia", settle=6)
    s.key(F3)  # text overlay off: the raw pixel frame
    s.snap("wikipedia_pixels", settle=3)


def s_tiling(s: Session):
    s.ready()
    s.close_window("Welcome")
    for icon in ("Snake", "Calendar", "Clock", "2048", "Mines", "Calc"):
        s.open_icon(icon, settle=1.5)
    s.menu(2, s.taskbar_row(), "Tile windows")  # Start button's context menu
    s.snap("tiling", settle=1.5)
    s.key(F6)
    s.snap("switcher")


def s_snap(s: Session):
    s.ready()
    s.close_window("Welcome")
    s.open_icon("Git", settle=3)
    # Git is on top: snap it first, or Files' left half would cover its title.
    s.window_menu("Git ·", "Snap right")
    s.window_menu(str(ROOT), "Snap left")
    s.snap("snap", settle=1.5)


def s_menus(s: Session):
    s.ready()
    s.close_window("Welcome")
    s.right_click(70, 12)
    s.snap("context_menu")
    s.key(ESC, F12)
    s.snap("shortcuts")


def s_wallpaper(s: Session):
    s.ready()
    s.key(F1)
    s.wait(0.6)
    s.click_text("Wallpaper")
    s.snap("wallpaper", settle=1.5)


def s_tools(s: Session):
    s.ready()
    for icon in ("Clock", "Calendar", "Calc"):
        s.open_icon(icon)
    s.type("(2+3)^4/5")
    s.key(ENTER)
    s.snap("tools")


def s_games(s: Session):
    s.ready()
    s.open_icon("Mines")
    s.open_icon("2048")
    s.key(LEFT, UP, RIGHT, DOWN, LEFT, UP, pause=0.15)
    s.snap("games")


def s_system(s: Session):
    s.ready()
    s.open_icon("System", settle=2)
    s.key(F11)
    s.snap("system")


WIKI_URL = "https://en.wikipedia.org/wiki/Aurora"


def private_browser(sandbox: Path) -> None:
    """Browse in a private window so the visit leaves no cookies behind."""
    d = sandbox / ".texttop_browser"
    d.mkdir(exist_ok=True)
    (d / "settings.json").write_text(
        '{"private": true, "accept_cookies": true, "clear_on_exit": false}'
    )
    return None


def make_studio(sandbox: Path) -> Path:
    p = sandbox / "song.ttstudio"
    p.write_text(BLANK_STUDIO)
    return p


def make_image(sandbox: Path) -> Path:
    d = sandbox / "pictures"
    d.mkdir(exist_ok=True)
    p = d / "sunset.png"
    write_png(p, 480, 300, plasma)
    return p


SCENES = [
    Scene("desktop", "texttop — desktop", s_desktop, shots=["desktop", "start_menu"]),
    Scene("files", "texttop — Files", s_files, lambda sb: ROOT),
    Scene("editor", "texttop — Editor", s_editor, lambda sb: ROOT / "src" / "gfx.rs"),
    Scene("git", "texttop — Git", s_git, lambda sb: ROOT),
    Scene("terminal", "texttop — Terminal", s_terminal, lambda sb: ROOT),
    Scene("ide", "texttop — IDE app group", s_ide, lambda sb: ROOT),
    Scene("image", "texttop — Images", s_image, make_image),
    Scene("studio", "texttop — Studio", s_studio, make_studio,
          shots=["studio_session", "studio_piano", "studio_arrange"]),
    Scene("web", "texttop — Web", s_web, private_browser),
    Scene("wikipedia", "texttop — Web · Wikipedia", s_wikipedia, private_browser,
          shots=["wikipedia", "wikipedia_pixels"]),
    Scene("tiling", "texttop — tiled windows", s_tiling, shots=["tiling", "switcher"]),
    Scene("snap", "texttop — snapped side by side", s_snap, lambda sb: ROOT),
    Scene("menus", "texttop — context menu", s_menus, shots=["context_menu", "shortcuts"]),
    Scene("wallpaper", "texttop — Wallpaper", s_wallpaper),
    Scene("tools", "texttop — Clock · Calendar · Calculator", s_tools),
    Scene("games", "texttop — Games", s_games),
    Scene("system", "texttop — System", s_system, optional=True),
]

TITLES = {
    "desktop": "texttop — desktop",
    "start_menu": "texttop — start menu",
    "studio_session": "texttop — Studio · session",
    "studio_piano": "texttop — Studio · piano roll",
    "studio_arrange": "texttop — Studio · arrangement",
    "wikipedia_pixels": "texttop — Web · pixels only (F3)",
    "switcher": "texttop — window switcher (F6)",
    "shortcuts": "texttop — shortcut overlay (F12)",
}

# ── documentation ───────────────────────────────────────────────────────────

# Handbook chapters: (anchor, heading, [screenshots], body markdown)
CHAPTERS = [
    ("desktop", "The desktop", ["desktop", "start_menu"], """
texttop is a windowed desktop that runs in any truecolor terminal with mouse
support. Windows can be dragged by their title bars, resized from the `◢`
corner, snapped to screen edges and maximized. The taskbar shows open windows,
the git branch of the focused workspace, and the clock.

| Keys | Action |
|---|---|
| F1 | start menu |
| F6 · Alt+Tab · Ctrl+Tab | window switcher |
| F11 · Ctrl+Shift+↑ | maximize / restore |
| Ctrl+Shift+← / → | snap left / right |
| Ctrl+W · Ctrl+Q | close window · quit |
| Ctrl+D | show desktop |
| F12 | shortcut overlay |

Right-click anything — desktop, icons, title bars, taskbar, files — for a
context menu. Drag files between windows, onto the desktop (shortcut) or onto
an app to open them there; hold Shift to move instead of copy. Open windows are
restored on the next start.
"""),
    ("windows", "Window management", ["tiling", "snap", "switcher", "context_menu", "shortcuts"], """
Any number of windows can share the screen.

- **Tile** — right-click the Start button (or the desktop) → *Tile windows*
  lays every open window out in a grid; *Cascade windows* stacks them again.
- **Snap** — drag a title bar against the left or right screen edge, press
  Ctrl+Shift+← / →, or right-click a title bar or taskbar button → *Snap left* /
  *Snap right*. Dragging a snapped window away restores its old size.
- **Maximize** — F11, double-click the title, drag it to the top edge, or □.
- **Switch** — F6, Alt+Tab or Ctrl+Tab opens the switcher; the taskbar shows
  every window (middle-click closes, **»N** lists the ones that don't fit).
- **Menus** — right-click the desktop, icons, title bars, the taskbar and files.
- **F12** shows every desktop shortcut at a glance.
"""),
    ("files", "Files", ["files"], """
A full file manager with git status badges, multi-select, previews and
bookmarks. The top bar has back / forward / up / home / refresh, a clickable
path and toggles for hidden files and the preview pane; click a column heading
to sort. The bottom bar holds the everyday actions: **+ Folder**, **+ File**,
copy ⧉ / cut ✂ / paste 📋, **Rename**, **Delete** (with a confirm button),
**⋯ More** and **🔍 Filter**. Click the ○ left of a name to tick several files.
Right-click a file for open-with, hex view, snapshots, compare, pin and git
actions. **Run app here…** lists every built-in app plus your own desktop and
taskbar apps: pick one to start it in the current folder, or to open the
selected files with it (untick **Open selected files** to just use the folder).
The list follows the folder live: files created, renamed or deleted elsewhere
show up on their own.
"""),
    ("editor", "Editor", ["editor"], """
A text editor with line numbers and syntax highlighting for Rust, Python,
JS/TS, C/C++, Go, Java, JSON, TOML, YAML, HTML, CSS, SQL, shell, Markdown and
more. **File** (new, open, save, save as…), **Edit** (cut, copy, paste, select
all) and **View** (syntax colors) menus sit on the top bar next to 💾 Save and
the clipboard buttons; right-click the text for the same edit menu. Drag to
select. Save as… asks for the path in a field with Save / Cancel buttons.
"""),
    ("git", "Git", ["git"], """
Status, log, diff, branches and commit through the system `git`. Tabs along
the top switch panes; the second row has the actions for the current pane —
**Stage / Unstage**, **Stage all**, **Diff**, **Open**, **Commit…**,
**Show commit**, **Checkout**, **New branch…**. Tick the box next to a file to
stage it, double-click to open it, right-click for its menu. **📂 Repo…**
switches repository; the commit message is typed into a field with a
✔ Commit button.
"""),
    ("terminal", "Terminal", ["terminal"], """
Your real shell in a window, with a full VT emulator and system clipboard
copy/paste. Everything you type goes to the shell. Drag to select (copied on
release), right-click for Copy / Paste. When the shell exits, a **Close**
button appears.
"""),
    ("ide", "IDE, app groups & Contribute", ["ide"], """
An **app group** is a virtual app made of several frames: built-in apps docked
left, right, top or bottom around a tab area, all started in one folder. App
groups are listed in the start menu and in **Run app here…** like any other
app, and starting one somewhere hands that folder to every frame.

The **IDE** is the built-in group: a file explorer on the left, a terminal at
the bottom, and everything you open from inside it (a file in the editor, an
image, a video, another terminal) lands in a tab. Its top bar shows the group's
name and folder, **📂 Open folder/file** to move it to another folder or open a
file, and **⚙ Settings**. Drag a frame's edge to resize it.

**🧱 App Groups** (in Settings) edits groups: name, icon, start-menu section,
whether opened apps become tabs, and the list of frames. Each frame has an app,
where it docks, a size, and an optional parameter (a subfolder, a file, or the
command a terminal runs). A sketch shows the arrangement as you go. **▶ Try
it** saves and opens the group.

**🤝 Contribute / Customize** downloads texttop's own source code with git
into texttop's data folder and opens it in the IDE, ready for
`cargo run --release`. **⟳ Get latest** updates the copy later.
"""),
    ("images", "Images", ["image"], """
Images are drawn with quadrant pixels: every cell carries a 2×2 pixel pattern
with a best-fit pair of truecolor colors. The toolbar browses the folder
(◀ ▶), zooms (− · Fit · 100% · +), rotates and flips, runs a slideshow, shows
image info and sets the picture as wallpaper. Scroll to zoom at the pointer,
drag to pan, double-click to toggle fit / 100%.
"""),
    ("studio", "Studio", ["studio_session", "studio_piano", "studio_arrange"], """
An electronic music studio: pick a genre template, launch clips in the session
grid, edit notes in the piano roll, lay out a song in the arrangement and
export a WAV. Drop mp3/wav/flac/ogg files onto the window to use samples.

The header switches between **Session**, **Arrange** and **Piano** and shows
buttons for whatever is selected: **Launch**, **Stop clip**, **New clip**,
**Duplicate**, **Clear**, **Scene**, **+ Track**, **Mute**, **🗑 Track** in the
session; **Place clip**, **Shorter / Longer**, **Remove** in the arrangement;
note length, octave and loop in the piano roll. The transport row plays, stops,
records, changes tempo, saves and exports.

**Session** — click a clip to launch it (again stops), click ▶S*n* to launch a
scene. Click a browser preset to arm it, then click empty cells to fill them.

**Piano roll** — click toggles a note, drag stretches it, the wheel scrolls
pitch.

**Arrangement** — click a lane to place the selected clip, drag to move,
right-click removes. Playback loops the song.

**Mixer** — click M / S, click or scroll the volume bar, scroll a track name
to pan, click Flt · Dly · Rvb · Pmp to toggle effects (scroll Flt for cutoff).
"""),
    ("web", "Web", ["wikipedia", "wikipedia_pixels", "web"], """
A real browser engine (WebView2 / WebKitGTK / WKWebView) rendered with hybrid
graphics: images and layout as pixels, text as crisp terminal characters. The
first picture is Wikipedia as you see it; the second is the same page with the
text overlay switched off — the raw pixel frame the engine captures, where
small text would be unreadable. Links, forms, scrolling and video work.

The toolbar has back / forward / reload, the address bar (click to type),
☆ bookmark, History, Bookmarks, Cookies and Privacy. The status bar toggles
**Aa Crisp text** and zooms (− / +).
"""),
    ("wallpaper", "Wallpaper", ["wallpaper"], """
Gradient themes for the desktop background — click a tile to try it — or any
picture via **🖼 Image…** (or **🖼 Wallpaper** in the image viewer).
"""),
    ("tools", "Clock, Calendar & Calculator", ["tools"], """
| App | How |
|---|---|
| Clock | tabs for clock / stopwatch / timer · big Start / Pause and Reset buttons · timer presets and ±1m / ±30s |
| Calendar | ◀◀ ◀ ▶ ▶▶ step months and years · click a day · Today · the wheel flips months |
| Calculator | click the keys or type an expression (`+ − * / ^ %`, parentheses) |
"""),
    ("games", "Games", ["games"], """
| Game | How |
|---|---|
| Snake | Start / Pause and New game on the toolbar · click beside the head (or ◀ ▲ ▼ ▶) to turn |
| Minesweeper | click reveals · right-click flags · **⚑ Flag mode** makes clicks plant flags · easy / medium / hard |
| 2048 | swipe the board with the mouse or click the arrows around it · New game |
"""),
    ("system", "System", ["system"], """
Operating system and CPU details, special folders and an environment snapshot.
**📋 Copy report** puts it all on the clipboard.
"""),
]

GALLERY = [
    ("desktop", "Desktop"),
    ("wikipedia", "Web — Wikipedia, text drawn as terminal characters"),
    ("tiling", "Tiled windows"),
    ("studio_session", "Studio"),
    ("snap", "Snapped side by side"),
    ("files", "Files"),
    ("image", "Images"),
    ("editor", "Editor"),
    ("studio_piano", "Piano roll"),
    ("git", "Git"),
    ("terminal", "Terminal"),
    ("tools", "Clock · Calendar · Calc"),
    ("context_menu", "Context menus"),
]


def shot_exists(name: str) -> bool:
    return (SHOTS / f"{name}.svg").exists()


def write_handbook():
    parts = [
        "# texttop handbook",
        "",
        "_Generated by `scripts/screenshots.py` — re-run it to refresh the pictures._",
        "",
        "## Contents",
        "",
    ]
    chapters = [c for c in CHAPTERS if any(shot_exists(s) for s in c[2]) or c[0] == "desktop"]
    for anchor, heading, _, _ in chapters:
        parts.append(f"- [{heading}](#{anchor})")
    parts.append("")
    for anchor, heading, shots, body in chapters:
        parts.append(f'<a id="{anchor}"></a>')
        parts.append(f"## {heading}")
        parts.append("")
        for s in shots:
            if shot_exists(s):
                parts.append(f"![{heading}](screenshots/{s}.svg)")
                parts.append("")
        parts.append(body.strip())
        parts.append("")
    (DOCS / "HANDBOOK.md").write_text("\n".join(parts), encoding="utf-8")


START, END = "<!-- screenshots:start -->", "<!-- screenshots:end -->"


def write_readme_gallery():
    items = [(s, cap) for s, cap in GALLERY if shot_exists(s)]
    if not items:
        return
    anchors = {s: a for a, _, shots, _ in CHAPTERS for s in shots}
    lines = [START, "", "## Screenshots", ""]
    first, rest = items[0], items[1:]
    lines.append(f'<p align="center"><img src="docs/screenshots/{first[0]}.svg" alt="{first[1]}" width="100%"></p>')
    lines.append("")
    lines.append("<table>")
    for i in range(0, len(rest), 2):
        lines.append("<tr>")
        for s, cap in rest[i : i + 2]:
            link = f"docs/HANDBOOK.md#{anchors.get(s, '')}"
            lines.append(
                f'<td width="50%" align="center"><a href="{link}">'
                f'<img src="docs/screenshots/{s}.svg" alt="{cap}"></a><br><sub>{cap}</sub></td>'
            )
        lines.append("</tr>")
    lines.append("</table>")
    lines.append("")
    lines.append("More in the **[handbook](docs/HANDBOOK.md)** — every app, pictured and explained.")
    lines.append("")
    lines.append(END)
    block = "\n".join(lines)

    readme = ROOT / "README.md"
    text = readme.read_text(encoding="utf-8")
    nl = "\r\n" if "\r\n" in text else "\n"
    text = text.replace("\r\n", "\n")
    if START in text and END in text:
        text = re.sub(re.escape(START) + r".*?" + re.escape(END), lambda _: block, text, flags=re.S)
    elif "\n## Building" in text:
        text = text.replace("\n## Building", "\n" + block + "\n\n## Building", 1)
    else:
        text = text.rstrip("\n") + "\n\n" + block + "\n"
    readme.write_text(text.replace("\n", nl), encoding="utf-8", newline="")


# ── main ────────────────────────────────────────────────────────────────────


def find_exe(explicit: str | None, build: bool) -> Path:
    if explicit:
        return Path(explicit)
    exe = ROOT / "target" / "release" / ("texttop.exe" if IS_WIN else "texttop")
    if build:
        cargo = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo")
        print("building release binary…")
        subprocess.run([cargo, "build", "--release"], cwd=ROOT, check=True)
    if not exe.exists():
        sys.exit(f"{exe} not found — build it or pass --exe")
    return exe


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", help="texttop binary (default: target/release, built first)")
    ap.add_argument("--no-build", action="store_true", help="don't run cargo build --release")
    ap.add_argument("--only", help="comma-separated scene names")
    ap.add_argument("--skip", help="comma-separated scene names to skip")
    ap.add_argument("--cols", type=int, default=140)
    ap.add_argument("--rows", type=int, default=42)
    ap.add_argument("--docs-only", action="store_true", help="only regenerate markdown")
    ap.add_argument("--list", action="store_true", help="list scenes and exit")
    args = ap.parse_args()

    if args.list:
        for sc in SCENES:
            opt = "  (opt-in: --only " + sc.name + ")" if sc.optional else ""
            print(f"{sc.name:10} → {', '.join(sc.shots or [sc.name])}{opt}")
        return

    if not args.docs_only:
        try:
            import pyte  # noqa: F401

            if IS_WIN:
                import winpty  # noqa: F401
        except ImportError as e:
            sys.exit(f"missing dependency: {e.name} — pip install pyte" + (" pywinpty" if IS_WIN else ""))
        exe = find_exe(args.exe, build=not args.no_build and not args.exe)
        only = set(filter(None, (args.only or "").split(",")))
        skip = set(filter(None, (args.skip or "").split(",")))
        SHOTS.mkdir(parents=True, exist_ok=True)
        failed = []
        for sc in SCENES:
            if (only and sc.name not in only) or sc.name in skip or (sc.optional and not only):
                continue
            print(f"• {sc.name} …", flush=True)
            with tempfile.TemporaryDirectory(prefix="texttop-shots-", ignore_cleanup_errors=True) as tmp:
                sandbox = Path(tmp)
                sess = Session(exe, args.cols, args.rows, sc.arg(sandbox), sandbox)
                try:
                    sc.run(sess)
                except Exception as e:  # keep going; report at the end
                    failed.append(f"{sc.name}: {e}")
                    print(f"  ✗ {e}")
                finally:
                    sess.close()
                    time.sleep(0.5)  # let the process release the sandbox
                for name, grid in sess.shots.items():
                    svg = render_svg(grid, TITLES.get(name, sc.title))
                    (SHOTS / f"{name}.svg").write_text(svg, encoding="utf-8")
                    print(f"  ✓ docs/screenshots/{name}.svg")
        if failed:
            print("\nsome scenes failed:\n  " + "\n  ".join(failed))

    DOCS.mkdir(exist_ok=True)
    write_handbook()
    write_readme_gallery()
    print("✓ docs/HANDBOOK.md and README.md gallery updated")


if __name__ == "__main__":
    main()
