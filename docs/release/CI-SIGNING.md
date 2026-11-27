# Signing a release candidate in CI

[`RC-SIGNING.md`](RC-SIGNING.md) has the release signer sign a candidate on their own Mac. This file is the other
way to do the same job: `.github/workflows/release-sign.yml` runs the same scripts on a hosted runner, so nobody
handles a certificate after the one-time setup below. Both ways publish the same 9 assets through
`release-publish-signed.yml`, with the same checks. A stable `vX.Y.Z` is still signed locally
([`SIGNING.md`](SIGNING.md)): its updater feed needs the stable updater key, and that key is not in CI.

The examples use `v1.0.0-rc.2`. Substitute the real tag everywhere.

## How it works

One dispatch, two jobs:

| Job | Runs | Sees secrets | Can write to the repository |
|---|---|---|---|
| `sign` | `fetch-unsigned.sh`, `sign-windows.sh`, `sign-macos.sh`, `sign-checksums.sh` on `macos-15` | yes, through the `release-signing` environment only | no (`contents: read`) |
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
| Windows installer: Authenticode, SHA-256, RFC 3161 timestamp | a key held by Azure Artifact Signing | not in GitHub: the job signs in to Azure with GitHub's OIDC token |
| `SHA256SUMS.asc` | the release PGP key | secrets `RELEASE_PGP_PRIVATE_KEY`, `RELEASE_PGP_PASSPHRASE` |

## One-time setup

Only the repository owner can do these. Every secret goes straight from the owner's machine into GitHub; none is
sent to anyone, and no agent handles one. `R` below is `PastaPastaPasta/dashwallet-desktop`.

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

### 3. PGP: a signing subkey for `SHA256SUMS.asc` (10 minutes)

CI gets a signing subkey of your release key, under a passphrase of its own. The primary key stays with you: it
still signs the tag ([`RC-SIGNING.md`](RC-SIGNING.md) section 3), it is what `pgp_fingerprint` pins, and it can
revoke the subkey without anything from GitHub. Neither the primary key nor your passphrase goes to GitHub.

```sh
export PGP_FPR=<40-hex primary fingerprint of your release key>
gpg --quick-add-key "$PGP_FPR" ed25519 sign 2y                  # asks to unlock your primary key
gpg --list-secret-keys --with-subkey-fingerprints "$PGP_FPR"    # the new [S] subkey is the last one listed
SUB=<40-hex fingerprint of that subkey>

CI_HOME=$(mktemp -d) && chmod 700 "$CI_HOME"                    # a throwaway home that holds only the subkey
gpg --export-secret-subkeys "$SUB!" | gpg --homedir "$CI_HOME" --import
GRIP=$(gpg --homedir "$CI_HOME" --list-secret-keys --with-keygrip --with-colons |
  awk -F: -v want="$SUB" '$1 == "fpr" && $10 == want { s = 1; next } s && $1 == "grp" { print $10; exit }')
gpg-connect-agent --homedir "$CI_HOME" "passwd $GRIP" /bye      # your passphrase once, then a NEW one for CI, twice
gpg --homedir "$CI_HOME" --armor --export-secret-subkeys "$SUB!" |
  gh secret set RELEASE_PGP_PRIVATE_KEY --env release-signing --repo "$R"    # asks for the new passphrase
gh secret set RELEASE_PGP_PASSPHRASE --env release-signing --repo "$R"       # prompts: paste the new passphrase
gpgconf --homedir "$CI_HOME" --kill all && rm -rf "$CI_HOME"
gpg --delete-secret-keys "$SUB!"                                # your own keyring keeps signing with your usual subkey
gpg --armor --export "$PGP_FPR" > release-key.asc               # public, now with the CI subkey: step 5
```

A new subkey takes its primary's passphrase, which is why the subkey is given another one in a throwaway home
before it is exported. The last-but-one line removes the subkey's secret half from your own keyring: gpg signs
with the newest signing subkey it holds, and that should stay yours.

### 4. Windows: Azure Artifact Signing (30 minutes of work, then 1 to 30 business days of waiting)

A code-signing key issued today must live in certified hardware, so a hosted runner cannot hold one. Azure Artifact
Signing (the service formerly named Trusted Signing) holds the key and signs on request. It costs $9.99 a month
(Basic, 5,000 signatures). Public Trust is open to organizations in the United States, Canada, the European Union,
the United Kingdom and eight more countries, and to individual developers in the United States and Canada.
Microsoft's steps are in <https://learn.microsoft.com/azure/artifact-signing/quickstart>.

1. In <https://portal.azure.com>: a paid subscription; **Subscriptions ▸ Resource providers**, register
   `Microsoft.CodeSigning`; create an **Artifact Signing Account** (Basic) and note its region's endpoint, for
   example `https://eus.codesigning.azure.net`.
2. Give yourself the **Artifact Signing Identity Verifier** role on the account, then **Identity validations ▸ New
   identity ▸ Public**. The legal name you validate is the publisher name Windows shows. For an organization,
   Microsoft asks for the legal name, a business identifier, an address, a website, and a primary email address on
   a domain the organization owns, and it has the named person verify a government ID. Microsoft quotes 1 to 30
   business days.
3. When the validation is **Completed**: **Certificate profiles ▸ Create ▸ Public Trust**, and pick that identity.
4. Let this repository's `release-signing` environment sign, and nothing else:

   ```sh
   az login
   APP_ID=$(az ad app create --display-name dashwallet-desktop-release-signing --query appId --output tsv)
   az ad sp create --id "$APP_ID"
   SUBJECT="$(gh api "repos/$R/actions/oidc/customization/sub" --jq .sub_claim_prefix):environment:release-signing"
   echo "$SUBJECT"   # repo:PastaPastaPasta@6443210/dashwallet-desktop@1409635492:environment:release-signing
   az ad app federated-credential create --id "$APP_ID" --parameters "{
     \"name\": \"dashwallet-desktop-release-signing\",
     \"issuer\": \"https://token.actions.githubusercontent.com\",
     \"subject\": \"$SUBJECT\",
     \"audiences\": [\"api://AzureADTokenExchange\"]}"
   SUB=$(az account show --query id --output tsv)
   az role assignment create --assignee "$APP_ID" --role "Artifact Signing Certificate Profile Signer" \
     --scope "/subscriptions/$SUB/resourceGroups/<group>/providers/Microsoft.CodeSigning/codeSigningAccounts/<account>/certificateProfiles/<profile>"
   gh variable set AZURE_CLIENT_ID --env release-signing --repo "$R" --body "$APP_ID"
   gh variable set AZURE_TENANT_ID --env release-signing --repo "$R" --body "$(az account show --query tenantId --output tsv)"
   gh variable set AZURE_SUBSCRIPTION_ID --env release-signing --repo "$R" --body "$SUB"
   gh variable set AZURE_SIGNING_ENDPOINT --env release-signing --repo "$R" --body "https://<region>.codesigning.azure.net"
   gh variable set AZURE_SIGNING_ACCOUNT --env release-signing --repo "$R" --body "<account>"
   gh variable set AZURE_SIGNING_PROFILE --env release-signing --repo "$R" --body "<profile>"
   ```

   An older portal names the role `Trusted Signing Certificate Profile Signer`. These six values are identifiers,
   not secrets: the federated credential is what lets a job in, and only a job of this repository that the
   `release-signing` environment admitted. Microsoft Entra compares the subject exactly. This repository's tokens
   carry the owner's and the repository's numeric IDs in it (GitHub's immutable subject), so the name-only form
   `repo:PastaPastaPasta/dashwallet-desktop:…` does not match, and a rename or transfer of the repository needs a
   new credential.
5. Note the two public values that identify this signer for step 5: the organization name (`O=`) in the
   certificate's subject, and the EKU that starts `1.3.6.1.4.1.311.97.` and is not `1.3.6.1.4.1.311.97.1.0`:

   ```sh
   az artifact-signing certificate-profile show -g <group> --account-name <account> -n <profile> \
     --query 'certificates[0].{subject: subjectName, eku: enhancedKeyUsage}'
   ```

**Until the validation completes**, pass `windows_sig_ref` when you dispatch (below). The Windows signature then
comes from the YubiKey, made with [`RC-SIGNING.md`](RC-SIGNING.md) steps 4.1, 4.3 and 5, and macOS and PGP are
still signed in CI.

### 5. Pin the public identities (one pull request to `main`, before the tag)

`release-publish-signed.yml` reads these from the tagged commit, and placeholders fail closed.
`docs/release/signing-identities.json`:

```json
{
  "schema": 1,
  "pgp_fingerprint": "<40 hex, from step 3>",
  "apple_team_id": "<10 characters, in parentheses in the Developer ID identity's name>",
  "windows_signer": {
    "organization": "<the organization name on the Azure certificate, exactly>",
    "eku": "<1.3.6.1.4.1.311.97.… from step 4>"
  }
}
```

- `docs/release/release-key.asc`: the file from step 3.
- `docs/release/windows-ca.pem`: with `windows_signer`, this file is required and is the only trust for the Windows
  signature and its timestamp. For Azure Artifact Signing it holds one certificate, the *Microsoft Identity
  Verification Root Certificate Authority 2020*
  (<https://www.microsoft.com/pkiops/certs/Microsoft%20Identity%20Verification%20Root%20Certificate%20Authority%202020.crt>,
  converted with `openssl x509 -inform DER -in <file>.crt`; its SHA-256 fingerprint is
  `5367F20C7ADE0E2BCA790915056D086B720C33C1FA2A2661ACF787E3292E1270`).

Azure issues a new leaf certificate every day, valid for three days, so a leaf hash cannot pin it. `windows_signer`
pins what does not change: the organization, and the EKU Microsoft assigns to one validated identity and to no
other. `release-attach.sh` finds the one certificate in the signature with both, then has `osslsigncode` require
that certificate as the signer and verify its chain and the timestamp's chain to `windows-ca.pem`. While the
Windows signature still comes from the YubiKey, pin `windows_leaf_sha256` as in `RC-SIGNING.md` instead; the file
may hold one of the two, never both.

## Each candidate

1. The version bump, the notes and the pins of step 5 are on `main`, and `DW_SIGN_RC` is `1`
   ([`RC-SIGNING.md`](RC-SIGNING.md) section 1). Push the tag and wait for its Release run
   ([`RC-SIGNING.md`](RC-SIGNING.md) section 3). It ends with the notice that the tag holds unsigned binaries.
2. Rehearse. This signs for real and verifies everything, then stops before attesting and publishing:

   ```sh
   R=PastaPastaPasta/dashwallet-desktop TAG=v1.0.0-rc.2
   RUN_ID=$(gh run list --repo "$R" --workflow release.yml --branch "$TAG" --event push --limit 1 --json databaseId --jq '.[0].databaseId')
   gh workflow run release-sign.yml --repo "$R" --ref "$TAG" -f tag="$TAG" -f release_run_id="$RUN_ID" -f dry_run=true
   ```

   Open the run, check it is on your tag and commit, and choose **Review deployments ▸ release-signing ▸ Approve
   and deploy**. Most of the `sign` job's time is Apple's notary service, usually a few minutes for each of its two
   submissions. If the `release` environment has a required reviewer too, the `publish` job asks once more; give that
   approval within three days, while the signing job's artifact still exists. A green rehearsal ends with the notice `dry_run=true on
   v1.0.0-rc.2 …`.
3. Publish: the same command without `-f dry_run=true`, and the same approval.
4. Verify the published candidate as in [`RC-SIGNING.md`](RC-SIGNING.md) section 7. With a `windows_signer` pin
   the identities file has no leaf hash to require, so find the leaf the pin selects and require that one. From a
   checkout of the tag:

   ```sh
   exe=dash-wallet-1.0.0-rc.2-windows-x64-setup.exe ids=docs/release/signing-identities.json
   osslsigncode extract-signature -pem -in "$exe" -out sig.pem
   leaf=$(python3 ci/github/windows-signer-leaf.py sig.pem \
     "$(jq -r .windows_signer.organization "$ids")" "$(jq -r .windows_signer.eku "$ids")")
   osslsigncode verify -in "$exe" -require-leaf-hash "$leaf" \
     -CAfile docs/release/windows-ca.pem -TSA-CAfile docs/release/windows-ca.pem
   # expect "Leaf hash match: ok", a "Timestamp time:" line and "Succeeded"
   ```

   The organization alone is not the check: only the EKU is unique to this publisher.

With the Windows signature from the YubiKey, add `-f windows_sig_ref=<commit in dashwallet-detached-sigs>` to both
commands.

If a run fails, nothing is published. A wrong secret or variable is fixed in the environment and the same command
is run again. A fault in a script or in a pin needs a fix on `main` and the next candidate, because both are read
from the tagged commit.

The `sign` job installs `osslsigncode` and `gnupg` from Homebrew at their current versions, as the signer's Mac
does; `jsign` is fetched at a pinned version and checked against a pinned SHA-256. Those two Homebrew packages
and what they depend on therefore run in a job that holds the PGP key and can sign with Azure, and the approved
tag does not fix their versions. That is the same trust the local runbook places in Homebrew, and it is not yet
removed here.

## Rotating and revoking

- **Apple:** revoke the API key in App Store Connect and upload a new one at any time. Revoke the certificate only
  if its key leaked: Gatekeeper then refuses every app signed with it.
- **Windows:** delete the federated credential or the role assignment and CI can no longer sign. Nothing in GitHub
  needs changing.
- **PGP:** revoke the CI subkey with your primary key (`gpg --edit-key "$PGP_FPR"`, select the subkey, `revkey`),
  commit the updated `release-key.asc`, and add a new subkey as in step 3. The pinned fingerprint does not change.
- **Shutting CI signing down:** first delete the Azure federated credential (or the role assignment) and check
  that a sign-in fails; only then delete the `release-signing` environment, which removes every secret at once.
  The order matters. Azure trusts the environment by its name, and a workflow that names an environment that
  does not exist creates it again without the reviewer and the tag rule. If Azure cannot be cut off at once,
  keep the environment and its rules and delete the secrets one by one.

## Another Windows key holder

`sign-windows.sh --jsign-jar` signs through [jsign](https://ebourg.github.io/jsign/), which also speaks to DigiCert
KeyLocker (`DIGICERTONE`), SSL.com eSigner (`ESIGNER`), Azure Key Vault, Google Cloud KMS and AWS KMS. Those hold
one long-lived certificate, so they keep the `windows_leaf_sha256` pin; moving to one means changing the store
type, keystore, alias and token in `release-sign.yml`'s Windows step, and nothing else.
