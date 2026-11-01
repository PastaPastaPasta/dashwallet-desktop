# Verifying a download

Every file the release workflow publishes carries a GitHub build attestation: a signed statement, made by
GitHub Actions for this repository, that the file was built by `.github/workflows/release.yml` from a given
commit. This replaces PGP signatures; there is no signing key until 1.0.

It proves **where and from what the file was built**, not that the code is safe. The app is a pre-release with
no security review: use small amounts only.

## Check a file

You need the [GitHub CLI](https://cli.github.com) (`gh`) and a GitHub login (`gh auth login`, or a `GH_TOKEN`): the
command asks GitHub for the attestation, even for a public repository, and any ordinary account will do. The
commands below were tested with gh 2.102.0. `gh attestation verify` appeared in 2.49, but the filters used here
(`--signer-workflow`, `--source-ref`, `--source-digest`) came in later releases and the exact version was not
checked: if `gh attestation verify --help` does not list a flag, update gh (or drop the flag and compare the
workflow, ref and commit in the `--format json` output yourself).

```sh
gh attestation verify dash-wallet-0.1.0-macos-universal.dmg --repo PastaPastaPasta/dashwallet-desktop
```

That alone accepts an attestation from any workflow run of the repository, including a branch build. To require
the release workflow **and** the ref it ran from, add both filters. For a build of `main` (every
`DashWallet-continuous-…` file and every `v<version>-dev.<run>+<sha>` release):

```sh
gh attestation verify <file> --repo PastaPastaPasta/dashwallet-desktop \
  --signer-workflow PastaPastaPasta/dashwallet-desktop/.github/workflows/release.yml \
  --source-ref refs/heads/main
```

For a tagged release use the tag, for example `--source-ref refs/tags/v0.1.0` (`--signer-workflow` alone checks
the workflow file, not the branch or tag it ran on).

It prints the workflow, the commit and the run that built the file, and exits non-zero when the file's digest has
no attestation from this repository (or none that satisfies the filters). The same works for every other file (`.exe`, `.AppImage`, `.deb`) and for
`SHA256SUMS`. Verification goes by the file's contents, so a renamed copy verifies too, and so does
`DashWallet-continuous-macos-universal.dmg` from the rolling `continuous` release: it is the same file as the
per-build one.

To see exactly what was attested: `gh attestation verify <file> --repo PastaPastaPasta/dashwallet-desktop --format json`.
To pin a file to a commit, add `--source-digest <full commit sha>`.

## Check SHA256SUMS

Each release has `SHA256SUMS` (file names only). It is itself attested, so verify it first, then check the files
against it:

```sh
gh attestation verify SHA256SUMS --repo PastaPastaPasta/dashwallet-desktop
sha256sum -c --ignore-missing SHA256SUMS        # Linux (and Git Bash)
grep <file name> SHA256SUMS | shasum -a 256 -c  # macOS, one file
```

`--ignore-missing` skips the downloads you did not fetch. The `continuous` release has its own `SHA256SUMS`, which
lists its `DashWallet-continuous-…` names.

## What you are getting

- **Per-build releases** are named `v<version>-dev.<run>+<short commit>`: one for each push to `main` (a burst of
  pushes can skip one), never edited.
- **`continuous`** is replaced by every push to `main`, never by an older commit; its files keep the same names.
  After a burst of pushes it can sit one commit behind until the next push; its notes say which commit it holds.
  While it is being replaced (tens of seconds) a file can be briefly missing, or newer than its `SHA256SUMS`:
  if a download fails or the checksum does not match, fetch the file and `SHA256SUMS` again a minute later.
- **Tagged releases** (`v0.1.0`) are made by the same workflow from the tag.
- All of them are marked pre-release. macOS builds are ad-hoc signed and not notarized, Windows builds are unsigned
  (see the README for the first-launch warnings).

A branch or pull-request build is only a CI artifact on its Actions run and is never attested or published.
