# Verifying a download

Every file the release workflow publishes carries a GitHub build attestation: a signed statement, made by
GitHub Actions for this repository, that the file was built or assembled by `.github/workflows/release.yml`
(or `.github/workflows/release-publish-signed.yml` for detached-signed stable releases) from a given commit.
Signed stable releases (starting with 1.0) also carry a detached PGP signature (`SHA256SUMS.asc`), Apple
Developer ID signing and notarization on macOS, and Windows Authenticode signing. Release candidates
(`vX.Y.Z-rc.N`), `main` builds, and `continuous` ship keyless: attestations only, with ad-hoc macOS and
unsigned Windows installers.

An attestation proves **where and from what the file was built**, not that the code is safe. For early
releases without a completed security review, use small amounts only.

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
the workflow file, not the branch or tag it ran on). Signed stable 1.0+ releases published via the detached-sigs
workflow use `--signer-workflow PastaPastaPasta/dashwallet-desktop/.github/workflows/release-publish-signed.yml`
for the final published assets (while the unsigned CI inputs carry attestations from `release.yml`).

It prints the workflow, the commit and the run that built the file, and exits non-zero when the file's digest has
no attestation from this repository (or none that satisfies the filters). The same works for every other file (`.exe`, `.AppImage`, `.deb`, `.flatpak`, the two SBOMs) and for
`SHA256SUMS`. Verification goes by the file's contents, so a renamed copy verifies too, and so does
`DashWallet-continuous-macos-universal.dmg` from the rolling `continuous` release: it is the same file as the
per-build one.

To see exactly what was attested: `gh attestation verify <file> --repo PastaPastaPasta/dashwallet-desktop --format json`.
To pin a file to a commit, add `--source-digest <full commit sha>`.

## Check SHA256SUMS and its PGP signature

Each release has `SHA256SUMS` (file names only). It is itself attested, so verify it first, then check the files
against it:

```sh
gh attestation verify SHA256SUMS --repo PastaPastaPasta/dashwallet-desktop
sha256sum -c --ignore-missing SHA256SUMS        # Linux (and Git Bash)
grep <file name> SHA256SUMS | shasum -a 256 -c  # macOS, one file
```

`--ignore-missing` skips the downloads you did not fetch. The `continuous` release has its own `SHA256SUMS`, which
lists its `DashWallet-continuous-…` names.

Signed stable releases (1.0 and later) also publish a detached armored PGP signature,
`SHA256SUMS.asc`, which is attested alongside `SHA256SUMS`. Import the release public key named in the release
notes, confirm its full 40-character primary key fingerprint matches the release notes, and verify
`SHA256SUMS.asc` before checking file hashes:

```sh
gpg --import <release-public-key.asc>
gpg --fingerprint <PRIMARY_FINGERPRINT>         # compare every character against the release notes
gpg --verify SHA256SUMS.asc SHA256SUMS
```

Look for `gpg: Good signature from …` and confirm that `Primary key fingerprint:` matches the fingerprint
published in the release notes. (Releases are signed by an online signing subkey while the primary key stays
offline, so `gpg` prints both the primary key fingerprint and the signing subkey fingerprint.)

## Check the macOS signature and notarization

On signed stable releases (1.0 and later), both the `.dmg` and `Dash Wallet.app` inside it are
signed with a Developer ID Application certificate and notarized by Apple. After copying `Dash Wallet.app` to
`/Applications`:

```sh
codesign --verify --deep --strict --verbose=2 "/Applications/Dash Wallet.app"
spctl -a -vv --type execute "/Applications/Dash Wallet.app"
xcrun stapler validate "/Applications/Dash Wallet.app"
```

You can also check the disk image before opening it:

```sh
codesign --verify --verbose=2 dash-wallet-<version>-macos-universal.dmg
xcrun stapler validate dash-wallet-<version>-macos-universal.dmg
```

`codesign` should report `valid on disk` and `satisfies its Designated Requirement`, `spctl` should report
`accepted` with `source=Notarized Developer ID`, and `stapler validate` should report `The validate action worked!`.

**Keyless candidates and `main` builds:** pre-1.0 release candidates (`-rc.N` without signing), per-build `main`
releases, and `continuous` are ad-hoc signed (`Signature=adhoc`) and not notarized. On first launch, macOS
Gatekeeper blocks the app until you allow it explicitly (right-click the app ▸ **Open**, or allow it under
**System Settings ▸ Privacy & Security ▸ Open Anyway**; see the release notes' Install section).

## Check the Windows signature

On signed releases (1.0 and later), the NSIS installer (`dash-wallet-<version>-windows-x64-setup.exe`) carries
an Authenticode signature with an RFC 3161 timestamp made with Dash Core's code-signing certificate (following
Dash Core's detached-signatures model, [`SIGNING.md`](SIGNING.md)).

- **In File Explorer:** right-click the `.exe` ▸ **Properties** ▸ **Digital Signatures** tab ▸ select the
  signature ▸ **Details**. Confirm it says **This digital signature is OK** and shows a valid timestamp.
- **In PowerShell:**

```powershell
Get-AuthenticodeSignature .\dash-wallet-<version>-windows-x64-setup.exe |
  Format-List Status, StatusMessage, SignerCertificate, TimeStamperCertificate
```

`Status` must be `Valid`.

**Keyless candidates and `main` builds:** pre-1.0 release candidates (`-rc.N`), per-build `main`
releases, and `continuous` are unsigned (`Status: NotSigned`). On first launch, Windows SmartScreen shows
"Windows protected your PC"; click **More info** ▸ **Run anyway** to install (see the release notes' Install
section).

## Check an SBOM

Each release also carries two software bills of materials in CycloneDX JSON:
`dash-wallet-<version>-sbom-rust.cdx.json` lists the Rust crates the app is built from (every platform's
dependencies), and `dash-wallet-<version>-sbom-npm.cdx.json` the renderer's declared production npm dependencies
(`package.json` `dependencies` and the packages they pull in). The npm one does not list build tools such as Vite,
even where they add a little code to the bundle (Vite's module-preload polyfill). The `continuous` release has
them as `DashWallet-continuous-sbom-rust.cdx.json` and `DashWallet-continuous-sbom-npm.cdx.json`.
They are made by the same workflow run as the bundles
(`ci/github/release-sbom.sh`), listed in `SHA256SUMS` and attested like every other file, so they are checked the
same way:

```sh
gh attestation verify dash-wallet-0.1.0-sbom-rust.cdx.json --repo PastaPastaPasta/dashwallet-desktop \
  --signer-workflow PastaPastaPasta/dashwallet-desktop/.github/workflows/release.yml \
  --source-ref refs/tags/v0.1.0
sha256sum -c --ignore-missing SHA256SUMS
```

(0.1.0 itself has no SBOMs; they start with the next release. The names show the pattern.) A passing check
proves the SBOM came from the release workflow run on that ref and commit, the same run as the bundles of that
release: compare the commit `--format json` prints for the SBOM and for a bundle. It does not prove the SBOM is
complete; it lists what Cargo and the pnpm lockfile resolve, not code vendored by hand. To read one:
`jq -r '.components[] | "\(.name) \(.version)"' <file>`.

## What you are getting

- **Per-build releases** are named `v<version>-dev.<run>+<short commit>`: one for each push to `main` (a burst of
  pushes can skip one), never edited. Since 11-25 a push to `main` builds nothing, so no new ones appear.
- **`continuous`** was replaced by every push to `main` until 11-25, never by an older commit; its files keep the
  same names. It now stays at the last merge built before then; its notes say which commit it holds.
  While it is being replaced (tens of seconds) a file can be briefly missing, or newer than its `SHA256SUMS`:
  if a download fails or the checksum does not match, fetch the file and `SHA256SUMS` again a minute later.
- **Tagged releases** (`vX.Y.Z` and `vX.Y.Z-rc.N`) are made by the same workflow from the tag. Final 1.0+
  releases are full releases and ship signed (PGP `SHA256SUMS.asc`, macOS Developer ID + notarization,
  Windows Authenticode, plus GitHub build attestations) once detached signatures are attached (enforced by
  default — stable tags fail closed). Release candidates (`-rc.N`), `main` builds, and `continuous` are prereleases
  and ship keyless: ad-hoc signed on macOS and unsigned on Windows (see the release notes and README for the
  first-launch warnings).

A branch or pull-request build is only a CI artifact on its Actions run and is never attested or published.
