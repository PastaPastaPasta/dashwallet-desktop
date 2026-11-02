// Toast (UX-SPEC C21): bottom-centre, 24 pt above the status bar, auto-dismiss.

import { useSyncExternalStore } from "react";
import { useUi } from "../app/ui";
import { motion } from "../styles/tokens.generated";

/** `text` is what the toast says; a toast with `discreetText` names an amount, and says that
 * instead whenever Discreet mode is on, the toast already on screen too. */
interface Toast {
  id: number;
  text: string;
  discreetText?: string;
}

let current: Toast | null = null;
const listeners = new Set<() => void>();
let seq = 0;

export function showToast(text: string, seconds: number = motion.toastDuration, discreetText?: string) {
  const id = ++seq;
  current = { id, text, discreetText };
  listeners.forEach((l) => l());
  window.setTimeout(() => {
    if (current?.id === id) {
      current = null;
      listeners.forEach((l) => l());
    }
  }, seconds * 1000);
}

/** Takes down a toast that names an amount (the wallet locked: what it received is for who unlocks it). */
export function dismissPrivateToast() {
  if (!current?.discreetText) return;
  current = null;
  listeners.forEach((l) => l());
}

/** iOS "Copied" toast after every copy (UX-SPEC §3.2). */
export async function copyWithToast(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    showToast("Copied", motion.toastCopiedDuration);
    return true;
  } catch {
    showToast("Could not copy");
    return false;
  }
}

export function ToastHost() {
  const toast = useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => void listeners.delete(l);
    },
    () => current,
  );
  const discreet = useUi((u) => u.discreet);
  return (
    <div className="toast-host" role="status" aria-live="polite">
      {toast && (
        <div className="toast" key={toast.id}>
          {discreet && toast.discreetText !== undefined ? toast.discreetText : toast.text}
        </div>
      )}
    </div>
  );
}
