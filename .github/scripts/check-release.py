#!/usr/bin/env python3
"""Check build.rs provenance and both commands in a native dist archive."""

import pathlib
import subprocess
import sys
import tarfile


def check(root, target, expected):
    outputs = list((root / "target" / target / "dist" / "build").glob("kashkeshet-*/output"))
    if not outputs:
        raise ValueError("no release build.rs output found")
    recorded = f"cargo:rustc-env=KASHKESHET_GIT_COMMIT={expected}"
    for output in outputs:
        if recorded not in output.read_text().splitlines():
            raise ValueError(f"{output}: expected clean release commit {expected}")
    archive = root / "target" / "distrib" / f"kashkeshet-{target}.tar.xz"
    with tarfile.open(archive) as package:
        for command in ("kashkeshet", "kkt"):
            member = package.getmember(f"kashkeshet-{target}/{command}")
            if not member.isfile() or not member.mode & 0o111:
                raise ValueError(f"{command}: archive must contain an executable file")
            # Check the compiled artifact as well as Cargo's build-script output.
            with package.extractfile(member) as binary:
                content = binary.read()
                if (expected + "-dirty").encode() in content:
                    raise ValueError(f"{command}: binary records a dirty release commit")
                if expected.encode() not in content:
                    raise ValueError(f"{command}: release commit absent from binary")


if __name__ == "__main__":
    root = pathlib.Path(__file__).resolve().parents[2]
    expected = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    check(root, sys.argv[1], expected)
    print(f"Both packaged commands record clean commit {expected}")
