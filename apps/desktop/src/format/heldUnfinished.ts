// What Home says of the coins a dropped payment holds back (the engine's `heldUnfinished` and
// `heldUnvalued`): a line under the balance that never hides while any coin is held, and the way
// out it links to.

import type { Balances } from "../api/generated/Balances";
import { t } from "../i18n";

/** The held line, or "" when nothing is held. The amount sums only the coins whose value is known;
 * "At least" says some are missing from it, and with none known the coins are counted instead. */
export function heldLine(core: Balances, fmt: (duffs: number) => string): string {
  const { heldUnfinished: valued, heldUnvalued: unvalued } = core;
  if (valued > 0) return t(unvalued > 0 ? "home.core.heldUnfinishedAtLeast" : "home.core.heldUnfinished", { amount: fmt(valued) });
  if (unvalued > 0) return t("home.core.heldUnfinishedUnvalued", { count: unvalued });
  return "";
}

/** The engine's facts about a wallet that choose its way out. */
type WayOutWallet = { walletId: string; name: string; watchOnly: boolean; hasMnemonic: boolean; legacyIssued: boolean };

/** What to do for the wallet `walletId` among the open `wallets`, chosen from the engine's facts
 * about it: a signing wallet with no recovery phrase, or one with legacy address ranges a phrase
 * doesn't restore, is kept (no phrase wins when both apply); any other waits while a held coin
 * `awaiting` a transaction that isn't final (the engine's `heldAwaiting`), then a watch-only wallet
 * is added again from its key and any other restored from its phrase. The single-wallet texts only
 * when exactly one is open (an empty list is a failed read, not a lone wallet). `null` when
 * `walletId` isn't listed. */
export function heldWayOutText(walletId: string, wallets: readonly WayOutWallet[], awaiting: boolean): string | null {
  const own = wallets.find((w) => w.walletId === walletId);
  if (!own) return null;
  const place = {
    wallet: own.name,
    settings: t("routes.settings"),
    walletTab: t("settings.wallets.title"),
    addWallet: t("settings.wallets.addWallet"),
    addWatchOnly: t("settings.wallets.addWatchOnly"),
    optionsTitle: t("onboarding.options.title"),
    scanFrom: t("onboarding.options.scanFrom"),
    coreCompat: t("onboarding.options.coreCompat"),
    welcomeTitle: t("onboarding.welcome.title"),
    welcomeRestore: t("onboarding.welcome.restore"),
    lockUnspent: t("tools.coins.lockUnspent"),
    tools: t("routes.tools"),
    coinSelection: t("tools.tabs.coinSelection"),
  };
  const single = wallets.length === 1;
  if (!own.watchOnly && !own.hasMnemonic) return t("home.heldUnfinished.keepNoPhrase", place);
  if (!own.watchOnly && own.legacyIssued) return t("home.heldUnfinished.keepLegacy", place);
  if (awaiting) return t("home.heldUnfinished.waiting", place);
  if (own.watchOnly) return t(single ? "home.heldUnfinished.wayOutWatchOnlySingle" : "home.heldUnfinished.wayOutWatchOnly", place);
  return t(single ? "home.heldUnfinished.wayOutSingle" : "home.heldUnfinished.wayOut", place);
}
