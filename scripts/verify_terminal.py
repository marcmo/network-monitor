"""Run the application in a real PTY and collect local terminal/resource evidence.

Requires pyte and Pillow in the Python environment. Output can contain location
coordinates, so keep artifacts in a local temporary or ignored directory.
"""

import argparse
import ctypes
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time

from PIL import Image, ImageDraw, ImageFont
import pyte


class Usage(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_ubyte * 16)] + [
        (key, ctypes.c_uint64)
        for key in (
            "user_ns", "system_ns", "package_idle_wakeups", "interrupt_wakeups",
            "pageins", "wired_bytes", "resident_bytes", "footprint_bytes", "start", "exit",
        )
    ]


class Timebase(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]


LIBPROC = ctypes.CDLL("/usr/lib/libproc.dylib")
TIMEBASE = Timebase()
ctypes.CDLL("/usr/lib/libSystem.B.dylib").mach_timebase_info(ctypes.byref(TIMEBASE))
# CPU counters use Mach ticks on this Mac; a calibrated busy loop verified conversion.
CPU_TO_NS = TIMEBASE.numer / TIMEBASE.denom


def usage(pid):
    value = Usage()
    if LIBPROC.proc_pid_rusage(pid, 0, ctypes.byref(value)):
        return None
    result = {
        name: getattr(value, name)
        for name in (
            "user_ns", "system_ns", "package_idle_wakeups", "interrupt_wakeups",
            "resident_bytes", "footprint_bytes",
        )
    }
    result["user_ns"] *= CPU_TO_NS
    result["system_ns"] *= CPU_TO_NS
    return result


def save_screen(screen, output):
    (output / "screen.txt").write_text("\n".join(screen.display))
    font = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 14)
    canvas = Image.new("RGB", (screen.columns * 9 + 24, screen.lines * 19 + 24), "#101820")
    draw = ImageDraw.Draw(canvas)
    palette = {
        "default": "#e1e8ed", "black": "#101820", "red": "#ff6973",
        "green": "#80d994", "brown": "#e7c27a", "yellow": "#e7c27a",
        "blue": "#6aa9ff", "magenta": "#d99bea", "cyan": "#7ddcdd",
        "white": "#e1e8ed", "brightblack": "#8a949f",
    }
    for y in range(screen.lines):
        for x in range(screen.columns):
            cell = screen.buffer[y][x]
            color = palette.get(cell.fg, "#e1e8ed")
            if len(cell.fg) == 6:
                color = "#" + cell.fg
            draw.text((12 + x * 9, 12 + y * 19), cell.data, font=font, fill=color)
    canvas.save(output / "screen.png")


def summarize(samples):
    steady = [sample for sample in samples if sample["elapsed_s"] >= 10]
    if len(steady) < 2:
        return {}
    first, last = steady[0], steady[-1]
    duration = last["elapsed_s"] - first["elapsed_s"]
    cpu_ns = last["user_ns"] + last["system_ns"] - first["user_ns"] - first["system_ns"]
    return {
        "steady_duration_s": duration,
        "cpu_one_core_percent": cpu_ns / 1e9 / duration * 100,
        "rss_mean_mib": sum(s["resident_bytes"] for s in steady) / len(steady) / 2**20,
        "rss_max_mib": max(s["resident_bytes"] for s in steady) / 2**20,
        "footprint_max_mib": max(s["footprint_bytes"] for s in steady) / 2**20,
        "interrupt_wakeups_per_s": (last["interrupt_wakeups"] - first["interrupt_wakeups"]) / duration,
        "package_idle_wakeups_per_s": (last["package_idle_wakeups"] - first["package_idle_wakeups"]) / duration,
    }



WRAPPER = """
import json, subprocess, sys, termios
from pathlib import Path
pid_path, state_path, *command = sys.argv[1:]
application = subprocess.Popen(command)
temporary = Path(pid_path + '.tmp')
temporary.write_text(str(application.pid))
temporary.replace(pid_path)
code = application.wait()
attributes = termios.tcgetattr(0)
attributes[3] &= ~getattr(termios, "PENDIN", 0)
attributes[6] = [item.hex() if isinstance(item, bytes) else item for item in attributes[6]]
Path(state_path).write_text(json.dumps({'exit_code': code, 'attributes': attributes}))
"""


def serial_attributes(attributes):
    # macOS sets PENDIN while restoring; it is a transient input retype flag.
    attributes[3] &= ~getattr(termios, "PENDIN", 0)
    attributes[6] = [item.hex() if isinstance(item, bytes) else item for item in attributes[6]]
    return attributes

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--duration", type=float, default=20)
    parser.add_argument("--startup-timeout", type=float, default=180)
    parser.add_argument("--output", required=True)
    parser.add_argument("--quit", choices=["q", "ctrl-c", "SIGINT", "SIGTERM", "SIGHUP"], default="q")
    parser.add_argument("--resize", action="store_true")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command or args.duration <= 0 or args.startup_timeout <= 0:
        parser.error("a command and positive duration/startup timeout are required")
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 34, 120, 0, 0))
    initial = serial_attributes(termios.tcgetattr(slave))
    (output / "terminal-initial.json").write_text(json.dumps(initial))
    env = dict(os.environ, TERM="xterm-256color", LANG="en_US.UTF-8")
    # The tool environment disables colors; exercise the normal color terminal path.
    env.pop("NO_COLOR", None)

    def child_session():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    pid_path = output / "pid"
    state_path = output / "terminal-state.json"
    active_path = output / "active.json"
    pid_path.unlink(missing_ok=True)
    state_path.unlink(missing_ok=True)
    active_path.unlink(missing_ok=True)
    process = subprocess.Popen(
        [sys.executable, "-c", WRAPPER, str(pid_path), str(state_path), *command],
        stdin=slave, stdout=slave, stderr=slave, env=env, preexec_fn=child_session,
    )
    deadline = time.monotonic() + 5
    while not pid_path.exists() and time.monotonic() < deadline:
        time.sleep(0.01)
    if not pid_path.exists():
        process.kill()
        raise RuntimeError("PTY wrapper did not start the application")
    application_pid = int(pid_path.read_text())
    screen = pyte.Screen(120, 34)
    screen.write_process_input = lambda text: os.write(master, text.encode("ascii"))
    stream = pyte.Stream(screen)
    decoder = codecs.getincrementaldecoder("utf8")("replace")
    spawned = time.monotonic()
    started = None
    samples = []
    raw = bytearray()
    resized = False

    def receive(timeout):
        ready, _, _ = select.select([master], [], [], timeout)
        if ready:
            try:
                chunk = os.read(master, 262144)
            except OSError:
                return
            raw.extend(chunk)
            stream.feed(decoder.decode(chunk))

    try:
        while process.poll() is None:
            receive(0.5)
            now = time.monotonic()
            if started is None:
                if any(" Recording " in line and "display 1Hz" in line for line in screen.display):
                    started = now
                    active_path.write_text(json.dumps({"pid": application_pid, "monotonic_s": started, "startup_s": started - spawned}))
                elif now - spawned >= args.startup_timeout:
                    raise RuntimeError("application did not render a recording before startup timeout")
                else:
                    continue
            if now - started >= args.duration:
                break
            value = usage(application_pid)
            if value:
                value["elapsed_s"] = now - started
                samples.append(value)
            if args.resize and not resized and now - started > args.duration / 2:
                (output / "before-resize.txt").write_text("\n".join(screen.display))
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
                screen.resize(24, 80)
                os.kill(application_pid, signal.SIGWINCH)
                resized = True
        save_screen(screen, output)
        quit_start = time.monotonic()
        if process.poll() is None:
            if args.quit.startswith("SIG"):
                os.kill(application_pid, getattr(signal, args.quit))
            else:
                os.write(master, b"q" if args.quit == "q" else b"\x03")
        deadline = time.monotonic() + 12
        while process.poll() is None and time.monotonic() < deadline:
            receive(0.1)
        if process.poll() is None:
            os.kill(application_pid, signal.SIGKILL)
        process.wait()
        receive(0.1)
        terminal_state = json.loads(state_path.read_text())
        code = terminal_state["exit_code"]
        final = terminal_state["attributes"]
        (output / "terminal.bin").write_bytes(raw)
        (output / "after-quit.txt").write_text("\n".join(screen.display))
        summary = {
            "command": command, "duration_s": time.monotonic() - spawned,
            "startup_s": started - spawned if started is not None else None,
            "active_duration_s": quit_start - started if started is not None else 0,
            "exit_code": code, "quit_latency_s": time.monotonic() - quit_start,
            "termios_restored": initial == final,
            "alternate_screen_left": b"\x1b[?1049l" in raw,
            "samples": len(samples), "cpu_timebase_numer": TIMEBASE.numer,
            "cpu_timebase_denom": TIMEBASE.denom,
            **summarize(samples),
        }
        (output / "usage.json").write_text(json.dumps({"summary": summary, "samples": samples}, indent=2))
        print(json.dumps(summary, indent=2))
    finally:
        if process.poll() is None:
            try:
                os.kill(application_pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=12)
            except subprocess.TimeoutExpired:
                try:
                    os.kill(application_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        os.close(master)
        os.close(slave)


if __name__ == "__main__":
    main()
