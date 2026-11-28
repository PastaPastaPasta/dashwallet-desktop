// Toast (UX-SPEC C21): bottom-centre, 24 pt above the status bar, auto-dismiss. An actionable
// failure is an alert in the same stack: it stays until Dismiss or the next attempt's outcome, and
// a toast shows under it rather than over it. An alert's region comes with it: role=alert is said
// when it is inserted, unlike the status region a toast fills (issue 291). A sheet opened after the
// alert hides it until the sheet closes, so it never covers that sheet's buttons; one raised while a
// sheet is open (that sheet's own refusal) shows over it.

import { useEffect, useRef, useSyncExternalStore, type KeyboardEvent } from "react";
import { useUi } from "../app/ui";
import { lastSheetOpened, newestOpenSheet, watchSheets } from "./sheetOrder";
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
  /** What had focus when it showed: Dismiss gives focus back there. */
  back: Element | null;
  /** `lastSheetOpened()` when it showed: a newer sheet open hides it. */
  sheet: number;
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
  globalThis.setTimeout(() => {
    if (current?.id === id) {
      current = null;
      notify();
    }
  }, seconds * 1000);
}

/** Shows `text` as the alert until Dismiss or `clearAlert(key)`; it replaces the alert up. */
export function showAlert(text: string, key: string) {
  if (held) return;
  alert = { id: ++seq, key, text, back: document.activeElement, sheet: lastSheetOpened() };
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

/** Another wallet's alert would read as this one's: a switch drops it. */
function dropAlertOnSwitch(walletId: string | null) {
  const seen = useRef(walletId);
  useEffect(() => {
    if (seen.current === walletId) return;
    seen.current = walletId;
    if (!alert) return;
    alert = null;
    notify();
  }, [walletId]);
}

export function ToastHost() {
  const toast = useSyncExternalStore(subscribe, () => current);
  const shown = useSyncExternalStore(subscribe, () => alert);
  const underSheet = useSyncExternalStore(watchSheets, newestOpenSheet) > (shown?.sheet ?? Infinity);
  const discreet = useUi((u) => u.discreet);
  dropAlertOnSwitch(useUi((u) => u.walletId));
  const tr = useT();
  const host = useRef<HTMLDivElement>(null);
  const dismiss = (a: Alert) => {
    const hadFocus = host.current?.contains(document.activeElement) ?? false;
    clearAlert(a.key);
    if (!hadFocus) return;
    const back = a.back instanceof HTMLElement && a.back.isConnected && a.back !== document.body ? a.back : document.querySelector<HTMLElement>(".app-body > main h1");
    if (back && !back.matches("button, a[href], input, select, textarea, [tabindex]")) back.tabIndex = -1;
    back?.focus({ preventScroll: true });
  };
  const onKey = (e: KeyboardEvent, a: Alert) => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    dismiss(a);
  };
  return (
    <div className={shown ? "toast-stack has-alert" : "toast-stack"}>
      {shown && (
        <div className={underSheet ? "toast-alert is-under-sheet" : "toast-alert"} role="alert" key={shown.id} ref={host} data-testid="toast-alert" onKeyDown={(e) => onKey(e, shown)}>
          <span className="toast-alert-text" data-testid="toast-alert-text">
            {shown.text}
          </span>
          <button type="button" className="toast-alert-dismiss" data-testid="toast-alert-dismiss" onClick={() => dismiss(shown)}>
            {tr("components.toast.dismiss")}
          </button>
        </div>
      )}
      <div className="toast-host" role="status" aria-live="polite">
        {toast && (
          <div className="toast" data-testid="toast" key={toast.id}>
            {discreet && toast.discreetText !== undefined ? toast.discreetText : toast.text}
          </div>
        )}
      </div>
    </div>
  );
}
