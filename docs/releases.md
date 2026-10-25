# Installing and releases

## Installing

Once the first version is released, install `kkt` on macOS (Apple Silicon
or Intel) or Linux (ARM64 or x86_64) with:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/sam-mfb/kashkeshet/releases/latest/download/kashkeshet-installer.sh | sh
kkt --version
```

The shell installer selects the native archive and installs `kkt`
into `$CARGO_HOME/bin` (or `~/.cargo/bin`). Linux builds use GNU libc on
Ubuntu 24.04; use Ubuntu 24.04 or newer, or another distribution with a
compatible glibc. Windows binaries and a Homebrew tap are not published yet.
To build from source instead, use the pinned Rust toolchain and run
`cargo install --path . --locked`.

## Verify a download

The [releases](https://github.com/sam-mfb/kashkeshet/releases) contain one
`.tar.xz` archive and a SHA-256 checksum per target, a combined `sha256.sum`,
and the installer. Each archive contains the `kkt` executable.

For example, download and verify the Linux x86_64 archive before unpacking
it (replace the example version with a published release):

```sh
VERSION=v0.1.0
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
owner-authorized stable `vX.Y.Z` tag triggers publishing; pushing a branch
does not. Pull requests run `dist plan` without creating a release. Do not
push the first release tag without Sam's approval.

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

Run the archive check from a clean committed tree; a local build of edited
sources deliberately records `-dirty` and fails that check. `dist build
--artifacts=global` builds the installer after the native artifacts exist.

`.github/workflows/release.yml` starts from dist's generated workflow but
has maintained changes for the stable tag filter, locked Cargo wrapper,
release-check tests, and packaged provenance check. `allow-dirty = ["ci"]`
in the dist config permits those edits. When updating dist, temporarily
remove that setting, run `dist generate --mode ci`, then restore the setting
and those marked workflow changes. Review the diff and rerun the checks.
The commented Homebrew settings in `Cargo.toml` document how to enable a
tap later; no tap repository or publishing credential is needed today.

