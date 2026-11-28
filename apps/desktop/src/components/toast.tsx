// Toast (UX-SPEC C21): bottom-centre, 24 pt above the status bar, auto-dismiss.

import { useSyncExternalStore } from "react";
import { useUi } from "../app/ui";
import { t } from "../i18n";
import { motion } from "../styles/tokens.generated";

/** `text` is what the toast says; a toast with `discreetText` names an amount, and says that
 * instead whenever Discreet mode is on, the toast already on screen too. */
interface Toast {
  id: number;
  text: string;
  discreetText?: string;
  /** The shell's own (the lock, links, a network switch): it shows while the wallet is locked. */
  shell?: boolean;
}

let current: Toast | null = null;
const listeners = new Set<() => void>();
let seq = 0;
let held = false;

function show(toast: Omit<Toast, "id">, seconds: number) {
  if (held && !toast.shell) return;
  const id = ++seq;
  current = { id, ...toast };
  listeners.forEach((l) => l());
  globalThis.setTimeout(() => {
    if (current?.id === id) {
      current = null;
      listeners.forEach((l) => l());
    }
  }, seconds * 1000);
}

export function showToast(text: string, seconds: number = motion.toastDuration, discreetText?: string) {
  show({ text, discreetText }, seconds);
}

/** A toast that names nothing of the wallet, so it shows while the wallet is locked too. */
export function showShellToast(text: string, seconds: number = motion.toastDuration) {
  show({ text, shell: true }, seconds);
}

/** While the wallet is locked (App), a page's or a flow's toast may name an amount, a wallet or a
 * contact: the one up goes, and none shows until the unlock. */
export function holdWalletToasts(hold: boolean) {
  held = hold;
  if (!hold || !current || current.shell) return;
  current = null;
  listeners.forEach((l) => l());
}

/** iOS "Copied" toast after every copy (UX-SPEC §3.2). */
export async function copyWithToast(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    showToast(t("components.copy.copied"), motion.toastCopiedDuration);
    return true;
  } catch {
    showToast(t("components.copy.failed"));
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
        <div className="toast" data-testid="toast" key={toast.id}>
          {discreet && toast.discreetText !== undefined ? toast.discreetText : toast.text}
        </div>
      )}
    </div>
  );
}
