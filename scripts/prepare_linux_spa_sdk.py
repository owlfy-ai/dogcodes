#!/usr/bin/env python3
"""Prepare pinned SPA build headers without replacing the system PipeWire runtime."""
import argparse
import hashlib
import os
from pathlib import Path, PurePosixPath
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

VERSION = "0.3.65"
COMMIT = "9558a5d5e0360d8af822431c76ee858a8c7495ac"
SHA256 = "c25238daf35eab61d5edf881509b72591a8d58c34301b1b73a79f2eab94b76b6"
URL = f"https://codeload.github.com/PipeWire/pipewire/tar.gz/{COMMIT}"
LIMIT = 32 * 1024 * 1024


def system_headers_ready():
    try:
        include = subprocess.check_output(
            ["pkg-config", "--variable=includedir", "libspa-0.2"], text=True,
            stderr=subprocess.DEVNULL).strip()
        if (Path(include).parent / ".dogcodes-spa-sdk").exists():
            return False  # Re-verify and repair our managed SDK on every invocation.
        raw = (Path(include) / "spa/param/video/raw.h").read_text()
        return bool(re.search(r"SPA_VIDEO_FLAG_MODIFIER\s*=", raw)
                    and re.search(r"uint32_t\s+flags\b", raw))
    except (OSError, subprocess.CalledProcessError):
        return False


def extract_headers(archive, destination, digest=SHA256):
    """Only verified regular headers are installed; archive paths never escape."""
    archive = Path(archive)
    if archive.stat().st_size > LIMIT or hashlib.sha256(archive.read_bytes()).hexdigest() != digest:
        raise RuntimeError("SPA SDK archive checksum mismatch")
    written = 0
    total = 0
    with tarfile.open(archive) as source:
        for item in source:
            parts = PurePosixPath(item.name).parts
            if len(parts) < 4 or parts[1:3] != ("spa", "include"):
                continue
            relative = parts[3:]
            if any(part in ("..", ".") for part in relative) or PurePosixPath(item.name).is_absolute():
                raise RuntimeError("Unsafe SPA SDK header path")
            if item.isdir():
                continue
            if not item.isfile():
                raise RuntimeError("SPA SDK headers cannot contain links or devices")
            if not item.name.endswith(".h"):
                continue
            total += item.size
            if item.size < 0 or total > LIMIT or written >= 2048:
                raise RuntimeError("SPA SDK headers exceed size limits")
            target = Path(destination).joinpath(*relative)
            target.parent.mkdir(parents=True, exist_ok=True)
            with source.extractfile(item) as incoming, target.open("wb") as output:
                shutil.copyfileobj(incoming, output)
            written += 1
    if not (Path(destination) / "spa/param/video/raw.h").is_file():
        raise RuntimeError("SPA SDK archive is missing video headers")
    return written


def prepare(root, archive=None):
    import fcntl  # Linux build helper; macOS can also run its isolated tests.
    root = Path(root).resolve()
    if any(char in str(root) for char in "\r\n"):
        raise RuntimeError("SDK path cannot contain newlines")
    root.mkdir(parents=True, exist_ok=True)
    sdk = root / f"pipewire-spa-{VERSION}-{COMMIT[:12]}"
    with (root / ".spa-sdk.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if archive is None:
            archive = root / f"{COMMIT}.tar.gz"
            if not archive.is_file() or archive.stat().st_size > LIMIT or hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
                with tempfile.NamedTemporaryFile(dir=root, delete=False) as download:
                    temporary = Path(download.name)
                    try:
                        with urllib.request.urlopen(URL, timeout=45) as incoming:
                            length = 0
                            while chunk := incoming.read(64 * 1024):
                                length += len(chunk)
                                if length > LIMIT:
                                    raise RuntimeError("SPA SDK download exceeds size limit")
                                download.write(chunk)
                        download.flush()
                        if hashlib.sha256(temporary.read_bytes()).hexdigest() != SHA256:
                            raise RuntimeError("SPA SDK download checksum mismatch")
                        os.replace(temporary, archive)
                    finally:
                        temporary.unlink(missing_ok=True)
        # Re-extract verified bytes rather than trusting a cached marker or
        # cached headers which may have changed since the previous build.
        with tempfile.TemporaryDirectory(prefix=".spa-sdk-", dir=root) as temporary:
            temporary = Path(temporary)
            prepared = temporary / "prepared"
            prepared.mkdir()
            extract_headers(archive, prepared / "include", SHA256)
            (prepared / ".dogcodes-spa-sdk").write_text(SHA256)
            config = prepared / "pkgconfig"
            config.mkdir()
            (config / "libspa-0.2.pc").write_text(
                f"prefix={sdk}\nincludedir=${{prefix}}/include\n\n"
                f"Name: libspa\nDescription: pinned PipeWire {VERSION} SPA build headers\n"
                "Version: 0.2\nCflags: -I\"${includedir}\"\n")
            backup = temporary / "previous"
            if sdk.exists():
                marker = sdk / ".dogcodes-spa-sdk"
                if not marker.is_file() or marker.read_text() != SHA256:
                    raise RuntimeError("Refusing to replace an unowned SPA SDK directory")
                expected = {path.relative_to(prepared) for path in prepared.rglob("*") if path.is_file()}
                actual = {path.relative_to(sdk) for path in sdk.rglob("*") if path.is_file() or path.is_symlink()}
                if actual == expected and all(
                    not (sdk / path).is_symlink()
                    and (sdk / path).stat().st_size == (prepared / path).stat().st_size
                    and (sdk / path).read_bytes() == (prepared / path).read_bytes()
                    for path in expected
                ):
                    return sdk / "pkgconfig"  # Keep valid headers stable for concurrent compilers.
                os.replace(sdk, backup)
            try:
                os.replace(prepared, sdk)
            except BaseException:
                if backup.exists() and not sdk.exists():
                    os.replace(backup, sdk)
                raise
    return sdk / "pkgconfig"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", default="target/native-sdk")
    parser.add_argument("--archive", type=Path, help="Previously downloaded pinned archive (offline builds)")
    parser.add_argument("--force", action="store_true", help="Prepare even when system headers are sufficient")
    parser.add_argument("--github-env", type=Path)
    parser.add_argument("--print-env", action="store_true", help="Print a shell-quoted PKG_CONFIG_PATH export")
    options = parser.parse_args()
    previous = os.environ.get("PKG_CONFIG_PATH", "")
    if any(char in previous for char in "\r\n"):
        raise RuntimeError("PKG_CONFIG_PATH cannot contain newlines")
    value = previous
    if options.force or not system_headers_ready():
        config = prepare(options.destination, options.archive)
        entries = [str(config)] + (previous.split(os.pathsep) if previous else [])
        value = os.pathsep.join(dict.fromkeys(entries))
        print(f"Prepared SPA {VERSION} headers; system PipeWire runtime unchanged.", file=sys.stderr)
    if options.github_env:
        with options.github_env.open("a") as output:
            output.write(f"PKG_CONFIG_PATH={value}\n")
    if options.print_env:
        print("export PKG_CONFIG_PATH=" + shlex.quote(value))


if __name__ == "__main__":
    main()
