// The renderer half of the event bridge: each engine event invalidates the queries that
// read what changed. Rows are never pushed; the queries refetch (DESIGN-opus §1.5 rule 4).
// Events arrive in the engine's order on one channel (`ENGINE_EVENT`); see dw-app's bridge.rs.
// A queue overflow drops events and sends one `eventsOverflowed`, which reloads everything.

import type { InfiniteData, QueryClient, QueryKey } from "@tanstack/react-query";
import type { Backend } from "./backend";
import type { AppEvent } from "./generated/commands";
import type { DashPayStatus } from "./generated/DashPayStatus";
import type { HistoryRows } from "./generated/HistoryRows";
import type { PlatformChangeName } from "./generated/PlatformChangeName";
import type { UnfinishedTransfers } from "./generated/UnfinishedTransfers";
import { profileKeys } from "../dashpay/profileQueries";
import { dashpayKeys } from "../dashpay/queries";
import { onTopUpSettled } from "../dashpay/topup";
import { onWithdrawSettled } from "../dashpay/withdraw";
import { announceReceived } from "../payments/received";
import { onMoveSettled } from "../platform/move";
import { onRecoverySettled } from "../recovery/model";
import { onShieldedSettled } from "../shielded/flow";
import { keys } from "./queries";
import { announceable, feedSync } from "./syncDisplay";

/** DP1-09 D28: a status read while a sync pass held the wallet's state is `startupIncomplete` (the
 * engine's identity read was not live, so it proves nothing), and no Platform signal follows when
 * the bring-up has nothing to report: sync re-reads it like `noIdentity`'s. */
const withoutIdentity = (kind: DashPayStatus["kind"] | undefined) => kind === "noIdentity" || kind === "startupIncomplete";

/** The query keys an event makes stale; `"all"` means every query but the network itself. */
export function invalidationsFor(event: AppEvent): QueryKey[] | "all" {
  switch (event.kind) {
    case "sessionOpened":
    case "sessionClosed":
    // The app's queue overflowed and dropped events: whatever they said, the queries know.
    case "eventsOverflowed":
      return "all";
    // A seed attached to a loaded watch-only wallet is a `walletCreated` of the same id: its
    // DashPay status stops being watch-only, and no Platform signal says so while SPV is stopped
    // (BL-153 D19).
    case "walletCreated":
    case "walletRemoved":
    case "walletLoadChanged":
      return [keys.wallets, keys.dashpayStatus(event.walletId)];
    // Sync ends `noIdentity`'s `waitingForSync` and sends no Platform signal (BL-153 D9); only
    // statuses without an identity are re-read (bridgeEvents), `startupIncomplete` too (DP1-09 D28).
    // The Platform loops start and stop with SPV, also without one (DP6-03 D6).
    case "spvStateChanged":
    case "sync":
      return [keys.sync, keys.dashpayStatusAll, dashpayKeys.syncInfoAll, keys.unfinishedTransfersAll];
    // The username quote's fee grows with the wallet's coins (DP1-08).
    case "balances":
      // A status read keeps an idle wallet in the engine's CoinJoin refresher (U-06 D7).
      return [keys.balance(event.walletId), keys.wallets, keys.coinsAll(event.walletId), dashpayKeys.quote(event.walletId), keys.coinjoinStatus(event.walletId)];
    case "historyChanged":
      return [
        keys.historyAll(event.walletId),
        keys.txDetailAll(event.walletId),
        keys.receiveAll(event.walletId),
        keys.coinsAll(event.walletId),
        keys.contactsAll(event.walletId),
      ];
    // A registration parks for an unlock or an authorization without writing its row, so
    // without a Platform signal (DP1-08 D19).
    case "vaultLockState":
      return [keys.lockStatus, keys.wallets, keys.lockReconcile, dashpayKeys.registrationsAll];
    case "lockProgress":
      // `done` carries the report itself (bridgeEvents sets it).
      return event.progress.kind === "done" ? [keys.lockStatus, keys.lockReconcile] : [];
    // The lock report's reconciliation reads leases and dispatch states (lockReport.ts); its
    // invalidations are coalesced (RECONCILE_COALESCE_MS).
    // A registration lease revoked or parked moves its row's `waiting` without a Platform
    // signal; a passphrase change revokes it without a `vaultLockState` (DP1-09 D14).
    case "leaseChanged":
      return event.lease.flow === "registration" ? [keys.lockReconcile, dashpayKeys.registrationsAll] : [keys.lockReconcile];
    // A top-up's funding step that settled leaves the stuck list (DP1-10 D3).
    case "dispatchResolved":
      return [keys.lockReconcile, profileKeys.stuckTopUpsAll];
    // The shell's chip, bell and Contacts badge (the DashPay screens come with their tasks).
    case "dashPayNews":
      return [keys.dashpayAll];
    // A contact's username lands after the first history read, and a restored send is
    // attributed after the library's DashPay pass (DP3-02 D5); a main name change moves the
    // receive reads' `user=` (DP3-03).
    case "platform":
      switch (event.change.domain) {
        case "contacts":
        case "payments":
          return [keys.dashpayAll, keys.historyAll(event.walletId), keys.txDetailAll(event.walletId)];
        // Contacts' cached profiles (display names, D4) change with the identities.
        case "identities":
          return [keys.dashpayAll, keys.historyAll(event.walletId), keys.txDetailAll(event.walletId), keys.receiveAll(event.walletId)];
        case "names":
          return [keys.dashpayAll, keys.receiveAll(event.walletId)];
        default:
          return [keys.dashpayAll];
      }
    case "coinJoin":
      return [keys.coinjoinStatus(event.walletId), keys.coinsAll(event.walletId)];
    // The store, the sync state or the list's voting keys changed (≤ 1 Hz, mg-engine §5). A new
    // block sends none, so the tip-derived clock and statuses go stale until the next (MG-02 D96).
    case "governance":
      return [keys.governanceAll];
    // A fetch, a setting, a price turning stale or expiring, mainnet opened or closed (MP-01).
    case "rates":
      return [keys.ratesAll];
    // No screen reads these yet (the notice and masternode views come with their tasks).
    // `masternodes` also follows a shared-masternode session change (mn-engine §6, MG-04b).
    // `newTransactions` is announced above (payments/received.ts), not read as a query.
    case "newTransactions":
    case "notice":
    case "masternodes":
      return [];
    default:
      return unreachable(event);
  }
}

function unreachable(event: never): never {
  throw new Error(`unhandled engine event ${JSON.stringify(event)}`);
}

/** A lock that cuts 16 flows sends a `leaseChanged` for each, and every reconciliation reads
 * `leases`, `lockStatus` and up to 128 `dispatchStatus`: one runs this long after the last of a
 * burst, and at most RECONCILE_MAX_WAIT_MS after its first event. */
/** The `platform` domains whose events move history rows' counterparties (DP3-02 D5, D25). */
const rereadsHistory = (domain: PlatformChangeName["domain"]) => domain === "contacts" || domain === "payments" || domain === "identities";

export const RECONCILE_COALESCE_MS = 100;
export const RECONCILE_MAX_WAIT_MS = 500;

/** Subscribes the query client to the backend's events; returns the unsubscribe function. */
export async function bridgeEvents(backend: Backend, client: QueryClient): Promise<() => void> {
  let reconcile: ReturnType<typeof setTimeout> | undefined;
  let reconcileBy = 0;
  const stopReconcile = () => {
    clearTimeout(reconcile);
    reconcile = undefined;
  };
  const unsubscribe = await backend.onEvent((event) => {
    if (event.kind === "notice") console.warn("engine notice", event.code, event.detail);
    // The engine's own reading, as it arrives: the display's (api/syncDisplay.ts) and what decides
    // below. Not the query's, which only follows once its refetch is back.
    if (event.kind === "sync") feedSync(backend, event.status);
    // A payment that arrives while the app is open is announced (toast, bell, Receive); a batch the
    // sync found on catching up is not news. The engine's `catchUp` is "SPV is not idle" at that
    // moment, which a payment found in a new block can be too: so it is news when the engine's own
    // latest reading says caught up, or a caught-up wallet is a few blocks short of it for a minute
    // at most (`announceable`). A wallet that has not been caught up yet is catching up. The smoothed
    // display never decides this.
    if (event.kind === "newTransactions" && (!event.catchUp || announceable(backend))) {
      void announceReceived(backend, event.walletId, event.txids);
    }
    if (event.kind === "historyChanged" || (event.kind === "platform" && rereadsHistory(event.change.domain))) {
      // Refetch only the first page of each list: every history_page call re-reads the whole
      // history in the engine, and during sync events arrive up to 4 times a second (contacts
      // events in bursts, one per identity and landing name).
      client.setQueriesData<InfiniteData<HistoryRows, string | null>>({ queryKey: keys.historyAll(event.walletId) }, (d) =>
        d && d.pages.length > 1 ? { pages: d.pages.slice(0, 1), pageParams: d.pageParams.slice(0, 1) } : d,
      );
    }
    // A wallet that comes (a restore, a re-import) or goes keeps none of its receive reads: the
    // engine may hold another "Include my username in payment requests" for it now, and a
    // cached `true` would draw the name until a refetch (DP3-03b D17).
    if (event.kind === "walletCreated" || event.kind === "walletRemoved") void client.resetQueries({ queryKey: keys.receiveAll(event.walletId) });
    if (event.kind === "lockProgress") {
      const { progress } = event;
      // The new report shows with its `done`, never the previous lock's while a refetch runs: a
      // fetch already under way may hold the previous one, so it is dropped first.
      if (progress.kind === "done") {
        void client.cancelQueries({ queryKey: keys.lastLockReport });
        client.setQueryData(keys.lastLockReport, progress.report);
      }
      // While a lock drains, the last report is the previous lock's (useLockDrain).
      client.setQueryData(keys.lockDrain, progress.kind === "draining" ? progress : null);
    }
    const stale = invalidationsFor(event);
    if (stale === "all") {
      stopReconcile();
      void client.invalidateQueries({ predicate: (q) => q.queryKey[0] !== keys.network[0] });
    } else
      for (const queryKey of stale) {
        if (queryKey === keys.dashpayStatusAll)
          void client.invalidateQueries({ queryKey, predicate: (q) => withoutIdentity((q.state.data as DashPayStatus | undefined)?.kind) });
        else if (queryKey === keys.unfinishedTransfersAll)
          // Only a list that was partial is read again: sync events arrive up to 4 times a second.
          void client.invalidateQueries({ queryKey, predicate: (q) => (q.state.data as UnfinishedTransfers | null | undefined)?.complete === false });
        else if (queryKey !== keys.lockReconcile) void client.invalidateQueries({ queryKey });
        else {
          const now = Date.now();
          if (reconcile === undefined) reconcileBy = now + RECONCILE_MAX_WAIT_MS;
          stopReconcile();
          reconcile = setTimeout(
            () => {
              reconcile = undefined;
              void client.invalidateQueries({ queryKey });
            },
            Math.min(RECONCILE_COALESCE_MS, reconcileBy - now),
          );
        }
      }
  });
  // DP1-10 D22: a top-up or a pending row's call that ended may have moved the locks, the stuck
  // fundings and the balance, and a funding left unknown sends no signal: read them again.
  const stopTopUps = onTopUpSettled(() => void client.invalidateQueries({ queryKey: keys.dashpayAll }));
  // DP6-02: a withdrawal that ended moved the balance and the recent withdrawals.
  const stopWithdrawals = onWithdrawSettled(() => void client.invalidateQueries({ queryKey: keys.dashpayAll }));
  // 0.1: a Platform-address move or a shielded call that ended moved the pools and Activity's rows.
  const stopMoves = onMoveSettled(() => void client.invalidateQueries({ queryKey: keys.dashpayAll }));
  const stopShielded = onShieldedSettled(() => void client.invalidateQueries({ queryKey: keys.dashpayAll }));
  // A finished transfer leaves the unfinished list and joins the Shielded balance.
  const stopRecovery = onRecoverySettled(() => void client.invalidateQueries({ queryKey: keys.dashpayAll }));
  return () => {
    stopReconcile();
    stopTopUps();
    stopWithdrawals();
    stopMoves();
    stopShielded();
    stopRecovery();
    unsubscribe();
  };
}
