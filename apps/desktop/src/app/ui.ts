// UI state that is not server state: the route and what a link or menu put on it, the open
// detail sheet or dialog, discreet mode and the appearance. A tiny external store so the
// self-test and the shell bridge can drive it.

import { useEffect, useSyncExternalStore } from "react";
import type { Route } from "../api/generated/Route";
import type { ThemeName } from "../api/generated/ThemeName";
import type { ToolsTab } from "../api/generated/ToolsTab";
import type { UiSettings } from "../api/generated/UiSettings";

export type { Route, ToolsTab };
export type ThemeChoice = ThemeName | null;

/** Settings' tabs (U-04 D1). */
export type SettingsTab = "general" | "security" | "wallets" | "backups" | "about";

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

/** The window's content protection while a secret surface is open (U-02 D27): `pending` until
 * the shell answered that it is in effect, `failed` when the platform could not apply it. */
export type Protection = "off" | "pending" | "on" | "failed";

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
  settingsTab: SettingsTab;
  /** An invitation link arrived (DP5-01 adds its intake; Join DashPay says so meanwhile). */
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
  /** How many mounted views show a recovery phrase (`useSecretScreen`, U-02 D15). */
  secrets: number;
  /** Whether the content protection those views need is in effect (D27; set by the shell). */
  protection: Protection;
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
  toolsTab: "information",
  settingsTab: "general",
  invitation: false,
  walletId: null,
  detail: null,
  dialog: null,
  recoveryWallet: null,
  onboarding: false,
  forgotPassphrase: false,
  secrets: 0,
  protection: "off",
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

/** Opens `route`, closing the detail sheet. */
export function navigate(route: Route, patch: Partial<UiState> = {}) {
  ui.set({ route, detail: null, ...patch });
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
