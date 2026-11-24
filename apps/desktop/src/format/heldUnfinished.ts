// What Home says of the coins a dropped payment holds back (the engine's `heldUnfinished` and
// `heldUnvalued`): a line under the balance that never hides while any coin is held, and the way
// out it links to.

import type { Balances } from "../api/generated/Balances";
import { t } from "../i18n";

/** The held line, or "" when nothing is held. The amount sums only the coins whose value is known;
 * "At least" says some are missing from it, and with none known the coins are counted instead. */
export function heldLine(core: Balances, fmt: (duffs: number) => string): string {
  const { heldUnfinished: valued, heldUnvalued: unvalued } = core;
  if (unvalued === 0) return valued > 0 ? t("home.core.heldUnfinished", { amount: fmt(valued) }) : "";
  if (valued > 0) return t("home.core.heldUnfinishedAtLeast", { amount: fmt(valued) });
  return t("home.core.heldUnfinishedUnvalued", { count: unvalued });
}

/** The way out for the wallet `walletId` among the open `wallets`: a watch-only wallet is added
 * again from its key, any other restored from its phrase; the single-wallet text only when exactly
 * one is open (an empty list is a failed read, not a lone wallet). */
export function heldWayOutText(walletId: string, wallets: readonly { walletId: string; name: string; watchOnly: boolean }[]): string {
  const own = wallets.find((w) => w.walletId === walletId);
  const place = {
    wallet: own?.name ?? walletId,
    settings: t("routes.settings"),
    walletTab: t("settings.wallets.title"),
    addWallet: t("settings.wallets.addWallet"),
    addWatchOnly: t("settings.wallets.addWatchOnly"),
    optionsTitle: t("onboarding.options.title"),
    scanFrom: t("onboarding.options.scanFrom"),
  };
  const single = wallets.length === 1;
  if (own?.watchOnly) return t(single ? "home.heldUnfinished.wayOutWatchOnlySingle" : "home.heldUnfinished.wayOutWatchOnly", place);
  return t(single ? "home.heldUnfinished.wayOutSingle" : "home.heldUnfinished.wayOut", place);
}
