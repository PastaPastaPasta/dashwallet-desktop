// The syncs the status bar and Sync progress list (format/syncItems.ts), read for the open wallet.
// Platform and Shielded are read again every few seconds while one is still syncing or its read
// failed, as their progress comes with no event; never while the window is hidden (TanStack's
// default). Shielded's is the read that answers while a pass holds the note store.

import { useQuery } from "@tanstack/react-query";
import { dashpayKeys } from "../dashpay/queries";
import { coreItem, platformItem, shieldedItem, type SyncItem } from "../format/syncItems";
import { SHIELDED_SCAN_POLL_MS, useBackend, useShieldedBind, useShieldedSyncInfo, useSyncStatus } from "./queries";

/** The open wallet's syncs, the network's first; `null` items are not on this network or wallet. */
export function useSyncItems(walletId: string | null): (SyncItem | null)[] {
  const b = useBackend();
  const sync = useSyncStatus().data;
  const platform = useQuery({
    queryKey: dashpayKeys.syncInfo(walletId ?? ""),
    queryFn: () => b.dashpaySyncStatus(walletId!),
    enabled: walletId !== null,
    retry: false,
    refetchInterval: (q) => (q.state.status === "error" || platformItem(q.state.data)?.state === "syncing" ? SHIELDED_SCAN_POLL_MS : false),
  });
  const shielded = useShieldedSyncInfo(walletId ?? "", walletId !== null);
  useShieldedBind(walletId ?? "", shielded.data, walletId !== null);
  if (walletId === null) return [coreItem(sync)];
  return [coreItem(sync), platformItem(platform.data, platform.isError), shieldedItem(shielded.data, shielded.isError)];
}
