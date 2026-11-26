// RESTORED as the money gate reads it: not known while Finish without Dash Platform runs, as the
// network closes and opens again under it.

import { useSyncExternalStore } from "react";
import { useRestored } from "./queries";

/** Finish without Dash Platform is in flight: the network closes and opens again under it, so what
 * `restored` reads meanwhile says nothing. */
let finishing = false;
const finishingListeners = new Set<() => void>();

export function setRestoreFinishing(on: boolean) {
  finishing = on;
  for (const l of finishingListeners) l();
}

const subscribeFinishing = (l: () => void) => {
  finishingListeners.add(l);
  return () => void finishingListeners.delete(l);
};
const isFinishing = () => finishing;

/** RESTORED as the money gate reads it: unknown (`undefined`) until read, and while Finish without
 * Dash Platform is in flight, so a closed session never reads as clear. */
export function useRestoredGate(): boolean | undefined {
  const restored = useRestored().data;
  return useSyncExternalStore(subscribeFinishing, isFinishing, isFinishing) ? undefined : restored;
}
