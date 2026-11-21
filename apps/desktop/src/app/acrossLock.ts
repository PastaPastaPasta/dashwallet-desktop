// What a page's operation keeps across a lock. A lock takes the wallet's pages down at once
// (lockGate.ts), but an operation the engine already admitted (a file being written, a payment
// being broadcast) still finishes, and its result is one the user needs. A page's model that is in
// such an operation, or showing its result, when the page goes with a lock is kept here, out of
// view while locked, and the page takes it back when it mounts after the unlock.

import { useEffect, useState } from "react";
import { walletGateClosed } from "../api/lockGate";

const slots = new Map<string, unknown>();

/** The model `slot` kept across a lock, if any. */
export const keptAcrossLock = <T,>(slot: string): T | undefined => slots.get(slot) as T | undefined;

/** The page's model under `slot`: the one kept across a lock, else `make()`. When the page goes
 * while locked and `holds(model)` (an admitted operation, or its result on show) it is kept; any
 * other unmount ends it with `end`. */
export function useAcrossLock<M>(slot: string, make: () => M, holds: (m: M) => boolean, end?: (m: M) => void): M {
  const [model] = useState(() => keptAcrossLock<M>(slot) ?? make());
  useEffect(() => {
    if (slots.get(slot) === model) slots.delete(slot);
    return () => {
      if (walletGateClosed() && holds(model)) slots.set(slot, model);
      else end?.(model);
    };
  }, [slot, model]); // eslint-disable-line react-hooks/exhaustive-deps
  return model;
}
