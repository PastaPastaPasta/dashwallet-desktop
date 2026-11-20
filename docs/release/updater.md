# Updates (PK-03)

Dash Wallet can update itself from a signed feed on this repository's GitHub Releases. It is off unless all three of
these allow it:

1. **The build.** Only a build made with the updater signing key has the feed. `rust/crates/dw-app/tauri.conf.json`
   carries the release public key (`plugins.updater.pubkey`, minisign key ID `ED322B963B527B21`) and no endpoint, so
   the public key alone turns nothing on. release.yml gives the private key only to the build jobs of a pushed stable
   tag `vX.Y.Z` on main (the `decide` job's `signing`), from the `release` environment, and `ci/github/tauri-build.sh`
   then merges `rust/crates/dw-app/tauri.updater.conf.json` (the feed URL and `bundle.createUpdaterArtifacts`).
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
  `Accept` header. GitHub sees the IP address and the time.

## The release key (set up 2026-11-20)

- The key pair is minisign key ID `ED322B963B527B21`. Its public key is in `tauri.conf.json`. The earlier key ID
  `7A4629E8E5438F3C` is dead: never use it.
- The private key and its passphrase are the secrets `TAURI_SIGNING_PRIVATE_KEY` and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, only in the `release` environment. That environment exists, with the
  deployment tag rule `v*`. There are no repository-level secrets: the build jobs read the key only through the
  environment, so a repository secret would never be used.
- **No offline backup of the private key exists.** If the environment's secret is lost or has to be rotated, the app
  must be re-keyed: a new key pair, its public key in `tauri.conf.json`, and new secrets. Copies installed before the
  re-keyed release trust only the old key, so their users install that release by hand once. If the key leaks,
  whoever has it can ship an update: re-key the same way.
- `v*` matches rc and dry-run tags too. What keeps the key to stable tags is the build jobs' condition, the `decide`
  job's `signing` output (a pushed `vX.Y.Z` tag on main), not the environment rule.

## Still open

1. **Pasta's choice: a tag ruleset or required reviewers on `release`.** The `v*` rule matches a tag's name, and
   `decide` is code of the pushed commit, so anyone who may push a `v*` tag could push one on a commit whose workflow
   prints the key. One of these closes that:
   - **A tag ruleset** (no wait at release time). Settings ▸ Rules ▸ Rulesets ▸ New ruleset ▸ New tag ruleset: name
     `release tags`, enforcement Active, target tags matching `v*`, rules "Restrict creations", "Restrict updates" and
     "Restrict deletions", and add the Repository admin role to the bypass list. Or:

     ```sh
     gh api -X POST repos/PastaPastaPasta/dashwallet-desktop/rulesets --input - <<'JSON'
     {"name": "release tags", "target": "tag", "enforcement": "active",
      "bypass_actors": [{"actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always"}],
      "conditions": {"ref_name": {"include": ["refs/tags/v*"], "exclude": []}},
      "rules": [{"type": "creation"}, {"type": "update"}, {"type": "deletion"}]}
     JSON
     ```

   - **Required reviewers on `release`** (each stable tag's three build jobs wait for an approval). Settings ▸
     Environments ▸ `release` ▸ "Required reviewers": add yourself and save. Then approve the jobs under the tag's run
     ("Review deployments") when they ask.
2. A `workflow_dispatch` run never gets the key, even on a tag: it builds the plain app. It is also the first run whose
   build jobs use `environment: ''` (no environment), which GitHub does not document: run
   `gh workflow run release.yml --ref <branch>` once before the first signed release and check that the three build
   jobs start and finish. A main push or the schedule takes the same path.
3. The next stable release tag then builds with the feed. Its release gains two assets, `latest.json` and
   `dash-wallet-X.Y.Z-macos-universal.app.tar.gz` (the macOS update), and both are listed in SHA256SUMS. The AppImage
   and the Windows installer are their own updates, so their signatures are only in `latest.json`.
4. Once that release is out, set the repository variable `DW_UPDATER_REQUIRED` to `1`
   (`gh variable set DW_UPDATER_REQUIRED --body 1 --repo PastaPastaPasta/dashwallet-desktop`). From then on a stable tag
   whose build has no key fails, and `release-publish.sh` refuses a stable release without `latest.json`: published as
   Latest, it would leave every installed copy without updates. Until then a stable tag without the key only warns.

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
