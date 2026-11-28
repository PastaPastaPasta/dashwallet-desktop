# Signing a release candidate in CI

[`RC-SIGNING.md`](RC-SIGNING.md) has the release signer sign a candidate on their own Mac. This file is the other
way to do the same job: `.github/workflows/release-sign.yml` signs the macOS app and `SHA256SUMS` on a hosted
runner, so after the one-time setup below the signer's own part of a candidate is one command with the YubiKey.
The Windows installer is still signed on that YubiKey: its key cannot leave it. Both ways publish the same 9
assets through `release-publish-signed.yml`, with the same checks. A stable `vX.Y.Z` is still signed locally
([`SIGNING.md`](SIGNING.md)): its updater feed needs the stable updater key, and that key is not in CI.

The examples use `v1.0.0-rc.2`. Substitute the real tag everywhere.

## How it works

One dispatch, two jobs:

| Job | Runs | Sees secrets | Can write to the repository |
|---|---|---|---|
| `sign` | `fetch-unsigned.sh`, `windows-verify-pinned.sh`, `sign-macos.sh`, `sign-checksums.sh` on `macos-15` | yes, through the `release-signing` environment only | no (`contents: read`) |
| `publish` | `release-publish-signed.yml`: `release-attach.sh`, attestation, `release-publish.sh` | no (`release` environment, zero secrets) | yes |

- **Nothing is built where the keys are.** `release.yml` built and attested the unsigned files on the tag push. The
  `sign` job installs no project dependency and compiles nothing: it downloads those files, checks every
  attestation, and signs them.
- **The job that can publish trusts nothing from the job that signs.** `sign` hands over only the detached
  signatures and the signed `.dmg` (the run artifact `ci-signed-v<version>`). `publish` re-attaches them to CI's own
  unsigned bytes and verifies each one against the identities pinned in this repository at the tag, exactly as it
  does for signatures made on a Mac.
- **A person approves every use of the keys.** The `release-signing` environment has a required reviewer and
  accepts `v*` tags only, so a signing run waits for that approval, and no branch or pull request can reach the
  secrets.

| Signature | Key | Where the key lives |
|---|---|---|
| macOS `.app` and `.dmg`: Developer ID, hardened runtime, notarized, stapled | Developer ID Application certificate; App Store Connect API key for the notary service | secrets `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_API_KEY_P8` |
| Windows installer: Authenticode, SHA-256, RFC 3161 timestamp | the code-signing certificate's key | on the release signer's YubiKey, never in GitHub: the signer signs, CI attaches the signature and checks it against the pinned certificate |
| `SHA256SUMS.asc` | a PGP key of CI's own, accepted for candidates only | secrets `RELEASE_PGP_PRIVATE_KEY`, `RELEASE_PGP_PASSPHRASE` |

## One-time setup

Only the repository owner can do these. Every secret goes straight from the owner's machine into GitHub; none is
sent to anyone, and no agent handles one. `R` below is `PastaPastaPasta/dashwallet-desktop`.

**Run the blocks in bash.** On macOS type `bash` first. zsh, the default there, passes a `# note` after a command
to the command as arguments, and it cuts `$NAME:e…` short. The blocks are written for bash, the macOS one
included.

### 1. The `release-signing` environment (3 minutes)

```sh
R=PastaPastaPasta/dashwallet-desktop
gh api -X PUT "repos/$R/environments/release-signing" --input - <<JSON
{"reviewers": [{"type": "User", "id": $(gh api user --jq .id)}],
 "deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true}}
JSON
gh api -X POST "repos/$R/environments/release-signing/deployment-branch-policies" -f name='v*' -f type=tag
```

Or on github.com: **Settings ▸ Environments ▸ New environment**, name `release-signing`; tick **Required
reviewers** and add yourself; under **Deployment branches and tags** choose **Selected branches and tags** and add
the tag rule `v*`. The existing `release` environment stays as it is, with no secrets.

The checks inside `release-sign.yml` (a candidate tag, on `main`) are read from the tagged commit, so they bind
only a tag made from `main`. Whoever can push a `v*` tag can push one with another workflow that names this
environment, and the reviewer's approval is then the only gate. Two things close that:

- a tag ruleset that lets only you create, move or delete `v*` tags: **Settings ▸ Rules ▸ Rulesets ▸ New tag
  ruleset**, target `v*`, restrict creations, updates and deletions, bypass list: yourself;
- before each **Approve and deploy**, check that the run is `Sign release in CI` on the tag you pushed, at the
  commit you tagged.

### 2. Apple: the certificate and the notary key (15 minutes)

CI gets a **Developer ID Application** certificate of its own, of the team pinned as `apple_team_id`. Do not
export the identity that signs Dash Core and the stable releases: that key stays off hosted runners
([`SIGNING.md`](SIGNING.md)), and a certificate of CI's own can be revoked without touching anything signed with
the other. A team can hold five.

1. **Keychain Access ▸ Certificate Assistant ▸ Request a Certificate From a Certificate Authority**, common name
   `Dash Wallet Desktop CI`, **Saved to disk**. The private key is made in your Keychain and never leaves it.
2. The team's Account Holder (only that role can) uploads the request at
   <https://developer.apple.com/account/resources/certificates/add>, **Developer ID Application**, and downloads
   the `.cer`.
3. Open the `.cer` so it joins its key. In **Keychain Access ▸ My Certificates** there are now two
   `Developer ID Application: …` identities; the new one expires five years from today. Right-click it,
   **Export**, format `.p12`, with a new password.
4. Upload it, then delete the new identity (the certificate and its private key) from your Keychain, so signing
   by name on this Mac still finds exactly one:

```sh
base64 -i ~/Desktop/developer-id.p12 | gh secret set APPLE_CERTIFICATE --env release-signing --repo "$R"
gh secret set APPLE_CERTIFICATE_PASSWORD --env release-signing --repo "$R"     # prompts: paste the .p12 password
rm ~/Desktop/developer-id.p12
```

The notary key: <https://appstoreconnect.apple.com/access/integrations/api> ▸ **Team Keys** ▸ **+**, name
`dashwallet-desktop notarization`, access **Developer**. Download the `.p8` (it downloads once) and note the
**Key ID** and the **Issuer ID** shown on that page.

```sh
gh secret set APPLE_API_KEY_P8 --env release-signing --repo "$R" < ~/Downloads/AuthKey_<KEY_ID>.p8
gh variable set APPLE_API_KEY_ID --env release-signing --repo "$R" --body "<KEY_ID>"
gh variable set APPLE_API_ISSUER_ID --env release-signing --repo "$R" --body "<ISSUER_ID>"
rm ~/Downloads/AuthKey_<KEY_ID>.p8
```

### 3. PGP: a key of CI's own for a candidate's `SHA256SUMS.asc` (10 minutes)

CI signs a candidate's checksums with a key made for that and nothing else. It is not your release key, and not
a subkey of it: a compromised run can sign with whatever key CI holds, and a subkey of the release key verifies
as the release key wherever that key is trusted, a stable release of this app included. `release-attach.sh`
accepts CI's key for a release candidate signed in CI and refuses it for everything else. Your own key keeps
signing the tag ([`RC-SIGNING.md`](RC-SIGNING.md) section 3) and every stable release.

```sh
export GNUPGHOME=$(mktemp -d)
gpg --quick-generate-key "Dash Wallet Desktop candidate signing (CI) <you@example.org>" ed25519 sign 2y   # asks for a new passphrase
CI_FPR=$(gpg --list-keys --with-colons | awk -F: '$1 == "fpr" { print $10; exit }')
gpg --armor --export "$CI_FPR" > ci-release-key.asc && echo "$CI_FPR"          # both are public: step 5

# Before anything is uploaded or deleted: what lets you revoke or re-install this key without GitHub.
BACKUP=<a directory on the offline medium that holds your other signing material>
cp "$GNUPGHOME/openpgp-revocs.d/$CI_FPR.rev" "$BACKUP/dashwallet-ci-$CI_FPR.rev"
gpg --armor --export-secret-keys "$CI_FPR" > "$BACKUP/dashwallet-ci-$CI_FPR.secret.asc"    # still under its passphrase
gpg --show-keys "$BACKUP/dashwallet-ci-$CI_FPR.secret.asc" | grep -q "$CI_FPR" &&
  test -s "$BACKUP/dashwallet-ci-$CI_FPR.rev" && echo "backup ok"
```

Go on only after `backup ok`:

```sh
gpg --armor --export-secret-keys "$CI_FPR" | gh secret set RELEASE_PGP_PRIVATE_KEY --env release-signing --repo "$R"
gh secret set RELEASE_PGP_PASSPHRASE --env release-signing --repo "$R"         # prompts: paste that passphrase
gpgconf --kill all && rm -rf "$GNUPGHOME" && unset GNUPGHOME
```

Do not certify this key with your own. A certification would make gpg show CI's key as valid to everyone who
trusts yours, and only the fingerprint would then tell a candidate's signature from a release's.

### 4. Windows: the YubiKey stays where it is (10 minutes, once)

CI holds nothing for Windows. A code-signing key issued today must live in certified hardware, so the installer is
signed on the YubiKey as in [`RC-SIGNING.md`](RC-SIGNING.md), and CI takes that signature from the
detached-signatures repository. Three things from that file, each done once:

- the tools and the YubiKey's PKCS#11 details ([section 2](RC-SIGNING.md#2-one-time-setup-on-the-signers-mac),
  "Tools" and "YubiKey PIV");
- the public repository `PastaPastaPasta/dashwallet-detached-sigs`
  ([the signer's checklist](RC-SIGNING.md#the-signers-checklist), item b);
- the certificate's SHA-256 for step 5, the `WIN_LEAF` line of that checklist's item a.

### 5. Pin the public identities (one pull request to `main`, before the tag)

`release-publish-signed.yml` reads these from the tagged commit, and placeholders fail closed.
`docs/release/signing-identities.json`:

```json
{
  "schema": 1,
  "pgp_fingerprint": "<your release key's, as it is today: it signs the tag and stable releases>",
  "ci_pgp_fingerprint": "<40 hex, CI's key from step 3>",
  "apple_team_id": "<10 characters, in parentheses in the Developer ID identity's name>",
  "windows_leaf_sha256": "sha256:<64 lower-case hex, the YubiKey's certificate from step 4>"
}
```

- `docs/release/ci-release-key.asc`: the file from step 3. `pgp_fingerprint` and `release-key.asc` stay your own
  release key's and are not read for a candidate signed in CI; the two fingerprints must be two different keys,
  and `release-key.asc` must not carry a subkey that CI holds.
- `docs/release/windows-ca.pem` (optional): the roots of the Windows certificate's CA. While it holds no
  certificate, the checks use Homebrew's CA bundle.

## Each candidate

1. The version bump, the notes and the pins of step 5 are on `main`, and `DW_SIGN_RC` is `1`
   ([`RC-SIGNING.md`](RC-SIGNING.md) section 1). Push the tag and wait for its Release run
   ([`RC-SIGNING.md`](RC-SIGNING.md) section 3). It ends with the notice that the tag holds unsigned binaries.
2. Sign the Windows installer on the Mac with the YubiKey and push the signature. `$YKCS11` and the slot's ID are
   from [`RC-SIGNING.md`](RC-SIGNING.md) section 2; osslsigncode asks for the PIV PIN:

   ```sh
   export REPO=PastaPastaPasta/dashwallet-desktop TAG=v1.0.0-rc.2 VERSION=1.0.0-rc.2
   cd ~/src/dashwallet-desktop && git fetch origin --tags && git checkout --detach "$TAG"
   export TAG_SHA=$(git rev-parse "$TAG^{commit}")
   export RUN_ID=$(gh run list --repo "$REPO" --workflow release.yml --branch "$TAG" --event push --limit 1 --json databaseId --jq '.[0].databaseId')
   export WORK="$HOME/dashwallet-release-work/$TAG" && rm -rf "$WORK" && mkdir -p "$WORK"
   ci/release/fetch-unsigned.sh --tag "$TAG" --tag-sha "$TAG_SHA" --run-id "$RUN_ID" --repo "$REPO" \
     --unsigned-dir "$WORK/unsigned" --sigs-dir "$WORK/sigs"
   ci/release/sign-windows.sh --version "$VERSION" --unsigned-dir "$WORK/unsigned" --sigs-dir "$WORK/sigs" --out-dir "$WORK/out" \
     --pkcs11-module "$YKCS11" --pkcs11-cert "pkcs11:id=%02;type=cert" --pkcs11-key "pkcs11:id=%02;type=private" \
     --chain "$HOME/certs/dashcore-win-chain.pem" --tsa "http://timestamp.digicert.com"

   cd ~/src/dashwallet-detached-sigs && git fetch origin && git checkout -B "$TAG" origin/main
   rm -rf "./$TAG" && mkdir "$TAG" && cp -R "$WORK/sigs/." "$TAG/"
   git add "$TAG" && git commit -S -m "Add the Windows signature for $TAG" && git push origin "$TAG"
   export SIGS_COMMIT=$(git rev-parse HEAD) && echo "$SIGS_COMMIT"
   ```

   `sign-windows.sh` ends with `Signature verification: ok` and `Succeeded`. What is pushed is two small files,
   `unsigned.SHA256SUMS` and `win/dash-wallet-1.0.0-rc.2-windows-x64-setup.exe.pem`; no binary and no key.
3. Rehearse. This signs for real and verifies everything, then stops before attesting and publishing:

   ```sh
   gh workflow run release-sign.yml --repo "$REPO" --ref "$TAG" -f tag="$TAG" -f release_run_id="$RUN_ID" \
     -f windows_sig_ref="$SIGS_COMMIT" -f dry_run=true
   ```

   Open the run, check it is on your tag and commit, and choose **Review deployments ▸ release-signing ▸ Approve
   and deploy**. The job attaches the Windows signature and checks it against `windows_leaf_sha256` before it uses
   any key of its own. Most of its time is Apple's notary service, usually a few minutes for each of its two
   submissions. If the `release` environment has a required reviewer too, the `publish` job asks once more; give
   that approval within three days, while the signing job's artifact still exists. A green rehearsal ends with the
   notice `dry_run=true on v1.0.0-rc.2 …`.
4. Publish: the same command without `-f dry_run=true`, and the same approval.
5. Verify the published candidate as in [`RC-SIGNING.md`](RC-SIGNING.md) section 7, with one difference. The PGP
   key is CI's, `docs/release/ci-release-key.asc` at the tag, and the primary fingerprint gpg prints is
   `ci_pgp_fingerprint`:

   ```sh
   gpg --import docs/release/ci-release-key.asc && gpg --verify SHA256SUMS.asc SHA256SUMS
   ```

If a run fails, nothing is published. A wrong secret or variable is fixed in the environment and the same command
is run again. A Windows signature that fails its check is made again and pushed, and the run is dispatched with the
new commit. A fault in a script or in a pin needs a fix on `main` and the next candidate, because both are read
from the tagged commit.

The `sign` job installs `osslsigncode` and `gnupg` from Homebrew at their current versions, as the signer's Mac
does. Those two packages and what they depend on therefore run in a job that holds CI's PGP key and the Apple
certificate, and the approved tag does not fix their versions. That is the same trust the local runbook places in Homebrew, and it is not yet
removed here (issue #1156).

## Rotating and revoking

- **Apple:** revoke the API key in App Store Connect and upload a new one at any time. Revoke the certificate only
  if its key leaked: Gatekeeper then refuses every app signed with it.
- **Windows:** nothing of it is in CI. A new certificate on the YubiKey needs a new `windows_leaf_sha256` on
  `main` before the next tag.
- **PGP:** make a new key as in step 3, writing its public key somewhere else than the repository (`NEW_KEY`
  below), and pin its fingerprint; from then on the old key verifies nothing. `docs/release/ci-release-key.asc`
  on `main` then gains the new key and keeps every earlier one with its revocation, so people who kept an old
  key learn it was revoked. From a checkout of `main`, with `OLD_FPR` the outgoing key's fingerprint:

  ```sh
  ROT=$(mktemp -d) && chmod 700 "$ROT"
  gpg --homedir "$ROT" --import docs/release/ci-release-key.asc             # every CI key so far, revocations included
  sed 's/^:-----BEGIN/-----BEGIN/' "$BACKUP/dashwallet-ci-$OLD_FPR.rev" | gpg --homedir "$ROT" --import
  gpg --homedir "$ROT" --import "$NEW_KEY"
  gpg --homedir "$ROT" --armor --export > docs/release/ci-release-key.asc   # all of them, nothing dropped
  gpg --show-keys docs/release/ci-release-key.asc     # one key without "revoked": the new one; every earlier one revoked
  gpgconf --homedir "$ROT" --kill all && rm -rf "$ROT"
  ```

  The saved revocation certificate has a colon in front of its `-----BEGIN` line, which the `sed` removes. The
  publish job still accepts only the pinned key. The key expires two years after it was made; replace it the same
  way before then.
- **Shutting CI signing down:** delete the `release-signing` environment, which removes every secret at once, and
  revoke the Apple API key and CI's PGP key as above. A workflow that names a deleted environment creates it
  again without its rules, but with nothing in it.

## Appendix: a cloud-held Windows key (not set up)

Nothing above needs this, and `release-sign.yml` has no step for it. It is here because the scripts already carry
the parts, should the YubiKey ever stop being the place for the Windows key:

- `ci/release/sign-windows.sh --jsign-jar` signs through [jsign](https://ebourg.github.io/jsign/), which speaks to
  Azure Artifact Signing (`TRUSTEDSIGNING`), DigiCert KeyLocker (`DIGICERTONE`), SSL.com eSigner (`ESIGNER`), Azure
  Key Vault, Google Cloud KMS and AWS KMS, and detaches the signature the same way.
- A service with one long-lived certificate keeps the `windows_leaf_sha256` pin. Azure Artifact Signing issues a
  new certificate every day, so it is pinned by `windows_signer` instead (the organization, and the EKU Microsoft
  assigns to one validated identity), under roots in `docs/release/windows-ca.pem`;
  `ci/github/windows-verify-pinned.sh` then also requires a verified timestamp.
- What it would take: an account with the service (Azure quotes 1 to 30 business days to validate an
  organization), a step in `release-sign.yml` that signs in and runs `sign-windows.sh`, and a first live run to
  prove it. For Azure the sign-in would be GitHub's OIDC token, whose subject for this repository carries numeric
  IDs: `repo:PastaPastaPasta@6443210/dashwallet-desktop@1409635492:environment:release-signing`.
