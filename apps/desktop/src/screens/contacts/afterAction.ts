// What a surface does once one of its contact actions ended (DP2-09 D23, D27, D28): say the
// outcome where the user is, and give keyboard focus a place when the action's control left
// with its row.

import { useCallback, useEffect, useRef, useState } from "react";
import { useWallets } from "../../api/queries";
import { ui } from "../../app/ui";
import { showToast } from "../../components/toast";
import type { ActionOutcome } from "../../contacts/actions";

/** Says an outcome in a toast when the view it ran in cannot (D23): that view is gone (closed,
 * left) or shows no outcome of its own (`shownHere` false). An outcome that ends after the window
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
      if (shownHere && !gone.current && !elsewhere) return;
      const name = all.find((w) => w.walletId === walletId)?.name ?? "Another wallet";
      showToast(elsewhere ? `${name}: ${o.text}` : o.text);
    },
    [walletId],
  );
}

/** Focus after an action whose control leaves with its row (D28). `arm(key, index)` once the
 * action succeeded; when `present(key)` turns false and focus fell to the body with the control,
 * `target(index)` takes it. Focus the user moved meanwhile stays where it is. */
export function useRefocus(present: (key: string) => boolean, target: (index: number) => HTMLElement | null | undefined) {
  const [armed, setArmed] = useState<{ key: string; index: number } | null>(null);
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
