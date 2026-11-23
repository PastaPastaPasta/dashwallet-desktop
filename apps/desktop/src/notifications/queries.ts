// The bell's server state (DP2-09 D11, D12): the identity's event feed, a page at a time, under
// `["dashpay", …]` so the `platform` and `dashPayNews` signals re-read it, and the read marks of
// an opening.

import { useEffect, useRef, useState } from "react";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import { keys, useBackend } from "../api/queries";
import { EVENTS_PAGE, newestId } from "./model";

/** `identity`'s feed: pending requests on the first page, then its events newest first. Each
 * view reads it afresh on opening: what it marks read is what it read (D11). */
export function useEventFeed(walletId: string, identity: string) {
  const b = useBackend();
  return useInfiniteQuery({
    queryKey: keys.events(walletId, identity),
    queryFn: ({ pageParam }) => b.dashpayEvents(walletId, identity, pageParam, EVENTS_PAGE),
    initialPageParam: null as number | null,
    getNextPageParam: (last) => last.nextCursor,
    refetchOnMount: "always",
  });
}

/** Marks the feed read once its first page shows (D11), and returns the ids that were unread
 * then: they stay under New for this opening. Once the mark is stored the bell's badge re-reads. */
export function useReadOnOpen(walletId: string, identity: string, feed: ReturnType<typeof useEventFeed>): ReadonlySet<number> {
  const b = useBackend();
  const client = useQueryClient();
  // The first page as this view read it, not a cached one from an earlier opening.
  const first = feed.isFetchedAfterMount && !feed.isFetching ? feed.data?.pages[0] : undefined;
  const [unreadAtOpen, setUnread] = useState<ReadonlySet<number> | null>(null);
  // Once per opening, StrictMode's second effect run included.
  const marked = useRef(false);
  useEffect(() => {
    if (!first || marked.current) return;
    marked.current = true;
    setUnread(new Set(first.new.map((e) => e.id)));
    const upTo = newestId(first);
    // A failed mark leaves the events unread for the next opening; nothing to say now.
    if (first.new.length > 0 && upTo !== null)
      b.dashpayMarkRead(walletId, identity, upTo).then(() => void client.invalidateQueries({ queryKey: keys.unread(walletId, identity) }), () => undefined);
  }, [b, client, walletId, identity, first]);
  return unreadAtOpen ?? new Set();
}
