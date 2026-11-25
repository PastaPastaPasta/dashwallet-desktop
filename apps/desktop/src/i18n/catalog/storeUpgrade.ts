import type { Tree } from "../core";

/** The screens the engine gives for a refused open, by their keys; each value is the engine's English
 * (dw-store-upgrade's copy), `refused` the refusal screen's lines and buttons. The screen shows the
 * engine's own text for a key missing here. `folder` is the network's data folder. `retry` is the
 * screen for an upgrade that stopped on the machine (`copy::retry_copy_for`). `notice` is the banner over
 * the wallet once an upgrade opened it without a before-upgrade copy it can restore (`copy::notice_line`);
 * its `dismiss` is the renderer's own. `restore` and
 * `restored` are the restore flow's (MG's restore copy): the renderer's own screens for `storage_restore`,
 * its confirm and banner, and the foreign screen's keys, which come in the engine's screen. `refused.recover`
 * is the damaged store's way out (Q107 C1) in the engine's screen; `recover` is the renderer's own
 * confirm for it, `folder` there the refused folder. `notPrivate`
 * is the renderer's own screen for `storage_not_private`: `folder` there is the directory others can
 * change, and `reason` says why. */
export const storeUpgrade = {
  recover: {
    confirm: {
      title: "Set up from a recovery phrase?",
      text: "Dash Wallet restarts with a new, empty data folder, and you enter a wallet's recovery phrase. It doesn't change or delete anything in {folder}.",
      confirm: "Restart and set up",
      cancel: "Cancel",
    },
  },
  notPrivate: {
    title: "Dash Wallet can't use this data folder",
    text: "Other accounts on this computer can change files in {folder}. To keep your wallet safe, Dash Wallet won't open it until only your account can change that folder.\n\nChange the folder's permissions so only you can write to it, then try again. Your wallet files weren't changed.",
    reason: {
      groupWritable: "Your group can write to it.",
      otherWritable: "Every account can write to it.",
      otherOwner: "Another account owns it.",
    },
    retry: "Try again",
    report: "Report a problem",
    quit: "Quit",
  },
  refused: {
    title: "Dash Wallet can't open this wallet data safely",
    intro: "This version of Dash Wallet can't safely open the wallet data in {folder}",
    unchanged:
      "Dash Wallet didn't change anything in it. Keep {folder} as it is: it may hold the only record of recent payments, and support may need it.",
    changedByUpgrade:
      "Dash Wallet had already started upgrading it. Keep {folder} as it is: it may hold the only record of recent payments, and support may need it.",
    keep: "Keep {folder} as it is: it may hold the only record of recent payments, and support may need it.",
    quit: "Quit",
    report: "Report a problem",
    restore: "Restore earlier data",
    recover: "Set up from recovery phrase",
    detail: {
      forkUnsupported: "it was written by a version of Dash Wallet that this one can't upgrade from.",
      shapeMismatch:
        "its layout doesn't match the version it says it is, so another program or a test build may have changed it.",
      rehearsalFailed: "a trial upgrade on a copy of it failed, so Dash Wallet left the real data as it was.",
      devBuild: "it was written by a development build of Dash Wallet.",
      tooNew: "it was written by a newer version of Dash Wallet.",
      historyMismatch:
        "its upgrade history doesn't match any released version of Dash Wallet, so this version can't tell what it contains.",
      contactsTable: "it holds contact data that this version can't read yet.",
      selfCheck: "a check Dash Wallet runs on itself before it opens any wallet data didn't pass.",
      unexpected: "it's in a state this version doesn't expect.",
      damaged: "it can't be read as wallet data, so it may be damaged.",
    },
    wayOut: {
      previous:
        "Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",
      newer:
        "Install that version of Dash Wallet, or a later one, to open it. Don't open it with an earlier version.",
      restore:
        "Earlier versions of Dash Wallet can't open it either, because it has already been upgraded. To go back to your wallet data from before the upgrade, choose Restore earlier data. Otherwise, choose Report a problem.",
      unverified:
        "Earlier versions of Dash Wallet can't open it either, because it has already been upgraded. A backup from before the upgrade exists, but it doesn't check out, so Dash Wallet won't restore from it. Choose Report a problem.",
      support: "Choose Report a problem.",
      supportMigrated:
        "Earlier versions of Dash Wallet can't open it either, because it has already been upgraded. Choose Report a problem.",
      selfCheck:
        "Quit Dash Wallet and open it again. If you see this again, free up some disk space and try once more. If it still happens, install the latest version of Dash Wallet. Until then, don't open this wallet data with an earlier version.",
      selfCheckNotMigrated:
        "Quit Dash Wallet and open it again. If you see this again, free up some disk space and try once more. If it still happens, install the latest version of Dash Wallet, or keep using the version you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",
      damaged:
        "Choose Set up from recovery phrase to set up a wallet again in a new data folder, then add any other wallets, each from its own recovery phrase. Dash Wallet leaves {folder} as it is and doesn't delete anything. Balances, history, Shielded funds, identities, usernames and DashPay contacts are rebuilt from the network. Imported keys, watch-only wallets and masternode keys aren't, so add them again. Other things saved only in this wallet data, such as labels and your address book, aren't brought across, but they stay in {folder}. If a governance proposal, a username or identity registration, a credit top-up or a masternode transaction was in progress, or you don't have a recovery phrase, choose Report a problem first.",
    },
  },
  retry: {
    noSpace: {
      title: "Dash Wallet couldn't finish upgrading your wallet data",
      text: "The disk is full, or Dash Wallet couldn't write to it. Free up some space, then quit Dash Wallet and open it again. Until then, don't open this wallet data with an earlier version.",
    },
  },
  notice: {
    snapshotDamaged:
      "Dash Wallet upgraded your wallet data. The copy it saved from before the upgrade doesn't check out, so Dash Wallet won't offer to restore it. Your wallet opened as usual. Don't delete anything in upgrade-backups: support may need it.",
    snapshotOther:
      "Dash Wallet upgraded your wallet data. The copy in upgrade-backups from before the upgrade doesn't match this wallet, so Dash Wallet won't offer to restore it. Your wallet opened as usual. Don't delete anything in upgrade-backups: support may need it.",
    dismiss: "Dismiss",
  },
  restored: {
    text: "Dash Wallet is using your wallet data from before the upgrade, so anything that happened since the upgrade is missing. Until the wallet has finished syncing, coins and Shielded notes you've already spent may show as unspent, so anything that moves your funds is paused until then.",
  },
  restore: {
    banner: "Anything that moves your funds is paused until syncing finishes.",
    confirm: {
      title: "Restore wallet data from before the upgrade?",
      text: "Dash Wallet restarts and puts back the wallet data it saved before the upgrade, so the wallet won't show anything recorded since then. Until the wallet has finished syncing, coins and Shielded notes you've already spent may show as unspent. The wallet data recorded since the upgrade is kept in the upgrade-backups folder inside {folder}. Don't delete it: it may hold the only record of a recent payment.\n\nThis version of Dash Wallet pauses sending until syncing finishes. An earlier version doesn't. If you open the wallet with one, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.\n\nIf Dash Wallet closes before the restore finishes, start this version again before you open an earlier one.",
      confirm: "Restart and restore",
      cancel: "Cancel",
    },
    refused: {
      title: "The earlier wallet data couldn't be restored",
      text: "Dash Wallet couldn't use the wallet data it saved before the upgrade, so nothing was changed. The wallet is using the same data as before you chose to restore.",
      ok: "OK",
    },
    noSpace: {
      title: "Not enough disk space to restore",
      text: "Dash Wallet needs more free disk space to restore your wallet data from before the upgrade, so nothing was changed. Free up space on the disk that holds {folder}, then choose to restore again.",
      ok: "OK",
    },
    notStarted: {
      title: "Dash Wallet couldn't start restoring your wallet data",
      text: "Dash Wallet couldn't start putting back the wallet data it saved before the upgrade, so nothing was changed: your wallet data is as it was before you chose to restore. Check that Dash Wallet can read and write {folder} and that no other program is using it, then restart Dash Wallet to try again. To keep your current wallet data instead, choose Cancel restore. If it still can't start, choose Report a problem.",
      restart: "Restart",
      cancel: "Cancel restore",
      report: "Report a problem",
      quit: "Quit",
    },
    incomplete: {
      title: "Dash Wallet couldn't finish restoring your wallet data",
      text: "Dash Wallet stopped partway through putting back the wallet data it saved before the upgrade, so it hasn't opened the wallet data in {folder}. The data saved before the upgrade and the data it was replacing are both still in the upgrade-backups folder inside {folder}: don't delete anything there. Make sure the disk has free space and that Dash Wallet can write to {folder}, then restart Dash Wallet to finish. Don't open an earlier version of Dash Wallet until this one has finished. If it still can't finish, choose Report a problem.",
      restart: "Restart",
      report: "Report a problem",
      quit: "Quit",
    },
    foreign: {
      title: "These wallet files don't match the restore",
      text: "Dash Wallet was restoring your wallet data from before the upgrade, but the wallet files now in {folder} don't match that data, so it stopped without changing them. Another version of Dash Wallet or another program may have replaced them.",
      keep: "Keep the current files",
      keepMeaning:
        "This stops the restore and opens the files in {folder} as they are now. Nothing is deleted: the wallet data saved before the upgrade, and the copy taken when the restore started, stay in the upgrade-backups folder.",
      quit: "Quit",
    },
  },
} as const satisfies Tree;
