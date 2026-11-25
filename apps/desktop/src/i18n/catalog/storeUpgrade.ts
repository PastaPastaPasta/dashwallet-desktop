import type { Tree } from "../core";

/** The screens the engine gives for a refused open, by their keys; each value is the engine's English
 * (dw-store-upgrade's copy). The screen shows the engine's own text for a key missing here. `folder` is
 * the network's data folder. `restore` and `restored` are the restore flow's (MG's restore copy): the
 * renderer's own screens for `storage_restore`, its confirm and banner, and the foreign screen's keys,
 * which come in the engine's screen. `notPrivate` is the renderer's own screen for `storage_not_private`:
 * `folder` there is the directory others can change, and `reason` says why. */
export const storeUpgrade = {
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
  cantOpen: {
    title: "Dash Wallet can't open this wallet data",
    text: "This version of Dash Wallet can't open the wallet data in {folder}. Nothing in it was changed, and the wallet data saved before the upgrade is still in the upgrade-backups folder inside {folder}. Your funds are held on the network, not in this data. Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",
    textWithoutBackup:
      "This version of Dash Wallet can't open the wallet data in {folder}. Nothing in it was changed. Your funds are held on the network, not in this data. Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",
    quit: "Quit",
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
      quit: "Quit",
    },
    noSpace: {
      title: "Not enough disk space to restore",
      text: "Dash Wallet needs more free disk space to restore your wallet data from before the upgrade, so nothing was changed. Free up space on the disk that holds {folder}, then choose to restore again.",
      ok: "OK",
      quit: "Quit",
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
