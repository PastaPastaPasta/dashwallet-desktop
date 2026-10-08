// Toast (UX-SPEC C21): bottom-centre, 24 pt above the status bar, auto-dismiss.

import { useSyncExternalStore } from "react";
import { motion } from "../styles/tokens.generated";

let current: { id: number; text: string } | null = null;
const listeners = new Set<() => void>();
let seq = 0;

export function showToast(text: string, seconds: number = motion.toastDuration) {
  const id = ++seq;
  current = { id, text };
  listeners.forEach((l) => l());
  window.setTimeout(() => {
    if (current?.id === id) {
      current = null;
      listeners.forEach((l) => l());
    }
  }, seconds * 1000);
}

/** iOS "Copied" toast after every copy (UX-SPEC §3.2). */
export async function copyWithToast(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    showToast("Copied", motion.toastCopiedDuration);
  } catch {
    showToast("Could not copy");
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
  return (
    <div className="toast-host" role="status" aria-live="polite">
      {toast && (
        <div className="toast" key={toast.id}>
          {toast.text}
        </div>
      )}
    </div>
  );
}
