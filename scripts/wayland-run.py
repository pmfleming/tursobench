#!/usr/bin/env python3
"""Run a desktop check under isolated headless Weston, with no X server."""
import argparse
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time


def stop(process):
    if process is None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        pass
    # Also stop descendants when the command's leader has already exited.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def interrupted(signum, _frame):
    raise SystemExit(128 + signum)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--width", type=int, default=1400)
    parser.add_argument("--height", type=int, default=900)
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command or min(args.width, args.height, args.timeout) <= 0:
        parser.error("provide a command and positive dimensions/timeout")

    root = Path(__file__).resolve().parent.parent
    logs_root = root / "target/wayland-smoke"
    logs_root.mkdir(parents=True, exist_ok=True)
    logs = Path(tempfile.mkdtemp(prefix="run-", dir=logs_root))
    print(f"Wayland check logs: {logs}", flush=True)
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, interrupted)

    compositor = child = None
    result = 1
    try:
        with tempfile.TemporaryDirectory(prefix="tursobench-wayland-") as runtime:
            environment = dict(os.environ, XDG_RUNTIME_DIR=runtime,
                               WAYLAND_DISPLAY="wayland-test", XDG_SESSION_TYPE="wayland",
                               LIBGL_ALWAYS_SOFTWARE="1", GALLIUM_DRIVER="llvmpipe")
            for key in ("DISPLAY", "XAUTHORITY", "WAYLAND_SOCKET", "EGL_PLATFORM"):
                environment.pop(key, None)
            try:
                with (logs / "compositor.log").open("w") as output:
                    compositor = subprocess.Popen(
                        ["weston", "--backend=headless", "--renderer=gl",
                         "--shell=desktop-shell.so", "--no-config", "--idle-time=0",
                         "--socket=wayland-test", f"--width={args.width}",
                         f"--height={args.height}"],
                        env=environment, stdout=output, stderr=subprocess.STDOUT,
                        start_new_session=True)
                deadline = time.monotonic() + 15
                while True:
                    if compositor.poll() is not None:
                        raise RuntimeError("Weston exited before accepting clients")
                    try:
                        with socket.socket(socket.AF_UNIX) as probe:
                            probe.settimeout(0.2)
                            probe.connect(str(Path(runtime) / "wayland-test"))
                        break
                    except (FileNotFoundError, ConnectionRefusedError, TimeoutError):
                        if time.monotonic() >= deadline:
                            raise RuntimeError("Weston did not become ready within 15 seconds")
                        time.sleep(0.05)

                environment.update(EGL_PLATFORM="wayland", TURSOBENCH_EXPECT_BACKEND="wayland")
                with (logs / "client.log").open("w") as output:
                    child = subprocess.Popen(command, env=environment, stdout=output,
                                             stderr=subprocess.STDOUT, start_new_session=True)
                    try:
                        result = child.wait(timeout=args.timeout)
                    except subprocess.TimeoutExpired:
                        result = 124
                        print(f"Wayland check exceeded {args.timeout:g} seconds", file=sys.stderr)
                if compositor.poll() is not None:
                    raise RuntimeError("Weston exited while the check was running")
            finally:
                stop(child)
                stop(compositor)
    except (OSError, RuntimeError) as error:
        print(f"Wayland check failed: {error}", file=sys.stderr)
        result = 1
    finally:
        client_log = logs / "client.log"
        if client_log.exists():
            print(client_log.read_text(errors="replace"), end="", flush=True)
        if result:
            compositor_log = logs / "compositor.log"
            if compositor_log.exists():
                print(compositor_log.read_text(errors="replace"), file=sys.stderr)
            print(f"Wayland check failed; logs retained at {logs}", file=sys.stderr)
    return result if result >= 0 else 128 - result


if __name__ == "__main__":
    sys.exit(main())
