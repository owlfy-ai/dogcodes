#!/usr/bin/env python3
"""Owned D-Bus portal + real PipeWire synthetic video; never the user's screen.

CI fixture prerequisites: PipeWire, WirePlumber 0.4, dbus-run-session and Rust.
The production backend does not depend on this fixture or its fake lock checker.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import uuid


def run(command, env, **kwargs):
    return subprocess.run(command, env=env, check=True, **kwargs)


def packaged_client(binary, appimage, root):
    """Exercise AppRun with the native fixture as its private entry executable.

    Keep the synthetic server/source on system libraries; only the client uses
    the actual package's launcher, configuration, modules and linked libraries.
    """
    run([str(appimage.resolve()), "--appimage-extract"], os.environ,
        cwd=root, stdout=subprocess.DEVNULL)
    packaged = root / "squashfs-root"
    client = root / "packaged-client"
    (client / "usr/bin").mkdir(parents=True)
    for name in ("lib", "share"):
        (client / "usr" / name).symlink_to(packaged / "usr" / name)
    (client / "usr/bin/desktop").symlink_to(binary)
    shutil.copy2(packaged / "usr/bin/dogcodes-appimage", client / "usr/bin/dogcodes-appimage")
    for name in ("AppRun", "AppRun.wrapped"):
        shutil.copy2(packaged / name, client / name)
    for name in ("apprun-hooks", "dog.codes.desktop"):
        (client / name).symlink_to(packaged / name)
    wrapper = root / "packaged-client.sh"
    wrapper.write_text(
        '#!/bin/sh\nset -eu\n'
        'if [ "${1:-}" = serve_raw_source_fixture ]; then\n'
        f'  exec {shlex.quote(binary)} "$@"\nfi\n'
        f'export APPDIR={shlex.quote(str(client))}\n'
        f'exec {shlex.quote(str(client / "AppRun"))} "$@"\n')
    wrapper.chmod(0o755)
    return str(wrapper)


def child(binary, directory):
    root = Path(directory)
    marker = os.environ.get("DOGCODES_WAYLAND_FIXTURE_MARKER", "")
    if not marker or (root / ".fixture-owned").read_text() != marker:
        raise RuntimeError("The fixture child must be launched by this script.")
    address = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
    if not address or address == os.environ.get("DOGCODES_PARENT_DBUS_ADDRESS", ""):
        raise RuntimeError("An independent D-Bus session is required.")
    env = dict(os.environ)
    for name in ("PIPEWIRE_RUNTIME_DIR", "PIPEWIRE_CONFIG_DIR", "PIPEWIRE_CONFIG_PREFIX",
                 "PIPEWIRE_CONFIG_NAME", "WIREPLUMBER_CONFIG_DIR"):
        env.pop(name, None)
    env.update(XDG_RUNTIME_DIR=str(root), PIPEWIRE_REMOTE="pipewire-0",
               XDG_CONFIG_HOME=str(root / "config"), XDG_CACHE_HOME=str(root / "cache"),
               XDG_DATA_HOME=str(root / "data"), DOGCODES_OWNED_PORTAL="portal-fixture-v1",
               DBUS_SYSTEM_BUS_ADDRESS=address, XDG_SESSION_ID="dogcodes-fixture")
    # login1 is mocked on this same owned bus. The native OS-query code runs
    # unchanged, without consulting the host's system bus or a fake CLI.
    config = root / "owned-pipewire.conf"
    contents = Path("/usr/share/pipewire/pipewire.conf").read_text()
    config.write_text(contents.replace("context.spa-libs = {",
        "context.spa-libs = {\n    videotestsrc = videotestsrc/libspa-videotestsrc", 1))
    policy = root / "wireplumber"
    shutil.copytree("/usr/share/wireplumber", policy)
    if not (policy / "main.lua.d").exists():
        raise RuntimeError("This CI fixture requires WirePlumber 0.4; production does not.")
    for path in policy.rglob("*config.lua"):
        path.write_text(re.sub(r"(\w*_monitor\.enabled\s*=\s*)true", r"\1false", path.read_text()))
    # Early 0.4 releases unconditionally enable monitors and Bluetooth logind.
    # Disable their entrypoints in this private copy as well; no physical device
    # monitoring or system login service is needed for the synthetic source.
    for directory in policy.glob("*.lua.d"):
        (directory / "89-dogcodes-owned-monitors.lua").write_text("\n".join(
            f"if {name} then {name}.enable = function() end end"
            for name in ("alsa_monitor", "v4l2_monitor", "libcamera_monitor", "bluez_monitor")))
    env["WIREPLUMBER_CONFIG_DIR"] = str(policy)
    processes = []
    logs = []
    try:
        for name, command in [("pipewire", ["pipewire", "-c", str(config)]),
                              ("wireplumber", ["wireplumber"])]:
            log = (root / f"{name}.log").open("w")
            logs.append(log)
            processes.append(subprocess.Popen(command, env=env, stdout=log, stderr=log))
        for _ in range(50):
            if subprocess.run(["pw-cli", "info", "0"], env=env,
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
                break
            time.sleep(0.1)
        video_format = env.get("DOGCODES_OWNED_VIDEO_FORMAT", "rgb")
        if video_format == "spa-rgb":
            run(["pw-cli", "create-node", "adapter",
                 "{ factory.name=videotestsrc node.name=dogcodes-fixture media.class=Video/Source object.linger=true }"], env)
        elif video_format in ("rgb", "rgb-cursor", "nv12-default", "nv12-bt601", "nv12-bt709", "nv12-bt709-planes", "rgb-crop-rotate90", "rgb-crop-flipped90"):
            log = (root / "video-source.log").open("w")
            logs.append(log)
            processes.append(subprocess.Popen([
                binary, "serve_raw_source_fixture", "--ignored", "--nocapture"
            ], env=env, stdout=log, stderr=log))
        else:
            raise RuntimeError("Unknown owned video format.")
        nodes = []
        for _ in range(100):
            graph = json.loads(run(["pw-dump"], env, capture_output=True).stdout)
            nodes = [item for item in graph if item.get("info", {}).get("props", {}).get("node.name") == "dogcodes-fixture"]
            if len(nodes) == 1:
                break
            time.sleep(0.1)
        if len(nodes) != 1:
            raise RuntimeError("The owned synthetic video source was not created.")
        source = nodes[0]
        env["DOGCODES_OWNED_PIPEWIRE_NODE"] = str(source["id"])
        env["DOGCODES_OWNED_PIPEWIRE_SERIAL"] = str(source["info"]["props"]["object.serial"])
        run([binary, "owned_portal_fixture", "--ignored", "--nocapture"], env)
    except BaseException:
        for name in ("pipewire", "wireplumber", "video-source"):
            path = root / f"{name}.log"
            if path.exists():
                print(f"{name} fixture diagnostics:\n" + "\n".join(path.read_text().splitlines()[-(400 if env.get("WIREPLUMBER_DEBUG") else 60):]), file=sys.stderr)
        raise
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        for log in logs:
            log.close()


def main():
    if sys.platform != "linux":
        raise SystemExit("Run this isolated fixture on Linux (for example in Docker).")
    if len(sys.argv) == 4 and sys.argv[1] == "--child":
        child(sys.argv[2], sys.argv[3])
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--format", choices=("rgb", "spa-rgb", "rgb-cursor", "nv12-default", "nv12-bt601", "nv12-bt709", "nv12-bt709-planes", "rgb-crop-rotate90", "rgb-crop-flipped90"), default="rgb")
    parser.add_argument("--appimage", type=Path, help="Verify a built AppImage's client runtime against the owned portal")
    parser.add_argument("--release", action="store_true", help="Build and verify the optimized native test executable")
    options = parser.parse_args()
    command = ["cargo", "test", "-p", "dogcodes-remote-desktop", "--locked", "--no-run", "--message-format=json"]
    if options.release:
        command.append("--release")
    try:
        built = run(command, os.environ, capture_output=True, text=True)
    except subprocess.CalledProcessError as error:
        print(error.stderr, file=sys.stderr)
        raise
    binaries = [item["executable"] for line in built.stdout.splitlines()
                if (item := json.loads(line)).get("reason") == "compiler-artifact"
                and item.get("executable") and item.get("profile", {}).get("test")
                and item.get("target", {}).get("name") == "dogcodes_remote_desktop"]
    if len(binaries) != 1:
        raise RuntimeError("Could not identify the native fixture test executable.")
    with tempfile.TemporaryDirectory(prefix="dogcodes-wayland-fixture-") as directory:
        binary = packaged_client(binaries[0], options.appimage, Path(directory)) if options.appimage else binaries[0]
        marker = uuid.uuid4().hex
        (Path(directory) / ".fixture-owned").write_text(marker)
        env = dict(os.environ, DOGCODES_OWNED_VIDEO_FORMAT=options.format,
                   DOGCODES_WAYLAND_FIXTURE_MARKER=marker,
                   DOGCODES_PARENT_DBUS_ADDRESS=os.environ.get("DBUS_SESSION_BUS_ADDRESS", ""))
        run(["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()),
             "--child", binary, directory], env)


if __name__ == "__main__":
    main()
