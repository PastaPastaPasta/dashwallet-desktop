# Signing a release candidate

## The signer's checklist

Only the release signer can do these four things, because each one needs their keys or admin rights on their
account. Do (a) to (c) before the cut on 11-27; (d) is the cut itself. The rest of this file explains each step.

- [ ] **a. Pin the public signing identities (one pull request to `main`).** These values are public, and nobody
  else fills them in. On the Mac with the keys:

  ```sh
  cd ~/src/dashwallet-desktop && git fetch origin && git checkout -b release/signing-identities origin/main
  gpg --list-secret-keys --keyid-format long --with-subkey-fingerprints   # the 40-hex primary fingerprint
  security find-identity -v -p codesigning     # the 10-character team ID, in parentheses after the name
  ykman piv certificates export 9c - | openssl x509 -noout -fingerprint -sha256 |
    sed 's/.*=//; s/://g' | tr 'A-F' 'a-f' | sed 's/^/sha256:/'          # slot 9a if ykman piv info says so
  export PGP_FPR=<40 hex> TEAM_ID=<10 chars> WIN_LEAF=<sha256:64 hex>
  jq --arg f "$PGP_FPR" --arg t "$TEAM_ID" --arg w "$WIN_LEAF" \
    '.pgp_fingerprint = $f | .apple_team_id = $t | .windows_leaf_sha256 = $w' \
    docs/release/signing-identities.json > ids.json && mv ids.json docs/release/signing-identities.json
  gpg --armor --export "$PGP_FPR" > docs/release/release-key.asc
  gpg --show-keys --with-fingerprint docs/release/release-key.asc   # the same primary fingerprint
  git add docs/release/signing-identities.json docs/release/release-key.asc
  git commit -m "release: pin the release signing identities" && git push -u origin HEAD
  gh pr create --base main --title "release: pin the release signing identities" --body "Public signing identities for 1.0."
  ```

  The jq checks in [Public identity values](#public-identity-values-the-signer-fills-in) each print `true`.
- [ ] **b. Create the detached-signatures repository** `PastaPastaPasta/dashwallet-detached-sigs`. It must be public,
  because `release-publish-signed.yml` fetches it anonymously, and it needs a `main` branch:

  ```sh
  gh repo create PastaPastaPasta/dashwallet-detached-sigs --public --add-readme \
    --description "Detached release signatures for Dash Wallet Desktop"
  gh repo edit PastaPastaPasta/dashwallet-detached-sigs --enable-issues=false --enable-wiki=false --enable-projects=false
  ```

  Or on github.com: **+ ▸ New repository**, name `dashwallet-detached-sigs`, **Public**, tick **Add a README**,
  **Create repository**; then **Settings ▸ General ▸ Features**, untick Issues, Wikis and Projects.
- [ ] **c. Opt candidates in to signing** before the tag is pushed:
  `gh variable set DW_SIGN_RC --body 1 --repo PastaPastaPasta/dashwallet-desktop`. Or on github.com:
  **dashwallet-desktop ▸ Settings ▸ Secrets and variables ▸ Actions ▸ Variables ▸ New repository variable**, name
  `DW_SIGN_RC`, value `1`. `gh variable list --repo PastaPastaPasta/dashwallet-desktop` then shows
  `DW_SIGN_RC  1`.
- [ ] **d. The signing session** on the Mac with the YubiKey, after the version bump and (a) are on `main`:
  [one-time setup](#2-one-time-setup-on-the-signers-mac) (once), then
  [cut the tag](#3-cut-the-candidate-one-tag-push-builds-everything),
  [sign](#4-sign-each-artifact-on-the-mac), [store the signatures](#5-store-the-detached-signatures) and
  [publish](#6-publish-verify-attach-attest-release-publish-signedyml). If it can't finish, use the
  [keyless fallback](#fallback-publish-the-waiting-candidate-keyless).

Optional, in the same Settings visit (not needed for the candidate): the `release` environment
(**Settings ▸ Environments ▸ release**) still holds the old `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets and has no required reviewer. Delete both secrets (neither workflow reads
them) and add yourself under **Required reviewers**.

The release signer signs a release candidate `vX.Y.Z-rc.N` on their own Mac, the same way as a stable release
([`SIGNING.md`](SIGNING.md), the Dash Core detached-signatures model). CI builds and attests the unsigned files. The
signer signs them locally with Dash Core's Apple Developer ID, the Windows code-signing certificate on their
YubiKey, and their PGP key, then commits the small detached signatures to the detached-signatures repository.
`release-publish-signed.yml` re-attaches them to CI's exact bytes, verifies every signature against the
identities pinned in this repository, attests the result and publishes the candidate. No key, certificate, PIN or
password goes into GitHub Actions, and no agent handles any of them.

Candidates stay keyless by default. A candidate takes the signed path only when the repository variable
`DW_SIGN_RC` is set to exactly `1` **before its tag is pushed** (`true` or `yes` mean keyless). Without it, the tag publishes the 8 keyless assets at once, as
before.

The examples use `v1.0.0-rc.1`. Substitute the real tag everywhere.

## What a signed candidate publishes

A signed candidate is a prerelease, never Latest, with 9 assets, all attested by `release-publish-signed.yml`:

| Asset | Signed how | Credential (role) |
|---|---|---|
| `dash-wallet-1.0.0-rc.1-macos-universal.dmg` and the `.app` inside it | Developer ID Application codesign, hardened runtime, notarized and stapled | Dash Core Developer ID identity in the signer's login Keychain; notarization Keychain profile |
| `dash-wallet-1.0.0-rc.1-windows-x64-setup.exe` | Authenticode, SHA-256, RFC 3161 timestamp | Dash Core Windows code-signing certificate and key in a YubiKey PIV slot; PIV PIN |
| `dash-wallet-1.0.0-rc.1-linux-x86_64.AppImage`, `-linux-amd64.deb`, `-linux-x86_64.flatpak` | Unchanged CI bytes, listed in `SHA256SUMS` | (covered by `SHA256SUMS.asc`) |
| `dash-wallet-1.0.0-rc.1-sbom-rust.cdx.json`, `-sbom-npm.cdx.json` | Unchanged CI bytes, listed in `SHA256SUMS` | (covered by `SHA256SUMS.asc`) |
| `SHA256SUMS` | Lists the 7 files above | — |
| `SHA256SUMS.asc` | Detached armored PGP signature over `SHA256SUMS` | Signer's release PGP key (signing subkey, e.g. on a YubiKey OpenPGP applet) |

The Linux bundles get no signature file of their own, as in Dash Core: one `SHA256SUMS.asc` covers them through
their hashes in `SHA256SUMS`. The publish flow accepts exactly these 9 files and refuses any other.

**No updater feed (`latest.json`) for a candidate, and no minisign step.** `ci/release/sign-updater.sh` refuses a
candidate version. There are three reasons:

1. A candidate is a prerelease and never Latest. The app reads the feed from `/releases/latest/download/latest.json`,
   so a candidate's feed would never be served.
2. Candidate builds embed no updater endpoint: `DW_ENABLE_UPDATER=1` is passed only for a signed stable tag, and
   `plugins.updater.endpoints` is `[]` in `tauri.conf.json`.
3. `plugins.updater.pubkey` in `tauri.conf.json` is still the retired CI key `ED322B963B527B21`, which must never
   sign a feed. Rotating it to the offline stable updater key is a 1.0 blocker ([`SIGNING.md`](SIGNING.md) item 6),
   not an RC step.

## 1. Before the cut: what must already be on `main` at the candidate's commit

The publish workflow reads the pinned identities, the scripts and the version from the tagged commit. Each of these
lands by pull request before the tag.

- [ ] **This change** (the `DW_SIGN_RC` opt-in) is merged. On an older commit the candidate can only publish keyless.
- [ ] **Version bump** ([`CHECKLIST.md`](CHECKLIST.md) §2). `ci/github/release-version.sh` fails every job of a
      `v1.0.0-rc.1` run while `rust/crates/dw-app/tauri.conf.json` says anything but `1.0.0` (`main` says `0.3.0`
      today). Bump `tauri.conf.json`, `rust/Cargo.toml` (`[workspace.package] version`, then
      `cargo metadata --manifest-path rust/Cargo.toml --format-version 1 > /dev/null` to rewrite `rust/Cargo.lock`),
      `apps/desktop/package.json` and `Apps/macOS/project.yml` to `1.0.0` in one pull request. The candidate tag
      takes the same `1.0.0`; later candidates and the final tag need no further bump. A candidate build still shows
      the "pre-release test build" warning, because its build version is `1.0.0-rc.1`, not `1.0.0`.
- [ ] **Pinned identities** (the table in [Public identity values](#public-identity-values-the-signer-fills-in)).
      `docs/release/signing-identities.json` and `docs/release/release-key.asc` hold placeholders today, and
      `release-attach.sh` refuses to publish until they hold the real public values. Optionally,
      `docs/release/windows-ca.pem` holds the Windows CA root and intermediates.
- [ ] **Release notes.** `docs/release/1.0.0-notes.md` in the signed wording (`SHA256SUMS.asc` in Files, no
      Gatekeeper or SmartScreen bypass steps, Verify against `release-publish-signed.yml`) and with nothing about
      `latest.json` or in-app updates for a candidate. It lands with the version bump (#872). If a candidate is
      published keyless instead, put back the "Keyless candidate" comments in that file first.
      `release-publish.sh` uses it as the body of every `v1.0.0-rc.N`.

Once, in GitHub (the signer, with admin rights on the repository):

- [ ] Create the public detached-signatures repository (`release-publish-signed.yml` fetches it anonymously over
      HTTPS):
      `gh repo create PastaPastaPasta/dashwallet-detached-sigs --public --add-readme --description "Detached release signatures for Dash Wallet Desktop"`
- [ ] Environment `release` (Settings ▸ Environments): **zero secrets**, deployment rule limited to tags `v*` (this
      covers `-rc.N`), the signer as required reviewer. If `TAURI_SIGNING_*`, `APPLE_*`, `AZURE_*` or `RELEASE_PGP_*`
      secrets are still there from before #780, delete them ([`SIGNING.md`](SIGNING.md) item 6). Neither workflow
      reads them.
- [ ] Opt candidates in: `gh variable set DW_SIGN_RC --body 1 --repo PastaPastaPasta/dashwallet-desktop`. Check it
      with `gh variable list --repo PastaPastaPasta/dashwallet-desktop`, which should list `DW_SIGN_RC  1`. To return
      to keyless candidates later, run `gh variable delete DW_SIGN_RC --repo PastaPastaPasta/dashwallet-desktop`.

## 2. One-time setup on the signer's Mac

### Tools

```sh
xcode-select -p                     # expect /Applications/Xcode.app/... or /Library/Developer/CommandLineTools
xcode-select --install              # only if the line above failed (codesign, notarytool, stapler, hdiutil)
brew install osslsigncode libp11 yubico-piv-tool ykman opensc gnupg gh jq
osslsigncode --version              # expect 2.14 or newer, "using: OpenSSL 3.x/4.x"
gh auth status                      # expect "Logged in to github.com" (read access is enough to fetch; the
                                    # dispatch in step 6 needs Actions write)
git clone https://github.com/PastaPastaPasta/dashwallet-desktop ~/src/dashwallet-desktop
```

Run every script from a checkout of the candidate tag (step 3), so the scripts match the build being signed.

### Apple: Developer ID identity and notarization profile

```sh
security find-identity -v -p codesigning
```

Expected output includes one line such as
`1) <40-hex SHA-1> "Developer ID Application: <Dash Core team name> (<TEAM_ID>)"` and
`N valid identities found`. Copy the quoted name exactly: it is `--identity` in step 4.2. The 10 characters in
parentheses are the Apple team ID for `signing-identities.json`.

Store the notarization credentials once in the login Keychain. Leave out `--password`, so notarytool prompts for the
app-specific password and it stays out of shell history:

```sh
xcrun notarytool store-credentials dash-notary --apple-id "<Apple ID of the Dash Core developer account>" --team-id "<TEAM_ID>"
# expect: "Validating your credentials...", "Success. Credentials validated." and "Credentials saved to Keychain."
xcrun notarytool history --keychain-profile dash-notary
# expect: a table of past submissions, or "No submission history." (either proves the profile works)
```

An App Store Connect API key works instead. Pass `--api-key-path`, `--api-key-id` and `--api-issuer` to
`sign-macos.sh` in place of `--notary-profile`.

### YubiKey PIV: Windows code-signing certificate

```sh
ykman list                          # expect one line: "YubiKey 5 ... Serial: <n>"
ykman piv info
```

Expected `ykman piv info` output: `PIV version`, `PIN tries remaining`, and a `Slot 9C (SIGNATURE)` or
`Slot 9A (AUTHENTICATION)` section whose `Subject DN` is Dash Core's code-signing subject and whose `Issuer DN` is
its CA. Note which slot holds it. Check the certificate and compute its pinned hash:

```sh
SLOT=9c                             # or 9a, from ykman piv info
ykman piv certificates export "$SLOT" - | openssl x509 -noout -subject -issuer -enddate -fingerprint -sha256
# windows_leaf_sha256 for signing-identities.json, in the exact form the pin expects:
ykman piv certificates export "$SLOT" - | openssl x509 -noout -fingerprint -sha256 |
  sed 's/.*=//; s/://g' | tr 'A-F' 'a-f' | sed 's/^/sha256:/'
```

Find the PKCS#11 module and provider paths, and the object ID that `sign-windows.sh` takes as a PKCS#11 URI:

```sh
export YKCS11="$(brew --prefix yubico-piv-tool)/lib/libykcs11.dylib"
export PKCS11PROV="$(brew --prefix libp11)/lib/ossl-modules/pkcs11prov.dylib"
ls -l "$YKCS11" "$PKCS11PROV"       # both must exist (pkcs11prov may end in .so; sign-windows.sh finds either)
pkcs11-tool --module "$YKCS11" --list-slots
# expect: "Slot 0 (0x0): Yubico YubiKey ..." and "token label : YubiKey PIV #<serial>"
pkcs11-tool --module "$YKCS11" --list-objects --type cert
# expect a "Certificate Object; type = X.509 cert" whose "ID:" is the slot's ID below
```

libykcs11 maps PIV slots to IDs as follows: `9a` → `01`, `9c` → `02`, `9d` → `03`, `9e` → `04`. For slot `9c` the URIs
are `pkcs11:id=%02;type=cert` and `pkcs11:id=%02;type=private`.

libykcs11 can also list a "PIV Attestation" certificate for the slot. Check how many certificates carry the ID:
`pkcs11-tool --module "$YKCS11" --list-objects --type cert` should show exactly one with `ID: 02`. If it shows more,
pin the code-signing certificate by its label as well, using the `label:` that `pkcs11-tool` printed (URL-encoded),
for example `pkcs11:id=%02;object=X.509%20Certificate%20for%20Digital%20Signature;type=cert`. A wrong certificate
fails closed later, in `osslsigncode verify` and in the pinned leaf hash, but it costs a signing round.

Save the CA's intermediate certificates (PEM) next to your other signing material, for example
`~/certs/dashcore-win-chain.pem`, and check what it contains:

```sh
openssl crl2pkcs7 -nocrl -certfile ~/certs/dashcore-win-chain.pem | openssl pkcs7 -print_certs -noout
# expect: the intermediate CA subject(s) that issued the YubiKey certificate
```

Use the CA's RFC 3161 timestamp URL (`sign-windows.sh` defaults to `http://timestamp.digicert.com`; Sectigo is
`http://timestamp.sectigo.com`).

### PGP release key

```sh
gpg --card-status                   # if the key lives on a YubiKey OpenPGP applet: shows "Signature key ...."
gpg --list-secret-keys --keyid-format long --with-subkey-fingerprints
# expect: "sec" (or "sec#" when the primary is offline) and an "ssb>" / "ssb" signing subkey [S]
export PGP_FPR=<40-hex primary key fingerprint, no spaces>
gpg --fingerprint "$PGP_FPR"        # the primary fingerprint, printed in groups of four
```

## 3. Cut the candidate: one tag push builds everything

Prerequisites: everything in section 1 is on `main`, and `DW_SIGN_RC` is `1`. Pick the commit (it must be on
`main`; see [`CHECKLIST.md`](CHECKLIST.md) §1). From `~/src/dashwallet-desktop`:

```sh
export REPO=PastaPastaPasta/dashwallet-desktop TAG=v1.0.0-rc.1 VERSION=1.0.0-rc.1
export RC_SHA=<full 40-hex commit on main>
git fetch origin && git merge-base --is-ancestor "$RC_SHA" origin/main && echo "on main"
gh variable list --repo "$REPO" | grep -w DW_SIGN_RC           # expect: DW_SIGN_RC  1
git tag -s -u "$PGP_FPR" "$TAG" "$RC_SHA" -m "Dash Wallet Desktop $VERSION" && git tag -v "$TAG" && git push origin "$TAG"
```

The tag push is the whole rebuild recipe. `git tag -v` should print `gpg: Good signature` and the
`Primary key fingerprint`. Push the tag once and never move it. To build the same commit without publishing
(a dry run), push `v0.0.0-rel<N>` instead ([`CHECKLIST.md`](CHECKLIST.md) §5). That builds every bundle, but it does
not pack the signing inputs and does not attest.

Then check out the tag (the scripts you run must be the tagged ones) and wait for the run. It takes one macOS
build job and one macOS script-test job; macOS runners are scarce, so don't re-run it without reason:

```sh
git checkout --detach "$TAG"
export TAG_SHA=$(git rev-parse "$TAG^{commit}")
RUN_ID=; until [[ -n "$RUN_ID" ]]; do sleep 10; RUN_ID=$(gh run list --repo "$REPO" --workflow release.yml --branch "$TAG" --event push --json databaseId,headSha --jq ".[] | select(.headSha == \"$TAG_SHA\") | .databaseId" | head -n 1); done
export RUN_ID && echo "RUN_ID=$RUN_ID"
gh run watch "$RUN_ID" --repo "$REPO" --exit-status
```

When it succeeds, the `publish` job shows the notice `Tag v1.0.0-rc.1 holds unsigned binaries until detached
signatures are attached`, and **no release exists yet** (`gh release view "$TAG" --repo "$REPO"` says `release not
found`). The run has two artifacts, and every file in them is attested by `release.yml` for the tag and commit:

- `release-v1.0.0-rc.1`: the 7 unsigned files (universal `.dmg`, NSIS `.exe`, AppImage, `.deb`, Flatpak, two SBOMs)
  and the unsigned `SHA256SUMS`;
- `unsigned-signing-inputs-v1.0.0-rc.1`: `dash-wallet-1.0.0-rc.1-macos-unsigned.tar.gz` (the unsigned `.app`) and
  `unsigned.SHA256SUMS` (all 8 build files).

If the release appeared at once with 8 keyless assets, `DW_SIGN_RC` was not `1` when the run started. That
candidate stays keyless (never re-push a tag). Set the variable and cut `-rc.2`.

### Fallback: publish the waiting candidate keyless

The candidate waits, unpublished, until step 6 succeeds. If signing can't finish in time (identities not on
`main` at the tag, a YubiKey or notarization problem), either:

- publish this tag keyless: `gh variable delete DW_SIGN_RC --repo "$REPO"`, then in the tag's Release run use
  **Re-run all jobs** (`gh run rerun "$RUN_ID" --repo "$REPO"`). `decide` reads the variable again, and the run
  rebuilds and publishes the 8 keyless assets. It costs a full rebuild, including one macOS job; or
- cut the next candidate: leave this tag unpublished and push `-rc.N+1`, with or without the variable.

The pinned identities and scripts come from the tagged commit, so a fix merged after the tag never helps that
tag: it needs a new candidate.

## 4. Sign each artifact on the Mac

```sh
export WORK="$HOME/dashwallet-release-work/$TAG"
export UNSIGNED="$WORK/unsigned" SIGS="$WORK/sigs" OUT="$WORK/out"
rm -rf "$WORK" && mkdir -p "$OUT"
```

### 4.1 Fetch and verify the unsigned build

Credential: `gh` login (read).

```sh
ci/release/fetch-unsigned.sh --tag "$TAG" --tag-sha "$TAG_SHA" --run-id "$RUN_ID" --repo "$REPO" \
  --unsigned-dir "$UNSIGNED" --sigs-dir "$SIGS"
```

The script refuses if the remote tag moved, if the run built another commit, or if any file fails
`gh attestation verify` or `unsigned.SHA256SUMS`. Expected output: `✓ Verification succeeded!` for each of the
10 files, `: OK` for each line of `unsigned.SHA256SUMS`, and finally
`fetch-unsigned: verified 10 attested unsigned files for v1.0.0-rc.1 (<TAG_SHA>) into …/unsigned`.

### 4.2 macOS: codesign, notarize, staple

Credential: the Developer ID Application identity in the login Keychain (macOS may ask to allow `codesign` to use
the key: choose Always Allow), and the `dash-notary` profile. It needs the network (Apple timestamp and notary
service). Notarization usually takes minutes; the script waits up to 60 minutes per submission.

```sh
ci/release/sign-macos.sh --version "$VERSION" --unsigned-dir "$UNSIGNED" --sigs-dir "$SIGS" --out-dir "$OUT" \
  --identity "Developer ID Application: <exact name from security find-identity>" --notary-profile dash-notary
```

Expected output, in order:

- `codesign --verify`: `valid on disk` and `satisfies its Designated Requirement`;
- notarytool: JSON with `"status": "Accepted"` (twice: the `.app` zip, then the `.dmg`). On failure the script
  prints `notarytool log <id>` with Apple's reasons and stops;
- `The staple and validate action worked!` / `The validate action worked!`;
- `spctl --assess`: `accepted` and `source=Notarized Developer ID`;
- `sign-macos: done`, with the detached signatures in `$SIGS/osx/Dash Wallet.app` and the signed
  `$OUT/dist/dash-wallet-1.0.0-rc.1-macos-universal.dmg`. The updater archive line reads
  `(none: release candidate, no updater feed)`.

Verification anyone can run on a Mac (after download; the same commands work on the published file):

```sh
dmg="$OUT/dist/dash-wallet-$VERSION-macos-universal.dmg"
codesign --verify --verbose=2 "$dmg"                                   # valid on disk
xcrun stapler validate "$dmg"                                          # The validate action worked!
spctl -a -t open --context context:primary-signature -vv "$dmg"       # accepted, source=Notarized Developer ID
hdiutil attach "$dmg" -nobrowse -readonly -mountpoint /Volumes/dw-rc
codesign --verify --deep --strict --verbose=2 "/Volumes/dw-rc/Dash Wallet.app"
codesign -dv --verbose=4 "/Volumes/dw-rc/Dash Wallet.app" 2>&1 | grep -E '^(Authority|TeamIdentifier|Runtime)'
spctl -a -t execute -vv "/Volumes/dw-rc/Dash Wallet.app"              # accepted, source=Notarized Developer ID
hdiutil detach /Volumes/dw-rc
```

### 4.3 Windows: Authenticode from the YubiKey

Credential: the YubiKey with the code-signing certificate, plugged in. When osslsigncode asks for `Password`, type
the **PIV PIN**. Touch the key if its touch policy asks. It needs the network (the timestamp server).

```sh
ci/release/sign-windows.sh --version "$VERSION" --unsigned-dir "$UNSIGNED" --sigs-dir "$SIGS" --out-dir "$OUT" \
  --pkcs11-module "$YKCS11" \
  --pkcs11-cert "pkcs11:id=%02;type=cert" --pkcs11-key "pkcs11:id=%02;type=private" \
  --chain "$HOME/certs/dashcore-win-chain.pem" --tsa "http://timestamp.digicert.com"
```

(Use the slot's ID from section 2 in both URIs.) Expected output: `Succeeded` after signing, then
`sign-windows: extracting detached PEM signature …`, the byte-identical round-trip check (silent on success),
`Signature verification: ok` and `Succeeded` from `osslsigncode verify`, then `sign-windows: done`. The signed
installer is `$OUT/dist/dash-wallet-1.0.0-rc.1-windows-x64-setup.exe`, and the detached signature is
`$SIGS/win/dash-wallet-1.0.0-rc.1-windows-x64-setup.exe.pem`.

If signing fails with `CKR_USER_NOT_LOGGED_IN` or `PKCS11_get_private_key returned NULL`, check the object ID with
`pkcs11-tool --list-objects`. Then retry with OpenSC's module: `--pkcs11-module "$(brew --prefix opensc)/lib/opensc-pkcs11.so"`.
In OpenSC, slot 9c is also `id=%02`.

Verification anyone can run (Linux or macOS with osslsigncode; Windows with PowerShell):

```sh
osslsigncode verify -in "$OUT/dist/dash-wallet-$VERSION-windows-x64-setup.exe"
# expect "Signature verification: ok", the signer's subject, a "Timestamp:" line, "Succeeded"
osslsigncode verify -in dash-wallet-1.0.0-rc.1-windows-x64-setup.exe -require-leaf-hash "<windows_leaf_sha256 from signing-identities.json>"
```

```powershell
Get-AuthenticodeSignature .\dash-wallet-1.0.0-rc.1-windows-x64-setup.exe | Format-List Status, SignerCertificate, TimeStamperCertificate
# Status : Valid
```

### 4.4 Linux bundles and SBOMs: SHA256SUMS and its PGP signature

Credential: the release PGP key (its signing subkey; on a YubiKey, enter the OpenPGP PIN and touch if asked).

```sh
for s in linux-x86_64.AppImage linux-amd64.deb linux-x86_64.flatpak sbom-rust.cdx.json sbom-npm.cdx.json; do
  cp "$UNSIGNED/dash-wallet-$VERSION-$s" "$OUT/dist/"
done
ci/release/sign-checksums.sh --version "$VERSION" --dist-dir "$OUT/dist" --sigs-dir "$SIGS" --gpg-key "$PGP_FPR"
```

The script first re-checks the 5 copied files against `unsigned.SHA256SUMS`. It then writes `SHA256SUMS` over the 7
files of `$OUT/dist`, signs it into `SHA256SUMS.asc`, and verifies the signature. Expected output:
`gpg: Good signature from "<the key's user ID>"`, `Primary key fingerprint:` matching `$PGP_FPR`,
`sign-checksums: wrote and verified …`, and the 7 lines of `SHA256SUMS` (no `latest.json`, no `.app.tar.gz`).

Verification anyone can run (in a directory holding the downloaded files):

```sh
gpg --import release-key.asc        # docs/release/release-key.asc at the tag
gpg --verify SHA256SUMS.asc SHA256SUMS   # Good signature; Primary key fingerprint = pgp_fingerprint
sha256sum -c SHA256SUMS             # every line OK (macOS: shasum -a 256 -c SHA256SUMS)
```

### 4.5 Updater feed: skipped for a candidate

Don't run `sign-updater.sh`: it refuses `-rc.N`, and the candidate publishes no `latest.json` (see the top of this
file). `$SIGS` must have no `updater/` directory, even an empty one: `release-attach.sh` refuses a candidate that has
one.

## 5. Store the detached signatures

Credential: push access to `PastaPastaPasta/dashwallet-detached-sigs`.

`$SIGS` now holds only small files, and no binaries:

```text
unsigned.SHA256SUMS
osx/Dash Wallet.app/manifest.json
osx/Dash Wallet.app/Contents/MacOS/dash-wallet.arm64.cs
osx/Dash Wallet.app/Contents/MacOS/dash-wallet.x86_64.cs
osx/Dash Wallet.app/Contents/_CodeSignature/CodeResources
osx/Dash Wallet.app/Contents/CodeResources
win/dash-wallet-1.0.0-rc.1-windows-x64-setup.exe.pem
SHA256SUMS
SHA256SUMS.asc
```

```sh
git clone https://github.com/PastaPastaPasta/dashwallet-detached-sigs ~/src/dashwallet-detached-sigs 2>/dev/null || true
cd ~/src/dashwallet-detached-sigs && git fetch origin && git checkout -B "$TAG" origin/main
rm -rf "./$TAG" && mkdir "$TAG" && cp -R "$SIGS/." "$TAG/"
git add "$TAG" && git commit -S -m "Add detached signatures for $TAG" && git push origin "$TAG"
export SIGS_COMMIT=$(git rev-parse HEAD) && echo "$SIGS_COMMIT"
cd ~/src/dashwallet-desktop
```

## 6. Publish: verify, attach, attest (`release-publish-signed.yml`)

Credential: `gh` login with Actions write on the repository, and approval of the `release` environment.

Stage the signed `.dmg` in a draft release, which the workflow downloads and deletes only after a successful publish:

```sh
gh release create "staging-dmg-$TAG" "$OUT/dist/dash-wallet-$VERSION-macos-universal.dmg" \
  --repo "$REPO" --draft --title "Staging signed dmg for $TAG"
```

Rehearse first (`dry_run=true`). It runs every verification on `macos-15` and stops before attesting and publishing:

```sh
gh workflow run release-publish-signed.yml --repo "$REPO" --ref "$TAG" \
  -f tag="$TAG" -f release_run_id="$RUN_ID" \
  -f sigs_repo=PastaPastaPasta/dashwallet-detached-sigs -f sigs_ref="$SIGS_COMMIT" -f dry_run=true
sleep 15; gh run list --repo "$REPO" --workflow release-publish-signed.yml --event workflow_dispatch --limit 3
gh run watch "<the newest run's ID from the list above>" --repo "$REPO" --exit-status
```

Approve the `release` environment when GitHub asks. A green rehearsal ends with the notice
`dry_run=true on v1.0.0-rc.1 (…): all unsigned-input attestations, detached signatures, pinned identities, …
succeeded`. Its log shows `release-attach: all 7 final release files match SHA256SUMS` and
`release-attach: verified PGP signature SHA256SUMS.asc (GOODSIG + pinned primary key …)`.

Then publish (the same command without `dry_run`):

```sh
gh workflow run release-publish-signed.yml --repo "$REPO" --ref "$TAG" \
  -f tag="$TAG" -f release_run_id="$RUN_ID" \
  -f sigs_repo=PastaPastaPasta/dashwallet-detached-sigs -f sigs_ref="$SIGS_COMMIT"
```

The workflow checks that the tag is on `main` and that the run built it. It verifies every unsigned input's
attestation and runs `ci/github/release-attach.sh` against the identities pinned at the tag (Windows leaf hash, Apple
team ID and Developer ID OIDs, the PGP primary fingerprint and `release-key.asc`). It attests the 9 final files and
publishes the prerelease `v1.0.0-rc.1` with the body `docs/release/1.0.0-notes.md`. Then it deletes the staging draft.

If any step fails, nothing is published, and the staging draft and the detached signatures stay. Fix the cause and
dispatch again with the same inputs. If a signature must be redone, repeat section 4 in a fresh `$WORK`, commit the
new signatures (a new `$SIGS_COMMIT`), and replace the staged `.dmg`.

## 7. Verify the published candidate (anyone)

```sh
mkdir dw-rc && cd dw-rc
sha=$(gh api repos/PastaPastaPasta/dashwallet-desktop/commits/v1.0.0-rc.1 --jq .sha)
gh release download v1.0.0-rc.1 --repo PastaPastaPasta/dashwallet-desktop
ls | wc -l                                   # 9
gh release view v1.0.0-rc.1 --repo PastaPastaPasta/dashwallet-desktop --json isPrerelease --jq .isPrerelease   # true
for f in *; do
  gh attestation verify "$f" --repo PastaPastaPasta/dashwallet-desktop \
    --signer-workflow PastaPastaPasta/dashwallet-desktop/.github/workflows/release-publish-signed.yml \
    --source-ref refs/tags/v1.0.0-rc.1 --source-digest "$sha"
done
curl -fsSLO https://raw.githubusercontent.com/PastaPastaPasta/dashwallet-desktop/v1.0.0-rc.1/docs/release/release-key.asc
gpg --import release-key.asc && gpg --verify SHA256SUMS.asc SHA256SUMS && sha256sum -c SHA256SUMS
# macOS: shasum -a 256 -c SHA256SUMS
```

Then run the macOS (4.2) and Windows (4.3) verification commands on the downloaded `.dmg` and `.exe`, and the
installs and screenshots of [`CHECKLIST.md`](CHECKLIST.md) §4 and §7. On a real Mac a signed candidate opens
without the Gatekeeper right-click step, and on Windows the installer shows Dash Core as the publisher.

## Public identity values the signer fills in

These are public values. Only the signer fills them in, from their own devices, by pull request to `main` before the
candidate's tag. Nobody else invents or looks them up. The placeholders fail closed, so nothing publishes until they
are real.

| File | Field | Value (by role) | Where it comes from |
|---|---|---|---|
| `docs/release/signing-identities.json` | `pgp_fingerprint` | 40-hex primary fingerprint of the release PGP key | `gpg --list-secret-keys --with-subkey-fingerprints` (section 2) |
| `docs/release/signing-identities.json` | `apple_team_id` | 10-character Apple team ID of the Developer ID identity | the parentheses in `security find-identity -v -p codesigning` |
| `docs/release/signing-identities.json` | `windows_leaf_sha256` | `sha256:` + 64 lower-case hex of the YubiKey's code-signing leaf certificate | the `ykman piv certificates export … | openssl x509 -fingerprint -sha256` pipeline (section 2) |
| `docs/release/release-key.asc` | whole file | armored public key of that PGP key | `gpg --armor --export "$PGP_FPR" > docs/release/release-key.asc` |
| `docs/release/windows-ca.pem` (optional) | whole file | root and intermediate PEM certificates of the Windows code-signing CA | the CA's published certificates |

Check the edited file before committing:
`jq -e '.pgp_fingerprint|test("^[0-9A-Fa-f]{40}$")' docs/release/signing-identities.json`,
`jq -e '.apple_team_id|test("^[A-Z0-9]{10}$")' …`, and `jq -e '.windows_leaf_sha256|test("^sha256:[0-9a-f]{64}$")' …`.
Each should print `true`.
Then run `gpg --show-keys --with-fingerprint docs/release/release-key.asc`: the primary fingerprint it prints must equal
`pgp_fingerprint`.
