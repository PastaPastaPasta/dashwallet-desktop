# Updates (PK-03)

Dash Wallet can update itself from a signed feed on this repository's GitHub Releases. It is off unless all three of
these allow it:

1. **The build.** Only a build made with the updater signing key has the feed. The public key in
   `rust/crates/dw-app/tauri.conf.json` (`plugins.updater.pubkey`) is a placeholder until the key exists. release.yml
   gives the key only to a pushed `v*` tag's build jobs, from the `release` environment, and `ci/github/tauri-build.sh`
   then merges `rust/crates/dw-app/tauri.updater.conf.json` (the feed URL and `bundle.createUpdaterArtifacts`).
   Without both, the build has no updater plugin at all, and Settings ▸ About says that this build does not update
   itself and links to the releases page.
2. **The install.** A Flatpak is updated by Flatpak, and a .deb or .rpm by the system's package manager. A macOS app
   run from the disk image or from a translocated path cannot replace itself: Settings asks the user to move it to
   Applications first. The AppImage, the Windows installer (NSIS) and the macOS app in a writable folder update
   themselves.
3. **The user.** Settings ▸ About ▸ Updates has "Check for updates automatically", off by default, and a "Check for
   updates" button. Nothing is fetched until the user turns the setting on (which checks once then) or presses the
   button.

A found update is only shown, with its version and a link to its release notes. It is downloaded, its signature
checked and installed when the user clicks **Download and install**, and the app restarts when the user clicks
**Restart now**, through the normal quit path: the engine locks and flushes the wallets first. On Windows the
installer must replace the running app, so the one button is **Install and restart**; the engine is shut down the
same way before the installer starts. If the installer then fails to start, Dash Wallet says so and asks for a
restart. Install and Restart wait while a payment is being sent, a wallet mixes, a Dash Platform step runs or Dash
Wallet is still starting.

What is checked (`rust/crates/dw-app/src/updates.rs`):

- The signature, against the public key built into the app. Each artifact is signed by the Tauri CLI, and the
  signature names the version it was signed for (`requireSignedVersion`). A feed that pairs a newer version number
  with an older signed build is refused, and so is a signature that names no version.
- Only a stable version newer than the running one is offered. A pre-release or an older version is not, and nothing
  can downgrade the app.
- The feed is `https://github.com/PastaPastaPasta/dashwallet-desktop/releases/latest/download/latest.json`. GitHub's
  "latest" is the newest full release, never a pre-release. `ci/github/release-feed.sh` writes `latest.json` for
  `vX.Y.Z` tags only, and `release-publish.sh` refuses one anywhere else. When the Latest release has no
  `latest.json`, the app says that no update information is published, not that the server is unreachable.
- The check gives up after 30 s and a download after 60 s without data, so a stalled connection never blocks the
  next check.
- The request carries no wallet data and no identifier: it is the static URL, the updater's user agent and an
  `Accept` header. GitHub sees the IP address and the time.

## Turning it on (once, by pasta)

1. Make the key pair on a trusted machine, with a passphrase:

   ```sh
   cd apps/desktop
   pnpm tauri signer generate -w ~/.tauri/dash-wallet-updater.key
   ```

   This writes the private key to `~/.tauri/dash-wallet-updater.key` and the public key to
   `~/.tauri/dash-wallet-updater.key.pub`. Back the private key and its passphrase up offline. Every installed copy
   trusts this one key: if it is lost, no installed copy can be updated again, and users must reinstall by hand. If
   it leaks, whoever has it can ship an update. Never commit it or paste it into an issue or a chat.
2. Put the public key in the app. Replace the placeholder value of `plugins.updater.pubkey` in
   `rust/crates/dw-app/tauri.conf.json` with the one line of `dash-wallet-updater.key.pub`, in a pull request of its
   own. The tests do not depend on the value.
3. Create the `release` environment and put the two secrets in it. Repository-wide secrets are not supported: the build
   jobs read the key only through this environment, so a repository secret is never used.

   - Settings ▸ Environments ▸ New environment `release`. Under "Deployment branches and tags" choose "Selected
     branches and tags" and add the tag rule `v*`, so only a release tag's jobs can read the secrets, whatever a
     branch's workflow file says.
   - Add the secrets to that environment (the value is the file's content, not its path):

     ```sh
     gh secret set TAURI_SIGNING_PRIVATE_KEY --env release --repo PastaPastaPasta/dashwallet-desktop < ~/.tauri/dash-wallet-updater.key
     gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --env release --repo PastaPastaPasta/dashwallet-desktop
     ```

   A `workflow_dispatch` run never gets the key, even on a tag: it builds the plain app.
4. The next release tag then builds with the feed. Its release gains two assets, `latest.json` and
   `dash-wallet-X.Y.Z-macos-universal.app.tar.gz` (the macOS update), and both are listed in SHA256SUMS. The AppImage
   and the Windows installer are their own updates, so their signatures are only in `latest.json`.
5. Once that release is out, set the repository variable `DW_UPDATER_REQUIRED` to `1`
   (`gh variable set DW_UPDATER_REQUIRED --body 1 --repo PastaPastaPasta/dashwallet-desktop`). From then on a stable tag
   whose build has no key fails, and `release-publish.sh` refuses a stable release without `latest.json`: published as
   Latest, it would leave every installed copy without updates. Until then a stable tag without the key only warns.

Copies installed before the first signed release have no updater: their users install that release by hand once.

## Trying it locally

A with-key build needs no secret from CI: use a throwaway key and delete everything it made afterwards.

```sh
cd apps/desktop
pnpm tauri signer generate --ci -p '' -w /tmp/throwaway.key
# Put the content of /tmp/throwaway.key.pub in tauri.conf.json for this build only; do not commit it.
TAURI_SIGNING_PRIVATE_KEY="$(cat /tmp/throwaway.key)" TAURI_SIGNING_PRIVATE_KEY_PASSWORD='' \
  ../../ci/github/tauri-build.sh --bundles appimage
ls ../../rust/target/release/bundle/appimage/   # the AppImage and its .sig
```

Without the key, `pnpm tauri build` builds as before, makes no `.sig` files, and Settings ▸ About says this build
does not update itself.
