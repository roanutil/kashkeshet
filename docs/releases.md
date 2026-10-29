# Installing and releases

## Installing

Install the latest stable `kkt` on macOS (Apple Silicon or Intel) or Linux
(ARM64 or x86_64) with:

```sh
curl -fsSL https://github.com/sam-mfb/kashkeshet/releases/latest/download/kashkeshet-installer.sh | sh
kkt --version
```

The shell installer selects the native archive and installs `kkt`
into `$CARGO_HOME/bin` (or `~/.cargo/bin`). Linux builds use GNU libc on
Ubuntu 24.04; use Ubuntu 24.04 or newer, or another distribution with a
compatible glibc. Windows binaries and a Homebrew tap are not published yet.
To build from source instead, use the pinned Rust toolchain and run
`cargo install --path . --locked`.

Release candidates use a tag-specific installer; GitHub's `latest` URL is
for stable releases. To install v0.4.2 specifically:

```sh
curl -fsSL https://github.com/sam-mfb/kashkeshet/releases/download/v0.4.2/kashkeshet-installer.sh | sh
kkt --version # kashkeshet 0.4.2
```

## Container image

Each stable or candidate tag also publishes `ghcr.io/sam-mfb/kashkeshet:X.Y.Z`
for linux/amd64 and linux/arm64, built from the tagged commit by the
`image-build` and `image-publish` jobs in the release workflow. Stable tags
also move `latest`. See [Docker](docker.md).

## Verify a download

The [releases](https://github.com/sam-mfb/kashkeshet/releases) contain one
`.tar.xz` archive and a SHA-256 checksum per target, a combined `sha256.sum`,
and the installer. Each archive contains the `kkt` executable.

For example, download and verify the Linux x86_64 archive before unpacking
it (replace the example version with a published release):

```sh
VERSION=v0.4.2
ARCHIVE=kashkeshet-x86_64-unknown-linux-gnu.tar.xz
gh release download "$VERSION" -R sam-mfb/kashkeshet --pattern "$ARCHIVE" --pattern "$ARCHIVE.sha256"
sha256sum -c "$ARCHIVE.sha256" # on macOS: shasum -a 256 -c "$ARCHIVE.sha256"
gh attestation verify "$ARCHIVE" --repo sam-mfb/kashkeshet --signer-workflow sam-mfb/kashkeshet/.github/workflows/release.yml
tar -xf "$ARCHIVE"
```

The checksum detects download corruption; the GitHub artifact attestation
verifies the repository and workflow that produced the archive. The same
`gh attestation verify` command works for the installer and checksums, which
are attested too. See [GitHub's verification instructions](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/verify-attestations).

## Preparing a release

Release packaging uses [dist 0.33.1](https://github.com/axodotdev/cargo-dist/releases/tag/v0.33.1),
configured in `Cargo.toml`. After a version change is merged, only an
owner-authorized `vX.Y.Z` or `vX.Y.Z-rc.N` tag triggers publishing; pushing
a branch does not. Pull requests run `dist plan` without creating a release.
The coordinator tags after `next` is merged into `main`, with Sam's approval.
Preparing a release does not authorize pushing a tag.

The tag must equal `v` followed by the complete Cargo package version,
including any `-rc.N` suffix. The workflow checks this before planning a
release. Cargo.toml and Cargo.lock must be committed with the same version;
the binary and its recorded provenance then report that version too.

| Cargo version | Tag | GitHub release |
|---|---|---|
| `0.1.0-rc.1` | `v0.1.0-rc.1` | Pre-release; use its tag-specific installer |
| `0.1.0` | `v0.1.0` | Stable |
| `0.2.0` | `v0.2.0` | Stable |
| `0.3.0` | `v0.3.0` | Stable |
| `0.4.0` | `v0.4.0` | Stable |
| `0.4.1` | `v0.4.1` | Stable |
| `0.4.2` | `v0.4.2` | Stable; available through `releases/latest` |

The validated `0.1.0-rc.1` candidate was followed by stable `0.1.0`,
then `0.2.0`, `0.3.0`, `0.4.0`, `0.4.1` and `0.4.2`.
The current Cargo version and release tag are `0.4.2` and `v0.4.2`.
Do not override a version mismatch with `--force-tag`.

The four targets build natively on `macos-15`, `macos-15-intel`,
`ubuntu-24.04`, and `ubuntu-24.04-arm`. A Cargo wrapper enforces `--locked`
(dist 0.33.1 does not add it), and each native job checks that the archived
`kkt` embeds the clean, exact Git commit recorded by `build.rs`.
Generated manifests are ignored so they cannot mark that commit dirty.
All workflow actions are pinned by commit. Attestations run after the
global artifacts have been assembled, covering the installer too.

To validate locally without publishing or creating a tag, install that
version of dist and run:

```sh
dist plan --output-format=json
python3 .github/scripts/check-release-tag.py v0.4.2
dist plan --tag=v0.4.2 --output-format=json
python3 -m unittest discover -s .github/scripts -p 'test_*.py'
# Example native target; use the target shown by `rustc -vV` on your machine.
export KKT_RELEASE_CARGO="$(command -v cargo)"
RELEASE_TOOLS="$(mktemp -d)"
cp .github/scripts/cargo-locked.sh "$RELEASE_TOOLS/cargo"
chmod +x "$RELEASE_TOOLS/cargo"
PATH="$RELEASE_TOOLS:$PATH" dist build --artifacts=local --target=aarch64-unknown-linux-gnu
python3 .github/scripts/check-release.py aarch64-unknown-linux-gnu
rm -rf "$RELEASE_TOOLS"
```

Run the archive check from a clean committed tree with fresh native build
output. When switching package versions, use a fresh checkout or clear that
target's dist-profile build directory: Cargo may retain build-script records
from the previous version. A local build of edited sources deliberately
records `-dirty` and fails the check. `dist build
--artifacts=global` builds the installer after the native artifacts exist.

`.github/workflows/release.yml` starts from dist's generated workflow but
has maintained changes for the stable/RC tag filters, the exact Cargo tag
check (`.github/scripts/check-release-tag.py`), locked Cargo wrapper,
release-check tests, and packaged provenance check. `allow-dirty = ["ci"]`
in the dist config permits those edits. When updating dist, temporarily
remove that setting, run `dist generate --mode ci`, then restore the setting
and those marked workflow changes. Keep validation after checkout and before
any dist hosting step; retain the generated manifest-based `--prerelease`
flag on `gh release create`. `force-latest = false` keeps RCs out of the
stable release path. Review the diff and rerun the checks with both a stable
and an RC version fixture, confirming `announcement_is_prerelease` is false
for the former and true for the latter.
The commented Homebrew settings in `Cargo.toml` document how to enable a
tap later; no tap repository or publishing credential is needed today.

