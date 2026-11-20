# Release checklist

How a release owner cuts `vX.Y.Z` (or a release candidate `vX.Y.Z-rc.N`). Every step comes from the repository as
it is: `.github/workflows/release.yml`, `ci/github/release-*.sh`, the test bar in
[`RELEASE-PLAN.md`](../research/gap-2026-10-31/RELEASE-PLAN.md) and the 1.0 dates in
[`PLAN-1.0.md`](../PLAN-1.0.md). Steps marked **when available** need pasta's certificates or keys and are skipped
(and the notes say so) until they exist.

Work through it top to bottom and tick each box in the release's tracking issue or pull request.

## 1. Pick the commit

- [ ] The commit is on `main`. The workflow publishes nothing for a commit that isn't an ancestor of `main`
      (`ci/github/release-decide.sh`, then `release-publish.sh` again).
- [ ] CI and Release are green on that commit (Release's `main` push has already built it and published
      `v<version>-dev.<run>+<sha>`). Nightly's latest run on `main` is green, or each red job has an issue that says
      why it doesn't block. UI gate and Tauri selftest run only by hand (`workflow_dispatch`): run both on the
      commit and have them green.
- [ ] No open blocker or critical issue is tagged for this version.

## 2. Bump the version (one pull request)

The tag must equal the version in `rust/crates/dw-app/tauri.conf.json`: `ci/github/release-version.sh` fails the
run otherwise. The engine reads its own version from Cargo (`CARGO_PKG_VERSION`) and shows dash-qt's "pre-release
test build" warning unless the build version is exactly that, so Cargo and Tauri must agree.

- [ ] `rust/crates/dw-app/tauri.conf.json`: `"version": "X.Y.Z"`.
- [ ] `rust/Cargo.toml`: `[workspace.package] version = "X.Y.Z"`, then `cargo metadata --manifest-path rust/Cargo.toml
      --format-version 1 > /dev/null` to rewrite the workspace crates in `rust/Cargo.lock`.
- [ ] `apps/desktop/package.json`: `"version": "X.Y.Z"` (the npm SBOM's root component).
- [ ] `Apps/macOS/project.yml`: `MARKETING_VERSION: "X.Y.Z"`, while the Swift app is in the tree (U-08 removes it).
- [ ] Leave the fixture versions alone (`apps/desktop/src/fixture/scenarios.ts` and the tests that read it): they are
      test data, not the app's version.
- [ ] The release notes, `docs/release/X.Y.Z-notes.md` (section 3), in the same pull request.

A release candidate needs no bump: `vX.Y.Z-rc.N` takes the same `X.Y.Z` and the same notes file. Bump once for the
first candidate.

## 3. Write the notes

`release-publish.sh` uses `docs/release/X.Y.Z-notes.md` as the GitHub release body for `vX.Y.Z` and every
`vX.Y.Z-rc.N`; without the file the release gets a one-line body. Follow
[`0.1.0-notes.md`](0.1.0-notes.md):

- [ ] Title `# Dash Wallet Desktop X.Y.Z`, one paragraph on what the app is, and the bold warning while there is no
      completed security review ("No security review has been done. Use small amounts only.").
- [ ] **What is in X.Y**: by area, as the user sees it.
- [ ] **Known limitations**: what does not work or behaves surprisingly, in the user's words; signing and
      notarization state (section 6).
- [ ] **How X.Y was tested**: the test bar of section 4 as it was actually run, with OS and build.
- [ ] **Files**: one row per OS with the five bundles (Linux: AppImage, .deb, Flatpak), plus `SHA256SUMS` and the two SBOMs
      (`dash-wallet-X.Y.Z-sbom-rust.cdx.json`, `dash-wallet-X.Y.Z-sbom-npm.cdx.json`).
- [ ] **Install**: the first-launch steps for each OS (they change once signing exists).
- [ ] **Verify**: `gh attestation verify` and `sha256sum -c` for this version, and a pointer to
      [`verifying.md`](verifying.md).

## 4. Run the test bar

Test the commit that will ship: tag a release candidate (section 5) and test its `-rc.N` files, or, for a version
that skips candidates, the `v<version>-dev.<run>+<sha>` build of the chosen commit (it shows the test-build warning;
nothing else differs). The final tag rebuilds the same commit with its own version stamp, so section 7 installs the
published files once more.

The minimal bar (RELEASE-PLAN §1), on public testnet, funded with the `dash-faucet` skill, on the Linux AppImage:

- [ ] 1. Create a wallet, write down the phrase, restore it in a second data folder: same balance.
- [ ] 2. Receive and send tDASH, both in Activity with the fee; Export CSV matches.
- [ ] 3. Register a username; add a contact (a second wallet), accept, pay the contact; both sides show it.
- [ ] 4. Core → Shielded, Shielded → an external shielded address (second wallet), Shielded → Core.
- [ ] 5. Restore an iOS-created testnet phrase holding Platform and shielded funds: all three balances appear; move
      the Platform funds to Core.
- [ ] 6. Start and stop mixing.
- [ ] 7. The Release run's launch checks passed on macOS and Windows (`launch-macos`, `launch-windows` artifacts),
      plus the manual screenshots below.

For 1.0 (RELEASE-PLAN §2, 0.9 and 1.0), also:

- [ ] Interactive QA of every user story on macOS, Windows and Linux (`agentic-qa`), with no open blocker or
      critical; accessibility pass (H-05).
- [ ] Mainnet canary with an iOS user (H-07): name, contact round trip, payments both ways, invitation claim,
      shielded round trip.
- [ ] H-01 security review findings fixed or accepted in writing; the kill -9 matrix (H-03) green.
- [ ] Every user story in `docs/parity.md` done or marked N/A with a reason.
- [ ] Signed with the real release key (section 6): the tag and `SHA256SUMS`. Without the B2 certificates, 1.0 ships
      unsigned binaries with a signed `SHA256SUMS` and the notes say so (RELEASE-PLAN §2, 0.9).

A bug that blocks the bar is fixed before the tag; anything else becomes an issue for the next version.

### Manual screenshots: macOS and Windows

The workflow screenshots only Linux: on the hosted macOS and Windows runners the capture returns a black screen
(release.yml header). A person takes these at release time, from the candidate's files:

- [ ] macOS (a real Mac, Apple silicon or Intel): open the dmg, drag to Applications, first launch (the
      Gatekeeper prompt and the right-click ▸ Open path from the notes), onboarding, Home of a testnet wallet,
      Settings ▸ About showing `X.Y.Z`, and Tools ▸ Information with its test-build warning (a candidate shows
      it; the final tag must not).
- [ ] Windows 11 (a real machine or VM): run the installer (the SmartScreen prompt, More info ▸ Run anyway), first
      launch, onboarding, Home, Settings ▸ About, Tools ▸ Information.
- [ ] Light and dark. Testnet data only; no real phrase, no mainnet address with funds.
- [ ] Post them on the release's tracking issue or pull request, committed to a `pr-artifacts/…` branch and linked
      by SHA-pinned raw URLs (the repository's `CLAUDE.md` rule), never to `main`.

## 5. Tag

- [ ] Before tagging (DEC-1806): a manual Linux testnet smoke on the AppImage that `main` built for the commit to
      tag: a fresh install and an upgrade from the previous release; create, restore, sync, receive and send, and
      each of the release's headline features once. The evidence goes in the release pull request and the release
      text, never key material (no phrase, no private key, no passphrase).
- [ ] Optional dry run of the whole build without publishing: `git tag v0.0.0-rel<N> <sha> && git push origin
      v0.0.0-rel<N>`. A tag that is not `vX.Y.Z[-rc.N]` builds and never starts the publish job. Delete it after.
- [ ] Release candidate: `git tag -a vX.Y.Z-rc.N <sha> -m "Dash Wallet Desktop X.Y.Z-rc.N"`, push it. It becomes a
      prerelease, never Latest.
- [ ] Final: `git tag -a vX.Y.Z <sha> -m "Dash Wallet Desktop X.Y.Z"` on the same commit as the last candidate, unless
      a fix landed since (then a new candidate first).
- [ ] **When available:** sign the tag with the release key (`git tag -s`, and check it with `git tag -v vX.Y.Z`).
      RELEASE-PLAN §2 (0.9) has the real release key, PGP or a Sigstore identity, decided before 1.0. Until it
      exists the tag is annotated and unsigned, and the attestations are the only signature.
- [ ] `git push origin vX.Y.Z`. Never move or re-push a tag that has published: fix forward with a new version.

## 6. What the workflow publishes

The Release run on the tag (`.github/workflows/release.yml`):

1. `decide` checks the commit is on `main`; `macos`, `windows`, `linux` and `flatpak` build and launch-check the bundles; `sbom`
   writes the two SBOMs (`ci/github/release-sbom.sh`); `checksums` writes `SHA256SUMS` over all seven files.
2. `publish` (the only job with write permissions) attests every file and `SHA256SUMS` with
   `actions/attest-build-provenance`, then creates the release: a full release for `vX.Y.Z` (GitHub picks Latest by
   version), a prerelease for `-rc.N`. Seven assets:
   - `dash-wallet-X.Y.Z-macos-universal.dmg`, ad-hoc signed, not notarized;
   - `dash-wallet-X.Y.Z-windows-x64-setup.exe`, unsigned NSIS installer;
   - `dash-wallet-X.Y.Z-linux-x86_64.AppImage` and `dash-wallet-X.Y.Z-linux-amd64.deb`;
   - `dash-wallet-X.Y.Z-sbom-rust.cdx.json`, `dash-wallet-X.Y.Z-sbom-npm.cdx.json` (CycloneDX JSON);
   - `SHA256SUMS`.
3. A tag does not touch `continuous` or the per-build releases of `main`.
4. **When available (the updater key, [`updater.md`](updater.md)):** with the `TAURI_SIGNING_PRIVATE_KEY` secret set
   in the `release` environment, a final `vX.Y.Z` tag on main builds with the update feed and Tauri signs its updater
   artifacts; a candidate or a dry run never gets the key. For a final `vX.Y.Z` only, `checksums` writes `latest.json` (`ci/github/release-feed.sh`) and the release gains two more
   assets, both attested and listed in `SHA256SUMS`: `latest.json` and `dash-wallet-X.Y.Z-macos-universal.app.tar.gz`.
   A candidate never gets a feed. Without the secret the build jobs warn on a final tag and nothing else changes: the
   app says it does not update itself. Once `DW_UPDATER_REQUIRED=1` is set, a final tag without the key fails its
   builds, and `release-publish.sh` refuses a final release without `latest.json`.

Signing that the workflow does not do yet:

- [ ] **When available (B2):** macOS Developer ID signing and notarization (PK-02). Today `tauri.conf.json` has
      `"signingIdentity": "-"` (ad-hoc); wiring the certificate and notarization credentials into the `macos` job
      is its own pull request.
- [ ] **When available (B2):** Windows code signing of the installer (W-03).
- [ ] Until both exist, the notes' Known limitations and Install sections say the builds are not notarized and not
      signed, and what the first launch shows.
- [ ] **When available (the updater key):** turning updates on is once: make the key, put its public key in
      `tauri.conf.json`, create the `release` environment (deployment rule: tags `v*`) and add the two secrets there,
      never as repository secrets, then add a `v*` tag ruleset or required reviewers on `release`, and run one
      `workflow_dispatch` to check the no-environment build jobs ([`updater.md`](updater.md), "Turning it on"). The first signed release cannot reach
      copies installed before it; its notes say to install it by hand.
- [ ] **After the first signed release:** set the repository variable `DW_UPDATER_REQUIRED=1`, so a final release can
      no longer go out without `latest.json`. As Latest, such a release breaks the feed for every installed copy.
- [ ] **When available (the real release key, required for 1.0):** sign `SHA256SUMS` once the release is published
      and add the signature to it (`gh release upload vX.Y.Z <signature file>`). This is the RELEASE-PLAN fallback
      when the certificates don't exist: unsigned binaries, signed checksums. The notes' Verify section then says
      how to check the signature with the published key.

## 7. Verify what was published

On a machine with `gh` logged in, in an empty directory:

```sh
sha=$(gh api repos/PastaPastaPasta/dashwallet-desktop/commits/vX.Y.Z --jq .sha)   # the tag's commit
gh release download vX.Y.Z --repo PastaPastaPasta/dashwallet-desktop
ls | wc -l                                  # 7
sha256sum -c SHA256SUMS                     # every file OK
for f in *; do
  gh attestation verify "$f" --repo PastaPastaPasta/dashwallet-desktop \
    --signer-workflow PastaPastaPasta/dashwallet-desktop/.github/workflows/release.yml \
    --source-ref refs/tags/vX.Y.Z --source-digest "${sha:?no commit for vX.Y.Z}"
done
jq -r '.metadata.component.name, (.components | length)' dash-wallet-X.Y.Z-sbom-*.cdx.json
```

- [ ] Seven assets, `SHA256SUMS` checks, every file verifies against the tag and its commit.
- [ ] Both SBOMs parse, name `dw-app` and `dashwallet-desktop` at `X.Y.Z`, and hold no machine path
      (`grep -c 'file:///' dash-wallet-X.Y.Z-sbom-*.cdx.json` prints 0 for each).
- [ ] The release is marked as the notes intend (full release, or prerelease for a candidate), the body is
      `X.Y.Z-notes.md`, and the final tag is Latest.
- [ ] Install each bundle once more from the published files (the screenshots of section 4 can be these):
      Settings ▸ About shows `X.Y.Z`, and Tools ▸ Information shows no test-build warning on the final tag.
- [ ] With the updater key: the release has `latest.json` and the macOS app archive too, and the feed names this
      release (`gh release download vX.Y.Z -p latest.json -O - | jq '.version, .platforms[].url'`: `X.Y.Z`, and every
      URL under `/releases/download/vX.Y.Z/`). The feed URL answers with it once the release is Latest:
      `curl -sL https://github.com/PastaPastaPasta/dashwallet-desktop/releases/latest/download/latest.json | jq .version`.
- [ ] With the updater key: the previous signed release, installed as an AppImage, finds the update in Settings ▸
      About ▸ Check for updates, installs it and restarts into `X.Y.Z` (and the same on macOS and Windows, by hand).
- [ ] Update the State column of `docs/PLAN-1.0.md` and the README's tagged-release mention if it names a version.

## 8. Rollback

A published release cannot be made unpublished for people who downloaded it, and its attestations stay valid for its
bytes. So:

- [ ] A bad release: mark it at once with `gh release edit vX.Y.Z --repo PastaPastaPasta/dashwallet-desktop
      --prerelease --latest=false` and a warning at the top of its notes (`--notes-file`). Make the previous release
      Latest again, explicitly: `gh release edit v<previous> --repo PastaPastaPasta/dashwallet-desktop --latest`,
      and check the Releases page. Then ship the fix as `vX.Y.(Z+1)`, with a candidate if time allows.
- [ ] With the updater key, the bad release is also what the feed offers while it is Latest. Marking it as a
      prerelease (above) takes it out of the feed at once: the feed is the Latest release's `latest.json`. Copies that
      already installed it stay on it (no downgrade is ever offered), so the fix ships as a newer version.
      If the release made Latest again has no `latest.json` (it predates the key), Settings says that no update
      information is published until the fix is out: ship the fix soon, or make Latest a release that has one.
- [ ] Delete the release (`gh release delete vX.Y.Z --repo PastaPastaPasta/dashwallet-desktop --yes`) only when
      its files must not be downloaded at all (for example, they lose funds). Keep the tag: the version is used and
      is never re-cut with other bytes.
- [ ] A publish job that failed half way: `release-publish.sh` refuses a release that exists but is incomplete
      ("finish or delete it by hand"). Delete that release (not the tag) and re-run the failed jobs of the same
      Release run; it rebuilds nothing it already has.
- [ ] A tag pushed by mistake that has not published (the run is still building): cancel the run, then
      `git push origin :refs/tags/vX.Y.Z` and `git tag -d vX.Y.Z`.
- [ ] Keep the repository setting "Immutable releases" off (the `continuous` release depends on it, release.yml
      header).
