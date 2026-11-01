# Verifying a download

Every file the release workflow publishes carries a GitHub build attestation: a signed statement, made by
GitHub Actions for this repository, that the file was built by `.github/workflows/release.yml` from a given
commit. This replaces PGP signatures; there is no signing key until 1.0.

It proves **where and from what the file was built**, not that the code is safe. The app is a pre-release with
no security review: use small amounts only.

## Check a file

You need the [GitHub CLI](https://cli.github.com) (`gh`, 2.49 or newer). Nothing else, and no login for a public
repository:

```sh
gh attestation verify dash-wallet-0.1.0-macos-universal.dmg --repo PastaPastaPasta/dashwallet-desktop
```

It prints the workflow, the commit and the run that built the file, and exits non-zero when the file's digest has
no attestation from this repository. The same works for every other file (`.exe`, `.AppImage`, `.deb`) and for
`SHA256SUMS`. Verification goes by the file's contents, so a renamed copy verifies too, and so does
`DashWallet-continuous-macos-universal.dmg` from the rolling `continuous` release: it is the same file as the
per-build one.

To see exactly what was attested: `gh attestation verify <file> --repo PastaPastaPasta/dashwallet-desktop --format json`.
To pin a release to a commit, add `--source-digest sha1:<full commit sha>` (or `--source-ref refs/tags/v0.1.0`
for a tagged release).

## Check SHA256SUMS

Each release has `SHA256SUMS` (file names only). It is itself attested, so verify it first, then check the files
against it:

```sh
gh attestation verify SHA256SUMS --repo PastaPastaPasta/dashwallet-desktop
sha256sum -c --ignore-missing SHA256SUMS        # on macOS: shasum -a 256 -c SHA256SUMS
```

`--ignore-missing` skips the downloads you did not fetch. The `continuous` release has its own `SHA256SUMS`, which
lists its `DashWallet-continuous-…` names.

## What you are getting

- **Per-build releases** are named `v<version>-dev.<run>+<short commit>`: one for each push to `main`, never edited.
- **`continuous`** is replaced by every push to `main`; its files keep the same names.
- **Tagged releases** (`v0.1.0`) are made by the same workflow from the tag.
- All of them are marked pre-release. macOS builds are ad-hoc signed and not notarized, Windows builds are unsigned
  (see the README for the first-launch warnings).

A branch or pull-request build is only a CI artifact on its Actions run and is never attested or published.
