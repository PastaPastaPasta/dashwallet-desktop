# Release signing runbook (stable 1.0+ — Dash Core detached-sigs model)

Stable releases of Dash Wallet Desktop follow the Dash Core signing model (`dashpay/dash` `contrib/` and
`dashpay/dash-detached-sigs`): **no Apple, Windows, PGP, or stable updater private keys ever enter GitHub
Actions.** CI builds unsigned artifacts and attests them; the release signer signs locally on macOS using
Dash Core's existing signing identities (macOS Developer ID keychain identity, YubiKey PIV code-signing
certificate, real PGP key, and local stable updater key), stores the small detached signatures in a
detached-signatures repository, and a zero-secret verification/attach step (`ci/github/release-attach.sh` /
`.github/workflows/release-publish-signed.yml`) verifies, re-attaches, attests, and publishes the final
release.

Keyless release candidates (`vX.Y.Z-rc.N`) and `main` / `continuous` builds remain untouched: CI builds and
publishes the 8 unsigned assets with GitHub Sigstore build provenance attestations only (no `.asc`, no
`latest.json`, nothing required even when `DW_SIGNING_REQUIRED=1`).

> **Nightly track:** automated nightly signing (separate macOS Developer ID cert + revocable API key in CI,
> unsigned Windows, separate CI updater key, separate bundle ID and app name `"Dash Wallet Nightly"` for
> side-by-side install, plus cross-channel key-rejection tests) is split into a follow-up PR after the RC so
> this PR stays focused on the stable release path.

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
   `dashpay/dash-detached-sigs`) and commit the public PGP release key as `release-key.asc` at its root.
2. **macOS Developer ID Application identity & Apple Team ID:** exact string from
   `security find-identity -v -p codesigning` (e.g. `"Developer ID Application: Dash Core Group, Inc. (XXXXXXXXXX)"`).
3. **Apple notarization credentials on his Mac:** either a `notarytool` Keychain profile
   (`xcrun notarytool store-credentials dash-notary ...`) or a local App Store Connect `.p8` key + Key ID +
   Issuer ID.
4. **Windows YubiKey details:** PIV slot (`9c` or `9a`), exact PKCS#11 URI, CA intermediate chain PEM
   (`--chain`), RFC 3161 TSA URL matching the CA, and optionally the leaf cert SHA-256 for repository variable
   `RELEASE_WIN_LEAF_HASH` (`sha256:<HEX>`). Also decide whether single-pass NSIS installer signing (matching
   Dash Core) is sufficient for 1.0 or if a 2-pass Windows NSIS repack for inner `dash-wallet.exe` (Smart App
   Control) is needed later.
5. **Real PGP release key:** primary key fingerprint set as repository variable `RELEASE_PGP_FINGERPRINT`
   (40 hex characters) and matching `release-key.asc` published in the detached-sigs repo and release notes.
6. **Stable updater key rotation:** the earlier updater key (`ED322B963B527B21`) was placed in the `release`
   environment during PK-03 setup. Because stable updater signing now lives **only** on Pasta's Mac and never
   in CI, remove `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` from the `release`
   environment, generate a fresh local-only stable updater key pair
   (`pnpm --dir apps/desktop tauri signer generate -w ~/.config/dashwallet-release/stable-updater.key`),
   and commit its public key into `rust/crates/dw-app/tauri.conf.json` (`plugins.updater.pubkey`) before 1.0.
7. **Enforcement switch:** set repository variable `DW_SIGNING_REQUIRED=1` once ready so a final `vX.Y.Z` tag
   cannot publish unsigned.

---

## Step-by-step release signing runbook

### Step 0 — One-time Mac setup

```sh
# Install signing tools
brew install osslsigncode libp11 yubico-piv-tool opensc gnupg gh jq

# Verify macOS Developer ID identity in Keychain
security find-identity -v -p codesigning
# Expected output includes:
#   1) <SHA1> "Developer ID Application: <Dash Core Team Name> (<TEAM_ID>)"
#      1 valid identities found

# Store Apple notarization credentials once in the macOS login Keychain
xcrun notarytool store-credentials "dash-notary" \
  --apple-id "<apple-id@example.com>" \
  --team-id "<TEAM_ID>" \
  --password "<app-specific-password>"
# Expected output:
#   Validating your credentials...
#   Success. Credentials saved to Keychain.
# (Alternatively, pass --api-key-path / --api-key-id / --api-issuer to sign-macos.sh.)

# Generate the local-only stable updater key (never uploaded to CI) and record its .pub in tauri.conf.json
mkdir -p -m 700 ~/.config/dashwallet-release
pnpm --dir apps/desktop tauri signer generate -w ~/.config/dashwallet-release/stable-updater.key
chmod 600 ~/.config/dashwallet-release/stable-updater.key
# Put the contents of stable-updater.key.pub into rust/crates/dw-app/tauri.conf.json -> plugins.updater.pubkey
# and update RELEASE_KEY_ID in rust/crates/dw-app/src/updates_tests.rs before tagging 1.0.

# Enable enforcement (checks for attached signatures, never CI secrets) and pin PGP / Windows certs
gh variable set DW_SIGNING_REQUIRED --body 1 --repo PastaPastaPasta/dashwallet-desktop
gh variable set RELEASE_PGP_FINGERPRINT --body "<40_HEX_PRIMARY_FPR>" --repo PastaPastaPasta/dashwallet-desktop
# Optional: pin Windows leaf cert SHA-256 (from `yubico-piv-tool -s 9c -a read-certificate | openssl x509 -noout -fingerprint -sha256`)
gh variable set RELEASE_WIN_LEAF_HASH --body "sha256:<64_HEX>" --repo PastaPastaPasta/dashwallet-desktop
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

git push origin "$TAG"
```

Wait for the `Release` workflow run on `$TAG` to finish green. Because `DW_SIGNING_REQUIRED=1` is set:
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

```sh
export WORK="$HOME/dashwallet-release-work/${TAG}"
export UNSIGNED="$WORK/unsigned"
export SIGS="$WORK/sigs"
export OUT="$WORK/out"
rm -rf "$WORK"
mkdir -p "$UNSIGNED" "$SIGS" "$OUT"

gh run download "$RUN_ID" --repo "$REPO" -n "release-${TAG}" -D "$UNSIGNED"
gh run download "$RUN_ID" --repo "$REPO" -n "unsigned-signing-inputs-${TAG}" -D "$UNSIGNED"

TAG_SHA=$(gh api "repos/${REPO}/commits/${TAG}" --jq .sha)
for f in "$UNSIGNED"/*; do
  gh attestation verify "$f" --repo "$REPO" \
    --signer-workflow "${REPO}/.github/workflows/release.yml" \
    --source-ref "refs/tags/${TAG}" \
    --source-digest "$TAG_SHA"
done
# Expected output for every file:
#   ✓ Verification succeeded!

(cd "$UNSIGNED" && shasum -a 256 -c unsigned.SHA256SUMS)
# Expected output: every file prints ": OK"

cp "$UNSIGNED/unsigned.SHA256SUMS" "$SIGS/unsigned.SHA256SUMS"
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
# Ensure the public PGP key is present at repo root for zero-secret CI verification:
gpg --armor --export "$PGP_FPR" > "$SIGS_REPO_DIR/release-key.asc"
git -C "$SIGS_REPO_DIR" add "$TAG" release-key.asc
git -C "$SIGS_REPO_DIR" commit -m "Add detached signatures for ${TAG}"
git -C "$SIGS_REPO_DIR" push origin "$TAG"
```

---

### Step 8 — Attach, verify, attest, and publish

#### Option A (recommended): Publish via `.github/workflows/release-publish-signed.yml` (mints Sigstore attestations on every final signed asset)

1. Stage the signed, notarized `.dmg` in a temporary draft release on `$TAG` (GitHub Actions will verify its
   `codesign`, `stapler`, and interior `Dash Wallet.app` byte-identity against `apply(unsigned, osx/)`, then
   delete the staging draft before publishing the complete 11-asset release):

   ```sh
   gh release create "$TAG" "$OUT/dist/dash-wallet-${VERSION}-macos-universal.dmg" \
     --repo "$REPO" --verify-tag --draft --title "Staging signed dmg for ${TAG}"
   ```

2. Dispatch `release-publish-signed.yml` (zero secrets in CI):

   ```sh
   gh workflow run release-publish-signed.yml \
     --repo "$REPO" \
     --ref "$TAG" \
     -f tag="$TAG" \
     -f release_run_id="$RUN_ID" \
     -f sigs_repo="PastaPastaPasta/dashwallet-detached-sigs" \
     -f sigs_ref="$TAG"
   ```

   The workflow runs on `macos-latest`, downloads the exact unsigned artifacts from `$RUN_ID`, clones
   `dashwallet-detached-sigs` at `$TAG`, runs `ci/github/release-attach.sh` (verifying `unsigned.SHA256SUMS`,
   attaching Windows `.pem` + `osslsigncode verify`, attaching macOS `osx/` + `codesign`/`spctl`/`stapler`,
   verifying the staged `.dmg`'s interior `.app` matches byte-for-byte, verifying `latest.json` via
   `verify-minisign.py`, verifying all 9 hashes in `SHA256SUMS`, and verifying `SHA256SUMS.asc` against
   `RELEASE_PGP_FINGERPRINT`), attests all 11 final assets with `actions/attest-build-provenance`, removes the
   staging draft, and publishes the final release via `ci/github/release-publish.sh` with
   `DW_SIGNING_REQUIRED=1`.

#### Option B: Local attach, verify, and publish from macOS

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
  --pgp-fingerprint "$PGP_FPR"

TAG_SHA=$(git rev-parse "${TAG}^{commit}")
GITHUB_EVENT_NAME=push GITHUB_REF="refs/tags/${TAG}" GITHUB_SHA="$TAG_SHA" \
GITHUB_REPOSITORY="$REPO" VERSION="$VERSION" \
DIST="$WORK/attached/dist" FEED="$WORK/attached/feed" \
SIGNED_MANIFEST="$WORK/attached/signed-manifest.json" \
DW_SIGNING_REQUIRED=1 GH_TOKEN="$(gh auth token)" \
  bash -c 'ci/github/release-publish.sh prepare && ci/github/release-publish.sh publish'
```

Then run the verification checklist in [`CHECKLIST.md`](CHECKLIST.md) §7 and [`verifying.md`](verifying.md).
