import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import prepare_linux_spa_sdk as sdk


class SpaSdkTests(unittest.TestCase):
    def archive(self, root, name="source/spa/include/spa/param/video/raw.h", link=False):
        target = root / "source.tar.gz"
        with tarfile.open(target, "w:gz") as output:
            entry = tarfile.TarInfo(name)
            if link:
                entry.type = tarfile.SYMTYPE
                entry.linkname = "/outside"
                output.addfile(entry)
            else:
                contents = b"/* isolated SDK header */\n"
                entry.size = len(contents)
                output.addfile(entry, io.BytesIO(contents))
        return target, hashlib.sha256(target.read_bytes()).hexdigest()

    def test_checksum_failure_does_not_extract_anything(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, _ = self.archive(root)
            with self.assertRaisesRegex(RuntimeError, "checksum"):
                sdk.extract_headers(archive, root / "out")
            self.assertFalse((root / "out").exists())

    def test_parents_and_symlinks_cannot_escape_verified_archive_extraction(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name, link in [("source/spa/include/../../outside.h", False),
                               ("source/spa/include/spa/linked.h", True)]:
                archive, digest = self.archive(root, name, link)
                with self.assertRaises(RuntimeError):
                    sdk.extract_headers(archive, root / "out", digest)
                self.assertFalse((root / "outside.h").exists())

    def test_modified_managed_headers_are_repaired_from_verified_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, digest = self.archive(root)
            with patch.object(sdk, "SHA256", digest):
                config = sdk.prepare(root / "cache", archive)
                header = config.parent / "include/spa/param/video/raw.h"
                original = header.stat().st_mtime_ns
                sdk.prepare(root / "cache", archive)
                self.assertEqual(header.stat().st_mtime_ns, original)
                header.write_text("stale or modified")
                sdk.prepare(root / "cache", archive)
                self.assertEqual(header.read_text(), "/* isolated SDK header */\n")
                self.assertIn('Version: 0.2', (config / "libspa-0.2.pc").read_text())

    def test_unowned_directory_is_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, digest = self.archive(root)
            occupied = root / "cache" / f"pipewire-spa-{sdk.VERSION}-{sdk.COMMIT[:12]}"
            occupied.mkdir(parents=True)
            (occupied / "keep.txt").write_text("user data")
            with patch.object(sdk, "SHA256", digest), self.assertRaisesRegex(RuntimeError, "unowned"):
                sdk.prepare(root / "cache", archive)
            self.assertEqual((occupied / "keep.txt").read_text(), "user data")

    def test_older_sdk_is_detected_even_when_pkg_config_succeeds(self):
        with tempfile.TemporaryDirectory() as temporary:
            include = Path(temporary) / "include"
            raw = include / "spa/param/video/raw.h"
            raw.parent.mkdir(parents=True)
            raw.write_text("struct spa_video_info_raw { int format; };\n")
            with patch.object(sdk.subprocess, "check_output", return_value=str(include)):
                self.assertFalse(sdk.system_headers_ready())
                raw.write_text("enum { SPA_VIDEO_FLAG_MODIFIER = 4 }; struct x { uint32_t flags; };")
                self.assertTrue(sdk.system_headers_ready())


if __name__ == "__main__":
    unittest.main()
