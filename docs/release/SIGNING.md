# Release signing runbook (stable 1.0+ — Dash Core detached-sigs model)

Stable releases of Dash Wallet Desktop follow the Dash Core signing model (`dashpay/dash` `contrib/` and
`dashpay/dash-detached-sigs`): **no Apple, Windows, PGP, or stable updater private keys ever enter GitHub
Actions.** CI builds unsigned artifacts and attests them; the release signer signs locally on macOS using
Dash Core's existing signing identities (macOS Developer ID keychain identity, YubiKey PIV code-signing
certificate, real PGP key, and local stable updater key), stores the small detached signatures in a
detached-signatures repository, and a zero-secret verification/attach step (`ci/github/release-attach.sh` /
`.github/workflows/release-publish-signed.yml`) verifies, re-attaches, attests, and publishes the final
release.

Keyless release candidates (`vX.Y.Z-rc.N`) remain untouched: CI builds and publishes the 8 unsigned assets
with GitHub Sigstore build provenance attestations only (no `.asc`, no `latest.json`, nothing required even when
`DW_SIGNING_REQUIRED=1`). The existing `main` per-build prereleases and `continuous` (no longer published since
merges stopped starting Release on 11-25) are unsigned the same way. A candidate opts in to this signed path
with the repository variable `DW_SIGN_RC=1` set before its tag is pushed; it then publishes 9 assets (signed
`.dmg` and `.exe`, `SHA256SUMS.asc`, no updater feed). The runbook for that is [`RC-SIGNING.md`](RC-SIGNING.md).

> **Nightly track:** automated nightly signing (separate macOS Developer ID cert + revocable API key in CI,
> unsigned Windows, separate CI updater key, separate bundle ID and app name `"Dash Wallet Nightly"` for
> side-by-side install, plus cross-channel key-rejection tests) is tracked in issue #781 as a follow-up PR
> after the RC so this PR stays focused on the stable release path.

---

## Reproducibility status (plain statement)

**None of the CI build targets are byte-for-byte reproducible today** (there is no Guix or hermetic build
container yet):

| Target | Reproducible today? | Why |
|---|---|---|
| Linux AppImage, `.deb`, Flatpak | **No** | Built on hosted `ubuntu-24.04` with `live-first` apt packages (so WebKitGTK security updates reach the AppImage), runner timestamps, and non-hermetic `linuxdeploy` / `dpkg-deb` / `flatpak-builder` metadata. |
| macOS universal `.dmg` / `.app` | **No** | Built on hosted `macos-latest` with Xcode/SDK updates, Vite/Rust timestamps, and non-deterministic `hdiutil` HFS+/APFS volume UUIDs and timestamps. |
| Windows NSIS `.exe` | **No** | Built on hosted `windows-latest` with MSVC/Windows SDK/NSIS timestamps. |

**What IS deterministic and cryptographically bound:**

- CI attests every unsigned build file and `unsigned.SHA256SUMS` with GitHub Sigstore build provenance
  (`actions/attest-build-provenance`) before the signer downloads them.
- `ci/github/release-attach.sh` verifies that the unsigned CI inputs match `unsigned.SHA256SUMS` before
  attaching any signature.
- Re-attaching `win/dash-wallet-<ver>-windows-x64-setup.exe.pem` via `osslsigncode attach-signature`
  reproduces the **exact signed Windows `.exe`** byte-for-byte on both macOS and Linux.
- Re-attaching `osx/Dash Wallet.app/` via `ci/github/macos-detached-sig.py apply` verifies every non-signature
  file and every non-signature Mach-O byte is unchanged, reproduces the **exact signed, notarized, stapled
  `Dash Wallet.app`** byte-for-byte, and `pack-app` packs a **byte-identical**
  `dash-wallet-<ver>-macos-universal.app.tar.gz` on both macOS and Linux.
- Because `hdiutil` is non-deterministic, `sign-macos.sh` builds, signs, notarizes, and staples
  `dash-wallet-<ver>-macos-universal.dmg` once on macOS around that exact signed `.app`; during attach,
  `release-attach.sh` mounts the `.dmg` on macOS and proves via `diff -qr` that its interior `Dash Wallet.app`
  is byte-for-byte identical to the freshly re-attached `Dash Wallet.app` from CI's unsigned build + `osx/`.

---

## Windows Authenticode from a YubiKey on macOS

**Yes — Authenticode signing works directly on macOS from a YubiKey PIV slot** using `osslsigncode` with the
OpenSSL PKCS#11 provider (`libp11`) and Yubico's PIV PKCS#11 library (`libykcs11.dylib`, or OpenSC's
`opensc-pkcs11.so`).

### What to install on macOS

```sh
brew install osslsigncode libp11 yubico-piv-tool opensc gnupg gh jq
```

Why each package is needed:
- `osslsigncode` (Homebrew `2.16`, linked against `openssl@4`): signs PE/NSIS installers, extracts detached
  PKCS#7 PEM signatures (`extract-signature -pem`), re-attaches them (`attach-signature`), and verifies them.
- `libp11` (Homebrew `0.4.21`): installs the OpenSSL 4 PKCS#11 **provider** at
  `$(brew --prefix libp11)/lib/ossl-modules/pkcs11prov.dylib` (OpenSSL 4 removed the legacy `ENGINE` API, so
  `osslsigncode` loads `pkcs11prov.dylib` via `-provider`).
- `yubico-piv-tool` (Homebrew `2.7.3`): provides `$(brew --prefix yubico-piv-tool)/lib/libykcs11.dylib`
  (Yubico's PIV PKCS#11 module) and `yubico-piv-tool -a status` to inspect PIV slots.
- `opensc` (Homebrew `0.27.1`): provides `pkcs11-tool` (to inspect exact PKCS#11 object IDs/URIs) and
  `$(brew --prefix opensc)/lib/opensc-pkcs11.so` as an alternative PKCS#11 module if `libykcs11.dylib` has
  any slot quirk.

### Finding your YubiKey PIV slot, URI, and certificate chain

With the YubiKey plugged in:

```sh
# 1. Confirm which PIV slot holds Dash Core's code-signing certificate (typically 9c or 9a):
yubico-piv-tool -a status

# 2. List PKCS#11 objects to get the exact CKA_ID / label for -pkcs11cert and -key:
YKCS11="$(brew --prefix yubico-piv-tool)/lib/libykcs11.dylib"
pkcs11-tool --module "$YKCS11" --list-objects
```

In `libykcs11`, PIV slot `9a` maps to `id=%01`, slot `9c` (Digital Signature) maps to `id=%02`, and slot `9d`
maps to `id=%03`. For example, if the code-signing cert is in slot `9c`:

```sh
export WIN_PKCS11_MODULE="$(brew --prefix yubico-piv-tool)/lib/libykcs11.dylib"
export WIN_PKCS11_CERT="pkcs11:id=%02;type=cert"
export WIN_PKCS11_KEY="pkcs11:id=%02;type=private"
```

- **Certificate chain (`--chain` / `-ac`):** the YubiKey PIV slot holds only the leaf code-signing certificate.
  Save your CA's intermediate certificate chain (PEM) to e.g. `~/certs/dashcore-win-chain.pem` and pass
  `--chain ~/certs/dashcore-win-chain.pem` so Windows verifies the full chain without needing online AIA
  fetches. You can check the issuer of the cert on the YubiKey with:
  ```sh
  yubico-piv-tool -s 9c -a read-certificate | openssl x509 -noout -subject -issuer -fingerprint -sha256
  ```
- **Timestamp server (`--tsa` / `-ts`):** use an RFC 3161 SHA-256 timestamp server matching your CA:
  - DigiCert: `http://timestamp.digicert.com` (default in `ci/release/sign-windows.sh`)
  - Sectigo / Comodo: `http://timestamp.sectigo.com`
  - GlobalSign: `http://timestamp.globalsign.com/tsa/r6advanced1`
- **Installer `.exe` vs inner `dash-wallet.exe`:** mirroring Dash Core's `dash-detached-sigs`
  (`win/dashcore-24.0.3-win64-setup-unsigned.exe.pem`), single-pass `osslsigncode` signs the NSIS setup
  installer (`dash-wallet-<version>-windows-x64-setup.exe`), which satisfies Windows SmartScreen when
  downloaded from a browser. The inner `dash-wallet.exe` extracted by the installer remains unsigned unless
  repackaged in a 2-pass Windows NSIS build (which would only be needed if Windows 11 Smart App Control
  enforcement is required later).

---

## Items only the release signer (Pasta) can decide or provide

Before the first signed 1.0 release, Pasta provides or confirms:

1. **Detached-sigs repository:** create (or name) `PastaPastaPasta/dashwallet-detached-sigs` (public, mirroring
   `dashpay/dash-detached-sigs`).
2. **Pinned signer identities in the main repo (1.0 BLOCKER):** commit the real identities in a reviewed PR
   before tagging 1.0 (they are read strictly from the main repo at the tag, never from `dashwallet-detached-sigs`
   and never from mutable repository variables — placeholders fail closed):
   - `docs/release/signing-identities.json`:
     - `pgp_fingerprint`: 40-hex primary PGP key fingerprint;
     - `apple_team_id`: 10-character Apple Team ID (from `security find-identity -v -p codesigning`);
     - `windows_leaf_sha256`: `sha256:<64-hex>` of Dash Core's Windows code-signing leaf cert (from
       `yubico-piv-tool -s 9c -a read-certificate | openssl x509 -noout -fingerprint -sha256`).
   - `docs/release/release-key.asc`: armored PGP public key exported with
     `gpg --armor --export <PRIMARY_PGP_FINGERPRINT> > docs/release/release-key.asc`.
   - `docs/release/windows-ca.pem` (recommended): root + intermediate PEM certificate bundle for Dash Core's
     Windows code-signing CA (`osslsigncode verify -CAfile docs/release/windows-ca.pem`). Homebrew's default CA
     bundle on the `macos-15` runner has not been exercised live against Dash Core's code-signing root/CRL
     endpoints; committing the CA bundle here ensures `release-publish-signed.yml` passes `--win-ca-file
     docs/release/windows-ca.pem` cleanly.
3. **macOS Developer ID Application identity & Apple notarization credentials on his Mac:** exact string from
   `security find-identity -v -p codesigning` (e.g. `"Developer ID Application: Dash Core Group, Inc. (XXXXXXXXXX)"`)
   and either a `notarytool` Keychain profile (`xcrun notarytool store-credentials dash-notary ...`) or a local
   App Store Connect `.p8` key + Key ID + Issuer ID.
4. **Windows YubiKey details:** PIV slot (`9c` or `9a`), exact PKCS#11 URI, CA intermediate chain PEM
   (`--chain`), and RFC 3161 TSA URL matching the CA. Also decide whether single-pass NSIS installer signing
   (matching Dash Core) is sufficient for 1.0 or if a 2-pass Windows NSIS repack for inner `dash-wallet.exe`
   (Smart App Control) is needed later.
5. **GitHub `release` environment (zero secrets, required reviewers):** keep the `release` environment restricted
   to `v*` deployment tags and add Pasta as a required reviewer (for `.github/workflows/release-publish-signed.yml`).
   The environment must hold **zero secrets**.
6. **Stable updater key rotation & GitHub secret cleanup (1.0 BLOCKER):** the earlier updater key
   (`ED322B963B527B21`) was placed in the GitHub `release` environment during PK-03 setup. **`ED322B963B527B21`
   must never sign a stable feed.** Because no workflow or agent can remove GitHub environment secrets, Pasta
   must manually delete `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, and any other #769
   signing secrets (`APPLE_*`, `AZURE_*`, `RELEASE_PGP_*`) from the `release` environment in GitHub repository
   settings, generate a fresh local-only stable updater key pair
   (`pnpm --dir apps/desktop tauri signer generate -w ~/.config/dashwallet-release/stable-updater.key`), and
   commit its public key into `rust/crates/dw-app/tauri.conf.json` (`plugins.updater.pubkey`) (and update
   `RELEASE_KEY_ID` in `rust/crates/dw-app/src/updates_tests.rs`) before tagging 1.0.
7. **Required pre-publish rehearsal (`dry_run=true`):** before **every** stable release, once `vX.Y.Z` is
   signed locally and staged on `staging-dmg-vX.Y.Z`, run `release-publish-signed.yml` first with
   `-f dry_run=true`. That executes the **entire** `macos-15` verification and attachment pipeline —
   including confirming that the repacked `.app.tar.gz` deflate/zlib stream on `macos-15` is byte-identical
   to the `.app.tar.gz` packed and signed on the release signer's Mac (catching any macOS/Python zlib
   implementation drift before publication), live Windows CA chain/CRL verification, and Apple Developer ID
   + notarization + `.dmg` layout checks — and stops cleanly **before** final attestation, public GitHub
   Release creation, and staging-draft cleanup.
8. **Fail-closed default:** final stable tags (`vX.Y.Z`) fail closed automatically and wait for detached
   signatures — no repo variable is required to enforce signing. (`DW_ALLOW_UNSIGNED_STABLE=1` is an explicit
   escape hatch only if an unsigned pre-1.0 stable tag is ever needed; when set, CI also omits the updater
   endpoint from all three platform builds so an unsigned release never trusts a retired updater key.)

### Accepted risk (CI verification tooling)

- **Homebrew formula versions (`L5`):** `.github/workflows/release-publish-signed.yml` pins the runner image to
  `macos-15` and logs exact tool versions (`osslsigncode --version`, `gpg --version`), but runs unpinned
  `brew install osslsigncode gnupg` because Homebrew core does not retain older formula bottles without a custom
  tap. Risk is accepted and bounded by pinning all cryptographic trust anchors in the main repository at the tag
  (`windows_leaf_sha256`, `apple_team_id` + Developer ID OIDs, `plugins.updater.pubkey`, `pgp_fingerprint` +
  `release-key.asc`), by the zero-secret `release` environment with required reviewers, and by `gh attestation verify`
  on every unsigned input.

---

## Step-by-step release signing runbook

### Step 0 — One-time Mac & repository setup

```sh
# Install signing tools
brew install osslsigncode libp11 yubico-piv-tool opensc gnupg gh jq

# Verify macOS Developer ID identity in Keychain
security find-identity -v -p codesigning
# Expected output includes:
#   1) <SHA1> "Developer ID Application: <Dash Core Team Name> (<TEAM_ID>)"
#      1 valid identities found

# Store Apple notarization credentials once in the macOS login Keychain
# (omit --password so notarytool prompts securely without saving it in shell history or ps args)
xcrun notarytool store-credentials "dash-notary" \
  --apple-id "<apple-id@example.com>" \
  --team-id "<TEAM_ID>"
# Expected output:
#   App-specific password for <apple-id@example.com>: ********
#   Validating your credentials...
#   Success. Credentials saved to Keychain.
# (Alternatively, pass --api-key-path / --api-key-id / --api-issuer to sign-macos.sh.)

# Generate the local-only stable updater key (never uploaded to CI) and record its .pub in tauri.conf.json
mkdir -p -m 700 ~/.config/dashwallet-release
pnpm --dir apps/desktop tauri signer generate -w ~/.config/dashwallet-release/stable-updater.key
chmod 600 ~/.config/dashwallet-release/stable-updater.key
# Put the contents of stable-updater.key.pub into rust/crates/dw-app/tauri.conf.json -> plugins.updater.pubkey
# and update RELEASE_KEY_ID in rust/crates/dw-app/src/updates_tests.rs before tagging 1.0.

# Populate pinned signer identities in the main repo (via a reviewed PR before tagging 1.0):
gpg --armor --export "<40_HEX_PRIMARY_FPR>" > docs/release/release-key.asc
# Edit docs/release/signing-identities.json with:
#   pgp_fingerprint     = "<40_HEX_PRIMARY_FPR>"
#   apple_team_id       = "<10_CHAR_TEAM_ID>"
#   windows_leaf_sha256 = "sha256:<64_HEX_FROM_YUBIKEY_CERT>"

# In GitHub Settings -> Environments -> release:
#   1. Delete ALL secrets (TAURI_SIGNING_*, APPLE_*, AZURE_*, RELEASE_PGP_*).
#   2. Keep deployment branch/tag rule limited to v* tags.
#   3. Add yourself as Required Reviewer for release-publish-signed.yml.
```

---

### Step 1 — Tag and push the release commit

**Credential / device:** Pasta's real PGP key (YubiKey OpenPGP or local GPG key).

```sh
export VERSION=1.0.0
export TAG="v${VERSION}"
export REPO=PastaPastaPasta/dashwallet-desktop
export PGP_FPR="<40_HEX_PRIMARY_FPR>"

git fetch origin
git tag -s -u "$PGP_FPR" "$TAG" origin/main -m "Dash Wallet Desktop ${VERSION}"
git tag -v "$TAG"
# Expected output:
#   object <full-commit-sha>
#   type commit
#   tag v1.0.0
#   gpg: Good signature from "..."
#   Primary key fingerprint: XXXX XXXX ...

# Record the exact commit SHA from the locally verified signed tag (trust anchor for Step 2):
export TAG_SHA=$(git rev-parse "${TAG}^{commit}")
echo "TAG_SHA=$TAG_SHA"

git push origin "$TAG"
```

Wait for the `Release` workflow run on `$TAG` to finish green. Because final stable tags fail closed by default:
- CI builds all 5 unsigned bundles, 2 SBOMs, `SHA256SUMS`, and `dash-wallet-1.0.0-macos-unsigned.tar.gz`.
- The `publish` job attests the unsigned build inputs and holds back publishing (no unsigned GitHub release is
  created). Record the Actions run ID:

```sh
export RUN_ID=$(gh run list --repo "$REPO" --workflow release.yml --branch "$TAG" --limit 1 --json databaseId --jq '.[0].databaseId')
echo "RUN_ID=$RUN_ID"
```

---

### Step 2 — Download and verify the unsigned CI build

**Credential / device:** `gh` GitHub login (read-only).

Use the fail-closed helper `ci/release/fetch-unsigned.sh` with the locally recorded `$TAG_SHA` from Step 1
(it refuses if the remote tag moved, if the Actions run built a different commit, or if any single file fails
`gh attestation verify` or `unsigned.SHA256SUMS`, exposing files to `$UNSIGNED` only after every check passes):

```sh
export WORK="$HOME/dashwallet-release-work/${TAG}"
export UNSIGNED="$WORK/unsigned"
export SIGS="$WORK/sigs"
export OUT="$WORK/out"
rm -rf "$WORK"
mkdir -p "$OUT"

ci/release/fetch-unsigned.sh \
  --tag "$TAG" \
  --tag-sha "$TAG_SHA" \
  --run-id "$RUN_ID" \
  --repo "$REPO" \
  --unsigned-dir "$UNSIGNED" \
  --sigs-dir "$SIGS"
# Expected output:
#   ✓ Verification succeeded! (for every file)
#   every file prints ": OK"
#   fetch-unsigned: verified ... attested unsigned files for v1.0.0 (<TAG_SHA>)
```

---

### Step 3 — Sign, notarize, staple, and detach-extract macOS

**Credential / device:** Dash Core `"Developer ID Application: ..."` identity in macOS Keychain + Apple
notarization Keychain profile `"dash-notary"` (or `--api-key-path` / `--api-key-id` / `--api-issuer`).

```sh
export APPLE_IDENTITY="Developer ID Application: <Dash Core Team Name> (<TEAM_ID>)"

ci/release/sign-macos.sh \
  --version "$VERSION" \
  --unsigned-dir "$UNSIGNED" \
  --sigs-dir "$SIGS" \
  --out-dir "$OUT" \
  --identity "$APPLE_IDENTITY" \
  --notary-profile "dash-notary"
```

Expected output:
- `codesign --verify --deep --strict`: `valid on disk`, `satisfies its Designated Requirement`
- `notarytool submit --wait --timeout 60m`: `"status": "Accepted"` (on failure, prints `notarytool log <id>`)
- `stapler validate`: `The validate action worked!`
- `spctl --assess --type execute -vv`: `accepted`, `source=Notarized Developer ID`
- `macos-detached-sig`: extracts into `$SIGS/osx/Dash Wallet.app`, round-trip applies to a fresh copy of the
  unsigned `.app`, confirms `diff -qr` is empty, packs deterministic `$OUT/feed/dash-wallet-${VERSION}-macos-universal.app.tar.gz`,
  and writes signed/notarized/stapled `$OUT/dist/dash-wallet-${VERSION}-macos-universal.dmg`.

---

### Step 4 — Sign Windows NSIS installer with YubiKey (PKCS#11)

**Credential / device:** YubiKey holding Dash Core's Windows code-signing certificate (enter PIV PIN when
prompted; touch YubiKey if touch policy is enabled).

```sh
ci/release/sign-windows.sh \
  --version "$VERSION" \
  --unsigned-dir "$UNSIGNED" \
  --sigs-dir "$SIGS" \
  --out-dir "$OUT" \
  --pkcs11-module "$(brew --prefix yubico-piv-tool)/lib/libykcs11.dylib" \
  --pkcs11-cert "pkcs11:id=%02;type=cert" \
  --pkcs11-key "pkcs11:id=%02;type=private" \
  --chain "$HOME/certs/dashcore-win-chain.pem" \
  --tsa "http://timestamp.digicert.com"
```

Expected output:
- `osslsigncode sign`: `Succeeded`
- `osslsigncode extract-signature -pem`: writes `$SIGS/win/dash-wallet-${VERSION}-windows-x64-setup.exe.pem`
- `osslsigncode attach-signature` + `cmp`: byte-identical round-trip
- `osslsigncode verify`: `Signature verification: ok` and `Succeeded`

---

### Step 5 — Copy unchanged Linux/SBOM files and sign the stable updater feed

**Credential / device:** Local stable updater private key (`~/.config/dashwallet-release/stable-updater.key`,
never in CI).

```sh
for suffix in linux-x86_64.AppImage linux-amd64.deb linux-x86_64.flatpak sbom-rust.cdx.json sbom-npm.cdx.json; do
  cp "$UNSIGNED/dash-wallet-${VERSION}-${suffix}" "$OUT/dist/"
done

ci/release/sign-updater.sh \
  --version "$VERSION" \
  --dist-dir "$OUT/dist" \
  --feed-dir "$OUT/feed" \
  --sigs-dir "$SIGS" \
  --key-file "$HOME/.config/dashwallet-release/stable-updater.key" \
  --repo "$REPO"
```

Expected output:
- `verify-minisign: verified all 4 platform signatures in .../latest.json for version 1.0.0 (key ID ...)`

---

### Step 6 — Write final `SHA256SUMS` and PGP-sign it

**Credential / device:** Pasta's real PGP key (`$PGP_FPR`).

```sh
ci/release/sign-checksums.sh \
  --version "$VERSION" \
  --dist-dir "$OUT/dist" \
  --feed-dir "$OUT/feed" \
  --sigs-dir "$SIGS" \
  --gpg-key "$PGP_FPR"
```

Expected output:
- `gpg: Good signature from "..."`
- `Primary key fingerprint: ...` matching `$PGP_FPR`
- Prints all 9 lines of `SHA256SUMS` (5 bundles + 2 SBOMs + `latest.json` + `.app.tar.gz`).

---

### Step 7 — Store detached signatures in `dashwallet-detached-sigs`

**Credential / device:** Git push access to `PastaPastaPasta/dashwallet-detached-sigs` (mirroring
`dashpay/dash-detached-sigs`).

The `$SIGS` directory (~40 KB total; zero binaries) contains:

```text
unsigned.SHA256SUMS
osx/Dash Wallet.app/manifest.json
osx/Dash Wallet.app/Contents/MacOS/dash-wallet.arm64.cs
osx/Dash Wallet.app/Contents/MacOS/dash-wallet.x86_64.cs
osx/Dash Wallet.app/Contents/_CodeSignature/CodeResources
osx/Dash Wallet.app/Contents/CodeResources
win/dash-wallet-1.0.0-windows-x64-setup.exe.pem
updater/latest.json
SHA256SUMS
SHA256SUMS.asc
```

Commit and tag them in the detached-sigs repository:

```sh
export SIGS_REPO_DIR="$HOME/src/dashwallet-detached-sigs"
git -C "$SIGS_REPO_DIR" fetch origin
git -C "$SIGS_REPO_DIR" checkout -B "$TAG" origin/main
rm -rf "${SIGS_REPO_DIR:?}/${TAG}"
mkdir -p "$SIGS_REPO_DIR/$TAG"
cp -R "$SIGS/"* "$SIGS_REPO_DIR/$TAG/"
git -C "$SIGS_REPO_DIR" add "$TAG"
git -C "$SIGS_REPO_DIR" commit -m "Add detached signatures for ${TAG}"
git -C "$SIGS_REPO_DIR" push origin "$TAG"
export SIGS_COMMIT=$(git -C "$SIGS_REPO_DIR" rev-parse HEAD)
echo "SIGS_COMMIT=$SIGS_COMMIT"
```

---

### Step 8 — Attach, verify, attest, and publish

#### Option A (recommended): Publish via `.github/workflows/release-publish-signed.yml` (mints Sigstore attestations on every final signed asset)

1. Stage the signed, notarized `.dmg` in a temporary draft release on `staging-dmg-${TAG}` (`release-publish-signed.yml`
   refuses `dmg_draft_tag == tag` so the draft is deleted only **after** the final `$TAG` release publishes
   successfully, preserving the staged `.dmg` if publish fails):

   ```sh
   gh release create "staging-dmg-${TAG}" "$OUT/dist/dash-wallet-${VERSION}-macos-universal.dmg" \
     --repo "$REPO" --draft --title "Staging signed dmg for ${TAG}"
   ```

2. **(Required before every stable release)** Dispatch `release-publish-signed.yml` with `-f dry_run=true`
   first and wait for it to pass green. This exercises the entire `macos-15` attach/verify pipeline — in
   particular verifying that `macos-15`'s repacked `.app.tar.gz` (zlib/deflate stream) is byte-identical to
   the locally packed and minisign-signed `.app.tar.gz`, alongside live Windows CA chain + CRL verification
   and Apple Developer ID + notarization checks — without minting final attestations, publishing, or deleting
   `staging-dmg-${TAG}`:

   ```sh
   gh workflow run release-publish-signed.yml \
     --repo "$REPO" \
     --ref "$TAG" \
     -f tag="$TAG" \
     -f release_run_id="$RUN_ID" \
     -f sigs_repo="PastaPastaPasta/dashwallet-detached-sigs" \
     -f sigs_ref="$SIGS_COMMIT" \
     -f dry_run=true
   ```

3. Dispatch `release-publish-signed.yml` on `--ref "$TAG"` to attest and publish (zero secrets in CI; approve
   the `release` environment review prompt in GitHub Actions if required reviewers are enabled):

   ```sh
   gh workflow run release-publish-signed.yml \
     --repo "$REPO" \
     --ref "$TAG" \
     -f tag="$TAG" \
     -f release_run_id="$RUN_ID" \
     -f sigs_repo="PastaPastaPasta/dashwallet-detached-sigs" \
     -f sigs_ref="$SIGS_COMMIT"
   ```

   The workflow runs on `macos-15` in the secret-less `release` environment, enforces `github.ref == refs/tags/$TAG`,
   checks out `github.sha` and asserts `refs/tags/$TAG^{commit} == HEAD`, verifies every downloaded unsigned
   artifact from `$RUN_ID` with `gh attestation verify` against `release.yml`, fetches `dashwallet-detached-sigs`
   at `$SIGS_COMMIT`, runs `ci/github/release-attach.sh` using the pinned identities from the main repo at `$TAG`
   (`docs/release/signing-identities.json`, `docs/release/release-key.asc`, and `docs/release/windows-ca.pem` when
   populated — comparing `unsigned.SHA256SUMS` byte-for-byte with CI's copy, attaching Windows `.pem` +
   `osslsigncode verify` with pinned leaf hash, attaching macOS `osx/` + Developer ID OID/Team ID `codesign -R` /
   `spctl` / `stapler`, verifying the staged `.dmg` Developer ID cert type (`codesign -R` + `spctl --assess --type open --context context:primary-signature`),
   staple, top-level `{Dash Wallet.app, Applications -> /Applications}` layout, and interior `.app` modes/bytes,
   verifying `latest.json` exact URLs + minisign signatures, verifying all 9 hashes in `SHA256SUMS`, and verifying
   `SHA256SUMS.asc` with `GOODSIG` + pinned primary fingerprint while rejecting revoked/expired keys), attests all
   11 final assets with `actions/attest-build-provenance`, publishes the exact attested copies via
   `ci/github/release-publish.sh`, and then deletes the `staging-dmg-${TAG}` draft.

#### Option B: Local attach, verify, and publish from macOS

**Must be run on macOS:** `ci/github/release-attach.sh` fails closed on non-Darwin hosts so Apple `codesign`,
`spctl`, `stapler`, and `.dmg` layout checks cannot be silently skipped (`--allow-non-darwin` is strictly for
Linux test suites, never for publishing).

If you need to attach and publish directly from your Mac (without GitHub Actions attesting the final signed
assets — the unsigned CI build inputs remain attested from Step 1):

```sh
ci/github/release-attach.sh \
  --version "$VERSION" \
  --unsigned-dir "$UNSIGNED" \
  --sigs-dir "$SIGS" \
  --out-dir "$WORK/attached" \
  --signed-dmg "$OUT/dist/dash-wallet-${VERSION}-macos-universal.dmg" \
  --repo "$REPO" \
  --identities-file docs/release/signing-identities.json \
  --pgp-pubkey docs/release/release-key.asc
# Add `--win-ca-file docs/release/windows-ca.pem` if populated.

TAG_SHA=$(git rev-parse "${TAG}^{commit}")
GITHUB_EVENT_NAME=push GITHUB_REF="refs/tags/${TAG}" GITHUB_SHA="$TAG_SHA" \
GITHUB_REPOSITORY="$REPO" VERSION="$VERSION" \
DIST="$WORK/attached/dist" FEED="$WORK/attached/feed" \
SIGNED_MANIFEST="$WORK/attached/signed-manifest.json" \
DW_SIGNING_REQUIRED=1 GH_TOKEN="$(gh auth token)" \
  bash -c 'ci/github/release-publish.sh prepare && ci/github/release-publish.sh publish'
```

Then run the verification checklist in [`CHECKLIST.md`](CHECKLIST.md) §7 and [`verifying.md`](verifying.md).
