#!/usr/bin/env python3
"""Desktop codec migration, geometry and backpressure on a private login1 bus.

Linux development dependencies are the desktop build dependencies, Python 3,
and dbus-run-session. No X11/Wayland display, portal or physical input is used.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import uuid


def executable(package, target):
    command = ["cargo", "test", "-p", package, "--lib", "--locked", "--no-run", "--message-format=json"]
    binaries = []
    # Keep long native builds observable without retaining every artifact and
    # compiler message in memory. stderr remains the real Cargo progress log.
    with subprocess.Popen(command, stdout=subprocess.PIPE, text=True) as build:
        for line in build.stdout:
            item = json.loads(line)
            if item.get("reason") == "compiler-message":
                rendered = item.get("message", {}).get("rendered")
                if rendered:
                    print(rendered, file=sys.stderr, end="")
            if (item.get("reason") == "compiler-artifact" and item.get("executable")
                    and item.get("profile", {}).get("test") and item.get("target", {}).get("name") == target):
                binaries.append(item["executable"])
        status = build.wait()
        if status:
            raise subprocess.CalledProcessError(status, command)
    if len(binaries) != 1:
        raise RuntimeError(f"Could not identify the {target} test executable.")
    return binaries[0]


def child(native, desktop, directory):
    root = Path(directory)
    marker = os.environ.get("DOGCODES_SESSION_STATE_MARKER", "")
    address = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
    if not marker or (root / ".fixture-owned").read_text() != marker:
        raise RuntimeError("The codec fixture must be launched by this script.")
    if not address or address == os.environ.get("DOGCODES_PARENT_DBUS_ADDRESS", ""):
        raise RuntimeError("An independent D-Bus session is required.")
    env = dict(os.environ)
    for name in ("DISPLAY", "WAYLAND_DISPLAY", "PIPEWIRE_REMOTE", "PIPEWIRE_RUNTIME_DIR"):
        env.pop(name, None)
    env.update(XDG_RUNTIME_DIR=str(root), XDG_CONFIG_HOME=str(root / "config"),
               XDG_DATA_HOME=str(root / "data"), XDG_CACHE_HOME=str(root / "cache"),
               XDG_SESSION_ID="dogcodes-fixture", DBUS_SYSTEM_BUS_ADDRESS=address,
               DOGCODES_OWNED_SESSION_STATE="login1-fixture-v1")
    log = root / "login1.log"
    with log.open("w") as output:
        server = subprocess.Popen([native, "serve_logind_fixture", "--ignored", "--nocapture"],
                                  env=env, stdout=output, stderr=output)
        try:
            deadline = time.monotonic() + 15
            while not (root / ".login1-ready").exists():
                if server.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("The private login1 service did not start.")
                time.sleep(0.05)
            if (root / ".login1-ready").read_text() != marker:
                raise RuntimeError("Private login1 readiness identity mismatch.")
            if desktop == "fixture-only":
                subprocess.run(["dbus-send", "--system", "--print-reply", "--dest=org.freedesktop.login1",
                                "/org/freedesktop/login1", "org.freedesktop.login1.Manager.GetSession",
                                "string:dogcodes-fixture"], env=env, check=True, timeout=10)
            else:
                subprocess.run([desktop, "daemon::remote::video::codec_switch_tests::",
                                "--ignored", "--nocapture", "--test-threads=1"],
                               env=env, check=True, timeout=120)
        except BaseException:
            output.flush()
            print("Private login1 diagnostics:\n" + "\n".join(log.read_text().splitlines()[-80:]), file=sys.stderr)
            raise
        finally:
            if server.poll() is None:
                server.terminate()
                try:
                    server.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()


def main():
    if sys.platform != "linux":
        raise SystemExit("Run this owned fixture on Linux (for example in Docker).")
    if len(sys.argv) == 5 and sys.argv[1] == "--child":
        child(*sys.argv[2:])
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture-only", action="store_true", help="validate only the private bus/service, without building desktop")
    options = parser.parse_args()
    native = executable("dogcodes-remote-desktop", "dogcodes_remote_desktop")
    desktop = "fixture-only" if options.fixture_only else executable("desktop", "desktop_lib")
    with tempfile.TemporaryDirectory(prefix="dogcodes-codec-fixture-") as directory:
        marker = uuid.uuid4().hex
        (Path(directory) / ".fixture-owned").write_text(marker)
        env = dict(os.environ, DOGCODES_SESSION_STATE_MARKER=marker,
                   DOGCODES_PARENT_DBUS_ADDRESS=os.environ.get("DBUS_SESSION_BUS_ADDRESS", ""))
        subprocess.run(["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()),
                        "--child", native, desktop, directory], env=env, check=True)


if __name__ == "__main__":
    main()
