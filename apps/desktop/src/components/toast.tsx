// Toast (UX-SPEC C21): bottom-centre, 24 pt above the status bar, auto-dismiss. An actionable
// failure is an alert in the same stack: it stays until Dismiss or the next attempt's outcome, and
// a toast shows under it rather than over it.

import { useSyncExternalStore } from "react";
import { useUi } from "../app/ui";
import { t, useT } from "../i18n";
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

/** A failure the user may act on; `key` names the attempt a retry repeats. */
interface Alert {
  id: number;
  key: string;
  text: string;
}

let current: Toast | null = null;
let alert: Alert | null = null;
const listeners = new Set<() => void>();
let seq = 0;
let held = false;

const notify = () => listeners.forEach((l) => l());

function show(toast: Omit<Toast, "id">, seconds: number) {
  if (held && !toast.shell) return;
  const id = ++seq;
  current = { id, ...toast };
  notify();
  window.setTimeout(() => {
    if (current?.id === id) {
      current = null;
      notify();
    }
  }, seconds * 1000);
}

/** Shows `text` as the alert until Dismiss or `clearAlert(key)`; it replaces the alert up. */
export function showAlert(text: string, key: string) {
  if (held) return;
  alert = { id: ++seq, key, text };
  notify();
}

/** Drops the alert when it is `key`'s: the attempt is being retried, or a retry worked. */
export function clearAlert(key: string) {
  if (alert?.key !== key) return;
  alert = null;
  notify();
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
  if (!hold) return;
  const dropToast = current !== null && !current.shell;
  if (!dropToast && !alert) return;
  if (dropToast) current = null;
  alert = null;
  notify();
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

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

export function ToastHost() {
  const toast = useSyncExternalStore(subscribe, () => current);
  const shown = useSyncExternalStore(subscribe, () => alert);
  const discreet = useUi((u) => u.discreet);
  const tr = useT();
  return (
    <div className="toast-stack">
      <div className="toast-alert-host" role="alert">
        {shown && (
          <div className="toast-alert" key={shown.id} data-testid="toast-alert">
            <span className="toast-alert-text" data-testid="toast-alert-text">
              {shown.text}
            </span>
            <button type="button" className="toast-alert-dismiss" data-testid="toast-alert-dismiss" onClick={() => clearAlert(shown.key)}>
              {tr("components.toast.dismiss")}
            </button>
          </div>
        )}
      </div>
      <div className="toast-host" role="status" aria-live="polite">
        {toast && (
          <div className="toast" key={toast.id}>
            {discreet && toast.discreetText !== undefined ? toast.discreetText : toast.text}
          </div>
        )}
      </div>
    </div>
  );
}
