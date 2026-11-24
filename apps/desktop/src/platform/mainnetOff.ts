// The Platform addresses' own funds paths this version keeps off on mainnet (the engine's flag
// `platform_address_sends`, `platform.feature_off` with that feature): adding to Platform, a send or
// move from the addresses, the Platform receive address. What finishes, exits or retries a move made
// earlier stays offered. Plain TypeScript over the generated types.

import { isAppError } from "../api/backend";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { PlatformAddressBalance } from "../api/generated/PlatformAddressBalance";
import { t } from "../i18n";

/** The engine's feature name for the flag (`platform.feature_off{feature}`, `sendsOff`). */
export const PLATFORM_ADDRESS_SENDS = "platform_address_sends";

/** Whether the flag holds the paths off: the balance's `sendsOff` once it is read, the network until then. */
export function addressSendsOff(network: Pick<NetworkInfo, "kind">, balance: PlatformAddressBalance | null | undefined): boolean {
  if (balance) return balance.sendsOff === PLATFORM_ADDRESS_SENDS;
  return network.kind === "mainnet";
}

/** The engine refused a call by the flag. */
export function isAddressSendsOff(e: unknown): boolean {
  return isAppError(e) && e.code === "platform.feature_off" && e.data?.kind === "feature" && e.data.feature === PLATFORM_ADDRESS_SENDS;
}

export const MAINNET_OFF_COPY = {
  /** Platform → Dash wallet and a payment from Platform, in place of their controls. */
  get moveAndSend() {
    return t("transfer.platform.mainnetOff");
  },
  /** `moveAndSend` while the addresses hold `amount`. */
  held(amount: string) {
    return t("transfer.platform.mainnetHeld", { amount });
  },
  /** While a move made earlier is held, in place of `moveAndSend` and `held`. */
  heldExit(moveAmount: string) {
    return t("transfer.platform.mainnetHeldExit", { moveAmount });
  },
  /** After `heldExit` while the balance outside the held move is above 0. */
  heldExitRest(rest: string) {
    return t("transfer.platform.mainnetHeldExitRest", { rest });
  },
  /** Dash wallet → Platform, in place of its form. */
  get add() {
    return t("transfer.platform.mainnetOffAdd");
  },
  /** Beside the Platform pool where it can't be reached. */
  get chip() {
    return t("transfer.platform.mainnetOffChip");
  },
  /** Receive ▸ Platform, in place of the address, its QR code and Copy. */
  get receive() {
    return t("receive.platform.mainnetOff");
  },
  /** The engine's refusal, should a screen still reach it. */
  get refused() {
    return t("platform.mainnetSendOff");
  },
};
