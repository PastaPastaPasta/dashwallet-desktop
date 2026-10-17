// When the window offers the link stash (DP5-01 D11): once at launch, once after onboarding
// finishes (`trigger`) and once after each unlock, as soon as a wallet shows with nothing in
// front of it. The offer is the "Links you opened" dialog, and only when links wait.

import { useCallback, useEffect, useRef, useState } from "react";
import type { Backend } from "../api/backend";
import { offerReady } from "./savedLinks";
import { ui } from "./ui";

export function useSavedLinksOffer(backend: Backend, s: { hasWallet: boolean; locked: boolean; covered: boolean }) {
  const [pending, setPending] = useState(true);
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
    backend.stashedLinks().then(
      (links) => {
        if (links.length > 0 && offerReady({ pending: true, ...now.current })) ui.set({ dialog: "savedLinks" });
      },
      // Nothing to offer; the sheet reports a failed read when it is opened.
      () => {},
    );
  }, [ready, backend]);
  return { trigger: useCallback(() => setPending(true), []) };
}
