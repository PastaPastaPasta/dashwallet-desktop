// UI state that is not server state: the route and what a link or menu put on it, the open
// detail sheet or dialog, discreet mode and the appearance. A tiny external store so the
// self-test and the shell bridge can drive it.

import { useEffect, useSyncExternalStore } from "react";
import type { ContactSortName } from "../api/generated/ContactSortName";
import type { Route } from "../api/generated/Route";
import type { ThemeName } from "../api/generated/ThemeName";
import type { ToolsTab } from "../api/generated/ToolsTab";
import type { UiSettings } from "../api/generated/UiSettings";

export type { Route, ToolsTab };
export type ThemeChoice = ThemeName | null;

/** Settings' tabs (U-04 D1). */
export type SettingsTab = "general" | "security" | "wallets" | "coinjoin" | "backups" | "about";

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

/** What opened Contacts' Add contact slot (DP2-07 D12): the wallet it was opened in, the search
 * text, and the network hit picked, if any. */
export interface AddContactIntent {
  walletId: string;
  query: string;
  hit: { identity: string; username: string } | null;
}

/** The window's content protection while a secret surface is open (U-02 D27): `pending` until
 * the shell answered that it is in effect, `failed` when the platform could not apply it. */
export type Protection = "off" | "pending" | "on" | "failed";

/** `recoveryPhrase`: Show Recovery Phrase (U-02 D13); `unlock`: the full unlock from mixing only (D12). */
export type Dialog = "openUri" | "about" | "recoveryPhrase" | "unlock" | "savedLinks";

export interface UiState {
  route: Route;
  payTab: PayTab;
  payPrefill: PayPrefill | null;
  /** Counts the links that opened Pay: each one starts a fresh form, the same link again too. */
  payLinks: number;
  contactPrefill: ContactPrefill | null;
  /** Coin Selection's "Pay with these coins" (U-05 D19): the `txid:vout` keys Pay ▸ To an
   * address starts with, under Advanced on "Selected coins". */
  payCoins: string[] | null;
  /** Pay ▸ To a contact's payee (DP3-06 D2): the contact's identity id, picked on the tab or
   * handed over by the contact pane, in the wallet it was picked in (another wallet's tab shows
   * none); `null`: none picked. */
  payContact: { walletId: string; contact: string } | null;
  toolsTab: ToolsTab;
  /** Pay's Create Unsigned (U-05 D18): the PSBT Tools ▸ PSBT opens with, and whether it reached
   * the clipboard. The tab takes it once. */
  psbtPrefill: { psbt: string; copied: boolean } | null;
  settingsTab: SettingsTab;
  /** An invitation link was opened: it waits in the link stash (DP5-01); Join DashPay says so
   * until DP5-03 joins with it. */
  invitation: boolean;
  /** The wallet the window shows; `null` = the first. */
  walletId: string | null;
  /** Txid whose detail sheet is open. */
  detail: string | null;
  dialog: Dialog | null;
  /** The wallet Show Recovery Phrase opens on when Settings ▸ Wallets picked one; `null`: the
   * window's wallet. */
  recoveryWallet: string | null;
  /** Onboarding covers the shell: the first wallet, or Wallet ▸ Add Wallet… (U-02 D1). Only
   * whether it is open; its state stays in its view model. */
  onboarding: boolean;
  /** Forgot passphrase is open (U-02 D11). Not a dialog: it starts on the lock screen and
   * stays open after the recovery unlocks the vault, to say what happened. */
  forgotPassphrase: boolean;
  /** Settings ▸ Wipe is open: onboarding stays closed while the wipe runs and while its sheet
   * reports a stop, so a vault that could not be deleted is said, not covered (U-04 D33). */
  wiping: boolean;
  /** How many mounted views show a recovery phrase (`useSecretScreen`, U-02 D15). */
  secrets: number;
  /** Whether the content protection those views need is in effect (D27; set by the shell). */
  protection: Protection;
  /** Contacts (DP2-07): the selected contact (its identity id), whatever opened it (DP2-07 D11). */
  contact: string | null;
  /** Contacts' Add contact slot: what a search or the Add button carries to it (D12). */
  addContact: AddContactIntent | null;
  /** Contacts' order of My Contacts and Hidden, for the session (D3). */
  contactSort: ContactSortName;
  /** Bumped by Try again after `failed`: the shell asks for the protection again. */
  protectionTries: number;
  discreet: boolean;
  /** A forced appearance; `null` follows the system. `--theme`, else the saved one (U-04 D3). */
  theme: ThemeChoice;
  /** The saved display settings (settings/prefs.ts, U-04 D2). */
  prefs: UiSettings;
  /** The system appearance (one `matchMedia` listener for the whole app, in theme.ts). */
  system: ThemeName;
}

/** dw-app's `UiSettings::default()`: what a missing settings file reads as. */
export const DEFAULT_PREFS: UiSettings = {
  theme: null,
  unit: "dash",
  decimalDigits: 2,
  autohideBalance: false,
  autoLockSecs: null,
  subtractFeeByDefault: false,
};

let state: UiState = {
  route: "home",
  payTab: "contact",
  payPrefill: null,
  payLinks: 0,
  contactPrefill: null,
  payCoins: null,
  payContact: null,
  toolsTab: "information",
  psbtPrefill: null,
  settingsTab: "general",
  invitation: false,
  walletId: null,
  detail: null,
  dialog: null,
  recoveryWallet: null,
  onboarding: false,
  forgotPassphrase: false,
  wiping: false,
  secrets: 0,
  protection: "off",
  contact: null,
  addContact: null,
  contactSort: "displayName",
  protectionTries: 0,
  discreet: false,
  theme: null,
  prefs: DEFAULT_PREFS,
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

/** A view holding unsaved work (DP4-03 D11): asked before `navigate` leaves for `route`; `true`
 * when it holds the navigation, which it runs with `go` once the user agrees. */
export type LeaveGuard = (route: Route, go: () => void) => boolean;
let leaveGuard: LeaveGuard | null = null;

/** Installs `guard` until the returned function removes it. */
export function setLeaveGuard(guard: LeaveGuard): () => void {
  leaveGuard = guard;
  return () => {
    if (leaveGuard === guard) leaveGuard = null;
  };
}

/** Opens `route`, closing the detail sheet. */
export function navigate(route: Route, patch: Partial<UiState> = {}) {
  const go = () => ui.set({ route, detail: null, ...patch });
  if (!leaveGuard?.(route, go)) go();
}

/** Opens a fresh Pay ▸ To an address form (each call starts a new one), with what a link or a
 * tool put in it. */
export function openPayForm(patch: Pick<Partial<UiState>, "payPrefill" | "payCoins"> = {}) {
  navigate("pay", { payTab: "address", payPrefill: null, payCoins: null, payLinks: ui.get().payLinks + 1, ...patch });
}

export function useUi<T>(select: (s: UiState) => T): T {
  return useSyncExternalStore(ui.subscribe, () => select(ui.get()));
}

/** Marks a view that shows or takes a recovery phrase as open: the window is then kept out of
 * screen captures where the OS allows (U-02 D15). Returns whether that is in effect: the view
 * renders its secret part only once it is `on` (D27). */
export function useSecretScreen(): Protection {
  useEffect(() => {
    ui.set({ secrets: ui.get().secrets + 1 });
    return () => ui.set({ secrets: ui.get().secrets - 1 });
  }, []);
  // Before the shell has heard of this view, whatever the store says is about the last one.
  return useUi((s) => (s.secrets > 0 ? s.protection : "pending"));
}

/** Asks the shell for the content protection again after it failed. */
export function retryProtection() {
  ui.set({ protectionTries: ui.get().protectionTries + 1 });
}
