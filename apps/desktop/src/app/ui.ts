// UI state that is not server state: the page, the open detail sheet, discreet mode and
// a forced appearance. A tiny external store so the self-test can drive it.

import { useSyncExternalStore } from "react";

export type Page = "overview" | "transactions";
export type ThemeChoice = "light" | "dark" | null;

export interface UiState {
  page: Page;
  /** Txid whose detail sheet is open. */
  detail: string | null;
  discreet: boolean;
  /** `null` follows the system appearance. */
  theme: ThemeChoice;
}

let state: UiState = { page: "overview", detail: null, discreet: false, theme: null };
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
