// Receive shows a payment before the next address (DEC-1058 #6). The engine moves its current
// receiving address on as soon as one is used, so the page would swap the QR before anyone saw that
// the payment came. The address stays for `RECEIVED_HOLD_MS` while the page says what was received;
// an address that moves on with no payment named waits `ROTATION_GRACE_MS` for the notice, which
// can arrive a moment after the new address does.

import { useEffect, useState } from "react";
import type { AddressInfo } from "../api/generated/AddressInfo";
import type { ReceivedPayment } from "./received";

export const RECEIVED_HOLD_MS = 4000;
export const ROTATION_GRACE_MS = 1500;

/** How long the address on show stays once `target` is the engine's: not at all for an address the
 * user asked for, `RECEIVED_HOLD_MS` once a payment to the one shown is known, until the answer while a
 * notice lookup is outstanding (`null`), else the grace. */
export function holdFor(asked: boolean, paid: boolean, lookingUp = false): number | null {
  if (asked) return 0;
  if (paid) return RECEIVED_HOLD_MS;
  // A lookup of what the engine just reported is outstanding: the address stays until it answers (or
  // the lookup's own timeout), not for a fixed time that a slow answer outlasts.
  return lookingUp ? null : ROTATION_GRACE_MS;
}

/** The address to show and the payments that paid it. `current` is the engine's receiving address,
 * counted once `settled` (read, not a cached one being read again: a used address is never shown
 * first); `asked` the one "New address" issued, shown at once and until a payment uses it. */
export function useHeldAddress(
  current: AddressInfo | undefined,
  settled: boolean,
  asked: AddressInfo | null,
  payments: readonly ReceivedPayment[],
  lookingUp = false,
): { shown: AddressInfo | undefined; paid: ReceivedPayment[] } {
  const keep = asked !== null && !payments.some((p) => p.address === asked.address) ? asked : null;
  const target = keep ?? (settled ? current : undefined);
  const [held, setHeld] = useState<AddressInfo | undefined>(undefined);
  const shown = held ?? target;
  const paid = shown ? payments.filter((p) => p.address === shown.address) : [];
  const delay = holdFor(keep !== null, paid.length > 0, lookingUp);
  useEffect(() => {
    // The first address read is the one on show; only later ones are held back.
    if (held === undefined && target) setHeld(target);
    if (!target || target.address === shown?.address || delay === null) return;
    const timer = window.setTimeout(() => setHeld(target), delay);
    return () => window.clearTimeout(timer);
  }, [target, held, shown?.address, delay]);
  return { shown, paid };
}
