#!/usr/bin/env python3
"""Accept stable/RC tags only, with an exact Cargo package version match."""

from pathlib import Path
import re
import sys
import tomllib

NUMBER = r"(?:0|[1-9][0-9]*)"
TAG = re.compile(rf"v{NUMBER}\.{NUMBER}\.{NUMBER}(?:-rc\.{NUMBER})?")


def check(tag, manifest):
    if not TAG.fullmatch(tag):
        raise ValueError("release tag must be vX.Y.Z or vX.Y.Z-rc.N (no leading zeros)")
    package = tomllib.loads(manifest.read_text())["package"]
    version = package["version"]
    if tag != f"v{version}":
        raise ValueError(f"tag {tag} does not match Cargo package version {version}; commit the matching Cargo.toml and Cargo.lock before tagging")
    return "-rc." in version


if __name__ == "__main__":
    manifest = Path(__file__).resolve().parents[2] / "Cargo.toml"
    try:
        if len(sys.argv) != 2:
            raise ValueError("usage: check-release-tag.py vX.Y.Z[-rc.N]")
        candidate = check(sys.argv[1], manifest)
    except (ValueError, KeyError, OSError) as error:
        sys.exit(str(error))
    print(f"Validated {'release-candidate' if candidate else 'stable'} tag {sys.argv[1]} against Cargo.toml")
