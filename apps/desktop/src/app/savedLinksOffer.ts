// When the window offers the link stash (DP5-01 D11): once at launch, once after onboarding
// finishes (`trigger`) and once after each unlock, as soon as a wallet shows with nothing in
// front of it. The offer is the "Links you opened" dialog, and only when links wait. After
// onboarding an invitation goes first: the newest opens the claim screen instead (DP5-03 D2),
// and the dialog waits for the next offer.

import { useCallback, useEffect, useRef, useState } from "react";
import type { Backend } from "../api/backend";
import { offerReady, offerTarget } from "./savedLinks";
import { navigate, ui } from "./ui";

export function useSavedLinksOffer(backend: Backend, s: { hasWallet: boolean; locked: boolean; covered: boolean }) {
  const [pending, setPending] = useState(true);
  /** The pending offer is onboarding's (`trigger`). */
  const welcome = useRef(false);
  const wasLocked = useRef(s.locked);
  useEffect(() => {
    if (wasLocked.current && !s.locked) setPending(true);
    wasLocked.current = s.locked;
  }, [s.locked]);
  // What holds at the answer: the window may have locked or opened something meanwhile.
  const now = useRef(s);
  useEffect(() => {
    now.current = s;
  });
  const ready = offerReady({ pending, ...s });
  useEffect(() => {
    if (!ready) return;
    setPending(false);
    const afterOnboarding = welcome.current;
    welcome.current = false;
    backend.stashedLinks().then(
      (links) => {
        const target = offerTarget(links, afterOnboarding);
        if (target === null || !offerReady({ pending: true, ...now.current })) return;
        if (target === "sheet") ui.set({ dialog: "savedLinks" });
        else navigate("joinDashPay", { invitation: { linkId: target.claim, entry: "welcome" } });
      },
      // Nothing to offer; the sheet reports a failed read when it is opened.
      () => {},
    );
  }, [ready, backend]);
  return {
    trigger: useCallback(() => {
      welcome.current = true;
      setPending(true);
    }, []),
  };
}
