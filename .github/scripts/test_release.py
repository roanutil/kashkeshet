"""Release gates: reject wrong provenance and incomplete archives; enforce locks."""

import importlib.util
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("check_release", SCRIPTS / "check-release.py")
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class ReleaseChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.target = "aarch64-unknown-linux-gnu"
        self.commit = "1234567890abcdef" * 2 + "12345678"
        output = self.root / "target" / self.target / "dist/build/kashkeshet-test/output"
        output.parent.mkdir(parents=True)
        output.write_text(f"cargo:rustc-env=KASHKESHET_GIT_COMMIT={self.commit}\n")
        self.output = output
        self.archive()

    def archive(self, commands=("kkt",), embedded=None):
        path = self.root / "target/distrib" / f"kashkeshet-{self.target}.tar.xz"
        path.parent.mkdir(exist_ok=True)
        with tarfile.open(path, "w:xz") as package:
            for command in commands:
                content = (embedded if embedded is not None else self.commit).encode()
                info = tarfile.TarInfo(f"kashkeshet-{self.target}/{command}")
                info.size = len(content)
                info.mode = 0o755
                package.addfile(info, io.BytesIO(content))

    def test_clean_archive_passes(self):
        CHECK.check(self.root, self.target, self.commit)

    def test_dirty_unknown_and_wrong_commits_fail(self):
        for recorded in (self.commit + "-dirty", "unknown", "f" * 40):
            with self.subTest(recorded=recorded):
                self.output.write_text(f"cargo:rustc-env=KASHKESHET_GIT_COMMIT={recorded}\n")
                with self.assertRaises(ValueError):
                    CHECK.check(self.root, self.target, self.commit)

    def test_missing_build_output_fails(self):
        self.output.unlink()
        with self.assertRaises(ValueError):
            CHECK.check(self.root, self.target, self.commit)

    def test_missing_kkt_fails(self):
        self.archive(commands=("kashkeshet",))
        with self.assertRaises(KeyError):
            CHECK.check(self.root, self.target, self.commit)

    def test_archive_from_another_build_fails(self):
        self.archive(embedded="unknown")
        with self.assertRaises(ValueError):
            CHECK.check(self.root, self.target, self.commit)

    def test_dirty_binary_fails_even_with_clean_build_output(self):
        self.archive(embedded=self.commit + "-dirty")
        with self.assertRaisesRegex(ValueError, "binary records a dirty"):
            CHECK.check(self.root, self.target, self.commit)

    def test_cargo_wrapper_locks_builds_and_preserves_arguments(self):
        cargo = self.root / "real-cargo"
        cargo.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
        cargo.chmod(0o700)
        env = dict(os.environ, KKT_RELEASE_CARGO=str(cargo))
        for args, expected in [
            (["build", "--target", self.target], ["build", "--target", self.target, "--locked"]),
            (["metadata", "--format-version=1"], ["metadata", "--format-version=1", "--locked"]),
            (["fetch"], ["fetch", "--locked"]),
            (["--version"], ["--version"]),
        ]:
            with self.subTest(args=args):
                result = subprocess.check_output(
                    ["sh", str(SCRIPTS / "cargo-locked.sh"), *args], env=env, text=True
                )
                self.assertEqual(result.splitlines(), expected)


if __name__ == "__main__":
    unittest.main()
