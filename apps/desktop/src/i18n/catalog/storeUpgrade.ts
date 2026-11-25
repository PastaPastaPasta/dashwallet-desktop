import type { Tree } from "../core";

/** The screens the engine gives for a refused open, by their keys; each value is the engine's English
 * (dw-store-upgrade's copy). The screen shows the engine's own text for a key missing here. `folder` is
 * the network's data folder. */
export const storeUpgrade = {
  cantOpen: {
    title: "Dash Wallet can't open this wallet data",
    text: "This version of Dash Wallet can't open the wallet data in {folder}. Nothing in it was changed, and the wallet data saved before the upgrade is still in the upgrade-backups folder inside {folder}. Your funds are held on the network, not in this data. Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",
    textWithoutBackup:
      "This version of Dash Wallet can't open the wallet data in {folder}. Nothing in it was changed. Your funds are held on the network, not in this data. Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",
    quit: "Quit",
  },
} as const satisfies Tree;
