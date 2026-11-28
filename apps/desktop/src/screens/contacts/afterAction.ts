// What a surface does once one of its contact actions ended (DP2-09 D23, D27, D28): say the
// outcome where the user is, and give keyboard focus a place when the action's control left
// with its row.

import { useCallback, useEffect, useRef, useState } from "react";
import { useWallets } from "../../api/queries";
import { ui } from "../../app/ui";
import { clearAlert, showAlert, showToast } from "../../components/toast";
import type { ActionOutcome } from "../../contacts/actions";
import { t } from "../../i18n";

/** Says an outcome in a toast when the view it ran in cannot (D23): that view is gone (closed,
 * left) or shows no outcome of its own (`shownHere` false). A refusal is an alert that stays until
 * Dismiss or the same action's next outcome. An outcome that ends after the window
 * moved to another wallet always toasts, under the name of the wallet it ran in (D27). */
export function useActionToast(walletId: string): (outcome: ActionOutcome | null, shownHere: boolean) => void {
  const wallets = useWallets();
  const list = useRef(wallets.data);
  list.current = wallets.data;
  const gone = useRef(false);
  useEffect(() => {
    gone.current = false;
    return () => {
      gone.current = true;
    };
  }, []);
  return useCallback(
    (o, shownHere) => {
      if (!o) return;
      // The window's wallet as App picks it: the chosen one, else the first.
      const all = list.current ?? [];
      const shown = all.find((w) => w.walletId === ui.get().walletId) ?? all[0];
      const elsewhere = shown !== undefined && shown.walletId !== walletId;
      const key = `contacts:${walletId}:${o.kind}:${o.contact ?? ""}`;
      clearAlert(key);
      if (shownHere && !gone.current && !elsewhere) return;
      const wallet = all.find((w) => w.walletId === walletId)?.name ?? t("contacts.toast.anotherWallet");
      const text = elsewhere ? t("contacts.toast.inWallet", { wallet, text: o.text }) : o.text;
      if (o.ok) showToast(text);
      else showAlert(text, key);
    },
    [walletId],
  );
}

/** How long an armed refocus waits for its row to leave (the re-read after the action). */
const REFOCUS_WAIT_MS = 10_000;

/** Focus after an action whose control leaves with its row (D28). `arm(key, index)` once the
 * action succeeded; when `present(key)` turns false within REFOCUS_WAIT_MS and focus fell to the
 * body with the control, `target(index)` takes it. Focus the user moved meanwhile stays where it
 * is. */
export function useRefocus(present: (key: string) => boolean, target: (index: number) => HTMLElement | null | undefined) {
  const [armed, setArmed] = useState<{ key: string; index: number } | null>(null);
  // A row that stays (its re-read failed) disarms: a later change must not move focus.
  useEffect(() => {
    if (!armed) return;
    const t = window.setTimeout(() => setArmed((a) => (a === armed ? null : a)), REFOCUS_WAIT_MS);
    return () => window.clearTimeout(t);
  }, [armed]);
  useEffect(() => {
    if (!armed || present(armed.key)) return;
    setArmed(null);
    const el = target(armed.index);
    if (!el) return;
    // While the passphrase sheet closes the shell is still inert and takes no focus (sheet.tsx):
    // try again for a few frames, as long as focus stays lost.
    const settle = (tries: number) => {
      const active = document.activeElement;
      if (active && active !== document.body) return;
      if (!el.hasAttribute("tabindex") && !el.matches("button, a[href], input, select, textarea")) el.tabIndex = -1;
      el.focus();
      if (document.activeElement !== el && tries > 0) requestAnimationFrame(() => settle(tries - 1));
    };
    settle(10);
  });
  return useCallback((key: string, index = 0) => setArmed({ key, index }), []);
}

/** Focuses `target` without a scroll jump, scrolling it into view only when it is outside the window. */
export function focusInView(target: HTMLElement | null) {
  if (!target) return;
  target.focus({ preventScroll: true });
  const box = target.getBoundingClientRect();
  if (box.top < 0 || box.bottom > window.innerHeight || box.left < 0 || box.right > window.innerWidth) target.scrollIntoView({ block: "nearest", inline: "nearest" });
}

/** The contact whose detail takes focus at its name when it next shows: Add contact's Show left
 * with the button that had focus, and the detail may show only once the list has read it. */
let detailFocus: string | null = null;

/** Asks the detail of `contact` to take focus when it shows. */
export function focusDetailWhenShown(contact: string) {
  detailFocus = contact;
}

/** Drops the request unless it is for `contact` (`null`: drops it): another selection, Close or
 * leaving Contacts supersedes it. */
export function dropDetailFocusUnless(contact: string | null) {
  if (detailFocus !== contact) detailFocus = null;
}

/** Whether the detail of `contact`, now shown, should take focus; answers true once. */
export function takeDetailFocus(contact: string): boolean {
  if (detailFocus !== contact) return false;
  detailFocus = null;
  return true;
}
