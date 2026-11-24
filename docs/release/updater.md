# Updates (PK-03)

Dash Wallet can update itself from a signed feed on this repository's GitHub Releases. It is off unless all three of
these allow it:

1. **The build.** Only a stable release build has the feed enabled. `rust/crates/dw-app/tauri.conf.json`
   carries the stable release public key (`plugins.updater.pubkey`) and no endpoint, so the public key alone
   turns nothing on. On a pushed stable tag `vX.Y.Z` on `main` (`decide` job's `stable_tag`), `release.yml`
   passes `DW_ENABLE_UPDATER=1` to `ci/github/tauri-build.sh`, which merges `rust/crates/dw-app/tauri.updater.conf.json`
   (adding the feed URL) with `bundle.createUpdaterArtifacts: false` so CI builds unsigned with zero private keys.
   The stable updater private key lives **only** with the release signer locally (never in CI); `ci/release/sign-updater.sh`
   signs the final Linux AppImage, signed Windows installer, and signed macOS `.app.tar.gz` with
   `pnpm tauri signer sign --app-version <VERSION>` to produce `latest.json` ([`SIGNING.md`](SIGNING.md)).
   Every other build (main, pull requests, rc, dry runs, dev, local) has no feed: no updater plugin at all, and
   Settings ▸ About says that this build does not update itself and links to the releases page.
2. **The install.** A Flatpak is updated by Flatpak, and a .deb or .rpm by the system's package manager. A macOS app
   run from the disk image or from a translocated path cannot replace itself: Settings asks the user to move it to
   Applications first. Any app under `/Volumes/` counts as the disk image, so one installed on a writable external
   drive is asked to move too; that is the conservative choice. The AppImage, the Windows installer (NSIS) and the
   macOS app in a writable folder update themselves.
3. **The user.** Settings ▸ About ▸ Updates has "Check for updates automatically", off by default, and a "Check for
   updates" button. Nothing is fetched until the user turns the setting on (which checks once then) or presses the
   button.

A found update is only shown, with its version and **Release notes**, which opens that release's page
(`…/releases/tag/vX.Y.Z` on this repository). It is downloaded, its signature checked and installed when the user
clicks **Download and install**, and the app restarts when the user clicks **Restart now**, through the normal quit
path: the engine locks and flushes the wallets first. Only the steps that close the engine wait while a payment is
being sent, a wallet mixes, a Dash Platform step runs or Dash Wallet is still starting: Restart everywhere, and on
Windows Install. On macOS and Linux the download and install run alongside mixing.

On Windows the installer must replace the running app, so the one button is **Install and restart**; the engine is
shut down the same way before the installer starts. The busy check runs at the click and again once the download is
done, just before the engine closes. If something started meanwhile, the download is kept and the card says the
update is ready to install, with why it waits. If the installer then fails to start, Dash Wallet shows its window
again (Tauri hid it, and removed the tray icon, before the installer ran) and asks for a restart, over every screen.

What is checked (`rust/crates/dw-app/src/updates.rs`):

- The signature, against the public key built into the app. Each artifact is signed by the Tauri CLI, and the
  signature names the version it was signed for (`requireSignedVersion`). A feed that pairs a newer version number
  with an older signed build is refused, and so is a signature that names no version.
- Only a stable version newer than the running one is offered. A pre-release or an older version is not, and nothing
  can downgrade the app.
- The feed is `https://github.com/PastaPastaPasta/dashwallet-desktop/releases/latest/download/latest.json`. GitHub's
  "latest" is the newest full release, never a pre-release. `ci/github/release-feed.sh` writes `latest.json` for
  `vX.Y.Z` tags only, and `release-publish.sh` refuses one anywhere else. When the feed answers 404 (the Latest release
  has no `latest.json`), the app says that no update information is published. Any other failed answer (a 403, a 5xx,
  a proxy's refusal) says that the update server could not be reached. The plugin reports both the same way, so the
  app asks the feed URL once more for its status in that case only.
- The check gives up after 30 s and a download after 60 s without data, so a stalled connection never blocks the
  next check.
- The request carries no wallet data and no identifier: it is the static URL, the updater's user agent and an
  `Accept` header. The status request after a failed check is the same URL with neither header. GitHub sees the IP
  address and the time.

## The stable updater key (local-only, Dash Core detached-sigs model)

- **No stable updater private key lives in GitHub Actions.** The stable updater key is held only locally by the
  release signer and used by `ci/release/sign-updater.sh` (`pnpm tauri signer sign --app-version <VERSION>`) after
  macOS and Windows code-signing completes; see [`SIGNING.md`](SIGNING.md).
- The currently committed public key in `rust/crates/dw-app/tauri.conf.json` is minisign key ID `ED322B963B527B21`
  (the earlier key ID `7A4629E8E5438F3C` is dead: never use it). Because `ED322B963B527B21` was originally uploaded
  to the GitHub `release` environment during PK-03 setup, **rotate it before 1.0**: remove
  `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` from GitHub Actions, generate a fresh
  local-only stable updater key pair, commit its public key to `tauri.conf.json` (and update `RELEASE_KEY_ID` in
  `rust/crates/dw-app/src/updates_tests.rs`), and keep an encrypted offline backup.
- If the private key is lost or has to be rotated, the app must be re-keyed: a new key pair, its public key in
  `tauri.conf.json`, and a new signed release. Copies installed before the re-keyed release trust only the old key,
  so their users install that release by hand once.
- A signed stable release gains `latest.json` and `dash-wallet-X.Y.Z-macos-universal.app.tar.gz` (plus
  `SHA256SUMS.asc`), and `latest.json` + `.app.tar.gz` are listed in `SHA256SUMS`. The AppImage and the Windows
  installer are their own updates, so their minisign signatures live only in `latest.json`.
- Final stable tags (`vX.Y.Z`) fail closed automatically: `release.yml` holds back unsigned binaries and
  `release-publish.sh` refuses any final `vX.Y.Z` release missing verified attached signatures (`SHA256SUMS.asc`,
  `latest.json` + `.app.tar.gz`, and `signed-manifest.json` with all 11 hashes and pinned identities) unless
  `DW_ALLOW_UNSIGNED_STABLE=1` is explicitly set. Release candidates (`vX.Y.Z-rc.N`) never require signing and
  never publish `latest.json`.

Copies installed before the first signed release have no updater: their users install that release by hand once.

## Trying it locally

A with-key build needs no secret from CI: use a throwaway key and delete everything it made afterwards.

```sh
cd apps/desktop
pnpm tauri signer generate --ci -p '' -w /tmp/throwaway.key
# Put the content of /tmp/throwaway.key.pub in tauri.conf.json for this build only; do not commit it, and put the
# release key back afterwards.
TAURI_SIGNING_PRIVATE_KEY="$(cat /tmp/throwaway.key)" TAURI_SIGNING_PRIVATE_KEY_PASSWORD='' \
  ../../ci/github/tauri-build.sh --bundles appimage
ls ../../rust/target/release/bundle/appimage/   # the AppImage and its .sig
```

Without the key, `pnpm tauri build` builds as before, makes no `.sig` files, and Settings ▸ About says this build
does not update itself.
