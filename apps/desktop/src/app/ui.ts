// UI state that is not server state: the route and what a link or menu put on it, the open
// detail sheet or dialog, discreet mode and the appearance. A tiny external store so the
// self-test and the shell bridge can drive it.

import { useSyncExternalStore } from "react";
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

export type Dialog = "openUri" | "about";

export interface UiState {
  route: Route;
  payTab: PayTab;
  payPrefill: PayPrefill | null;
  contactPrefill: ContactPrefill | null;
  toolsTab: ToolsTab;
  /** An invitation link arrived (DP5-01 takes it over; Join DashPay says so meanwhile). */
  invitation: boolean;
  /** The wallet the window shows; `null` = the first. */
  walletId: string | null;
  /** Txid whose detail sheet is open. */
  detail: string | null;
  dialog: Dialog | null;
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
  contactPrefill: null,
  toolsTab: "information",
  invitation: false,
  walletId: null,
  detail: null,
  dialog: null,
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
