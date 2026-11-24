// What Abandon and Remove Unconfirmed say when a saved transaction of an open wallet doesn't read
// (the engine's `recordUnreadable`): the wallet by name, and where to close or remove it.

import { isAppError } from "../api/backend";
import { t } from "../i18n";

/** The wallet `recordUnreadable` names, when `e` is that refusal; `undefined` otherwise. */
export function unreadableWallet(e: unknown): string | null | undefined {
  if (!isAppError(e) || e.data?.kind !== "txAction" || e.data.refusal !== "recordUnreadable") return undefined;
  return e.data.walletId;
}

/** The refusal's text for the wallet `walletId` among the open `wallets`: the single-wallet text
 * only when exactly one is open (an empty list is a failed read, not a lone wallet). */
export function recordUnreadableText(walletId: string | null, wallets: readonly { walletId: string; name: string }[]): string {
  const wallet = wallets.find((w) => w.walletId === walletId)?.name ?? walletId ?? "";
  const place = { wallet, settings: t("routes.settings"), walletTab: t("settings.wallets.title"), addWallet: t("settings.wallets.addWallet") };
  return t(wallets.length === 1 ? "activity.abandon.recordUnreadableOnly" : "activity.abandon.recordUnreadable", place);
}
