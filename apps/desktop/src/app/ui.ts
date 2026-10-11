// UI state that is not server state: the page, the open detail sheet, discreet mode and
// the appearance. A tiny external store so the self-test can drive it.

import { useSyncExternalStore } from "react";
import type { ThemeName } from "../api/generated/ThemeName";

export type Page = "overview" | "transactions";
export type ThemeChoice = ThemeName | null;

export interface UiState {
  page: Page;
  /** Txid whose detail sheet is open. */
  detail: string | null;
  discreet: boolean;
  /** A forced appearance; `null` follows the system. */
  theme: ThemeChoice;
  /** The system appearance (one `matchMedia` listener for the whole app, in theme.ts). */
  system: ThemeName;
}

let state: UiState = { page: "overview", detail: null, discreet: false, theme: null, system: "light" };
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

export function useUi<T>(select: (s: UiState) => T): T {
  return useSyncExternalStore(ui.subscribe, () => select(ui.get()));
}
