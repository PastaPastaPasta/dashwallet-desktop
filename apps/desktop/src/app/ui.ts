// UI state that is not server state: the route and what a link or menu put on it, the open
// detail sheet or dialog, discreet mode and the appearance. A tiny external store so the
// self-test and the shell bridge can drive it.

import { useEffect, useSyncExternalStore } from "react";
import type { Route } from "../api/generated/Route";
import type { ThemeName } from "../api/generated/ThemeName";
import type { ToolsTab } from "../api/generated/ToolsTab";

export type { Route, ToolsTab };
export type ThemeChoice = ThemeName | null;

/** Pay's two ways (DASHPAY §4): to a contact, or to an address. */
export type PayTab = "contact" | "address";

/** What a payment link or Open URI put in Pay ▸ To an address. */
export interface PayPrefill {
  address: string;
  /** Duffs. */
  amount: number | null;
  label: string | null;
  message: string | null;
  /** The DashPay username the link claims for the payee, unverified. */
  username: string | null;
}

/** What a `dashpay://user` link put in Contacts ▸ Add contact. */
export interface ContactPrefill {
  /** Base58 identity id. */
  identity: string;
  username: string;
}

/** `recoveryPhrase`: Show Recovery Phrase (U-02 D13); `unlock`: the full unlock from mixing only (D12). */
export type Dialog = "openUri" | "about" | "recoveryPhrase" | "unlock";

export interface UiState {
  route: Route;
  payTab: PayTab;
  payPrefill: PayPrefill | null;
  /** Counts the links that opened Pay: each one starts a fresh form, the same link again too. */
  payLinks: number;
  contactPrefill: ContactPrefill | null;
  toolsTab: ToolsTab;
  /** An invitation link arrived (DP5-01 adds its intake; Join DashPay says so meanwhile). */
  invitation: boolean;
  /** The wallet the window shows; `null` = the first. */
  walletId: string | null;
  /** Txid whose detail sheet is open. */
  detail: string | null;
  dialog: Dialog | null;
  /** Onboarding covers the shell: the first wallet, or Wallet ▸ Add Wallet… (U-02 D1). Only
   * whether it is open; its state stays in its view model. */
  onboarding: boolean;
  /** Forgot passphrase is open (U-02 D11). Not a dialog: it starts on the lock screen and
   * stays open after the recovery unlocks the vault, to say what happened. */
  forgotPassphrase: boolean;
  /** How many mounted views show a recovery phrase (`useSecretScreen`, U-02 D15). */
  secrets: number;
  discreet: boolean;
  /** A forced appearance; `null` follows the system. */
  theme: ThemeChoice;
  /** The system appearance (one `matchMedia` listener for the whole app, in theme.ts). */
  system: ThemeName;
}

let state: UiState = {
  route: "home",
  payTab: "contact",
  payPrefill: null,
  payLinks: 0,
  contactPrefill: null,
  toolsTab: "information",
  invitation: false,
  walletId: null,
  detail: null,
  dialog: null,
  onboarding: false,
  forgotPassphrase: false,
  secrets: 0,
  discreet: false,
  theme: null,
  system: "light",
};
const listeners = new Set<() => void>();

export const ui = {
  get: () => state,
  set(patch: Partial<UiState>) {
    state = { ...state, ...patch };
    for (const l of listeners) l();
  },
  subscribe(l: () => void) {
    listeners.add(l);
    return () => void listeners.delete(l);
  },
};

/** Opens `route`, closing the detail sheet. */
export function navigate(route: Route, patch: Partial<UiState> = {}) {
  ui.set({ route, detail: null, ...patch });
}

export function useUi<T>(select: (s: UiState) => T): T {
  return useSyncExternalStore(ui.subscribe, () => select(ui.get()));
}

/** Marks a recovery phrase as on screen while `shown`: the window is then kept out of screen
 * captures where the OS allows (U-02 D15). */
export function useSecretScreen(shown: boolean) {
  useEffect(() => {
    if (!shown) return;
    ui.set({ secrets: ui.get().secrets + 1 });
    return () => ui.set({ secrets: ui.get().secrets - 1 });
  }, [shown]);
}
