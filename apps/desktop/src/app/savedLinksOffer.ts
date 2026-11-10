// When the window offers the link stash (DP5-01 D11): once at launch, once after onboarding
// finishes (`trigger`) and once after each unlock, as soon as a wallet shows with nothing in
// front of it. The offer is the "Links you opened" dialog, and only when links wait. After
// onboarding an invitation goes first: the newest opens the claim screen instead (DP5-03 D2),
// and the dialog waits for the next offer.

import { useCallback, useEffect, useRef, useState } from "react";
import type { Backend } from "../api/backend";
import { liveLinkCount, offerReady, offerTarget } from "./savedLinks";
import { navigate, ui } from "./ui";

export function useSavedLinksOffer(backend: Backend, s: { hasWallet: boolean; locked: boolean; covered: boolean }) {
  const [pending, setPending] = useState(true);
  /** The pending offer is onboarding's (`trigger`). */
  const welcome = useRef(false);
  /** Live links applied when the pending offer was armed (launch, onboarding done, an unlock). */
  const armedAt = useRef(liveLinkCount());
  const arm = useCallback((keep = false) => {
    if (!keep) armedAt.current = liveLinkCount();
    setPending(true);
  }, []);
  const wasLocked = useRef(s.locked);
  useEffect(() => {
    // An unlock with the offer still unanswered (a launch while locked) keeps its count: a live
    // link applied while locked opened its page behind the lock screen and wins.
    if (wasLocked.current && !s.locked) arm(pending);
    wasLocked.current = s.locked;
  }, [s.locked, pending, arm]);
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
        // A live link applied since the offer was armed has opened its page (the claim screen, Pay,
        // a contact): while the wallets loaded, or while this read was out and `now` (refreshed
        // after a commit) did not show it yet. The dialog does not open over it; the stash still
        // holds the link for the next offer.
        if (liveLinkCount() !== armedAt.current) return;
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
      arm();
    }, [arm]),
  };
}
