// The bell's server state (DP2-09 D11, D12): the identity's event feed, a page at a time, under
// `["dashpay", …]` so the `platform` and `dashPayNews` signals re-read it, and the read marks of
// an opening.

import { useEffect, useState } from "react";
import { useInfiniteQuery } from "@tanstack/react-query";
import { useBackend } from "../api/queries";
import { EVENTS_PAGE, newestId } from "./model";

/** `identity`'s feed: pending requests on the first page, then its events newest first. */
export function useEventFeed(walletId: string, identity: string) {
  const b = useBackend();
  return useInfiniteQuery({
    queryKey: ["dashpay", "events", walletId, identity] as const,
    queryFn: ({ pageParam }) => b.dashpayEvents(walletId, identity, pageParam, EVENTS_PAGE),
    initialPageParam: null as number | null,
    getNextPageParam: (last) => last.nextCursor,
  });
}

/** Marks the feed read once its first page shows (D11), and returns the ids that were unread
 * then: they stay under New for this opening. */
export function useReadOnOpen(walletId: string, identity: string, feed: ReturnType<typeof useEventFeed>): ReadonlySet<number> {
  const b = useBackend();
  const first = feed.data?.pages[0];
  const [unreadAtOpen, setUnread] = useState<ReadonlySet<number> | null>(null);
  useEffect(() => {
    if (!first || unreadAtOpen !== null) return;
    setUnread(new Set(first.new.map((e) => e.id)));
    const upTo = newestId(first);
    // A failed mark leaves the events unread for the next opening; nothing to say now.
    if (first.new.length > 0 && upTo !== null) b.dashpayMarkRead(walletId, identity, upTo).catch(() => undefined);
  }, [b, walletId, identity, first, unreadAtOpen]);
  return unreadAtOpen ?? new Set();
}
