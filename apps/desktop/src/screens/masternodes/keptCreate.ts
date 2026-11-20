// A proposal's fee payment that outlived its sheet: a lock takes the page down at once, and
// `create_proposal` may already have paid the fee. The page reads it back after the unlock, so
// the user is sent to Mine rather than invited to pay again.

import { useSyncExternalStore } from "react";
import type { MyProposal } from "../../api/generated/MyProposal";

export type KeptCreate = { kind: "paying" } | { kind: "created"; p: MyProposal } | { kind: "unsure" };

const kept = new Map<string, KeptCreate>();
const listeners = new Set<() => void>();
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

export function keepCreate(walletId: string, k: KeptCreate | null) {
  if (k) kept.set(walletId, k);
  else kept.delete(walletId);
  listeners.forEach((l) => l());
}

export const keptCreate = (walletId: string): KeptCreate | null => kept.get(walletId) ?? null;

export const useKeptCreate = (walletId: string): KeptCreate | null => useSyncExternalStore(subscribe, () => keptCreate(walletId));
