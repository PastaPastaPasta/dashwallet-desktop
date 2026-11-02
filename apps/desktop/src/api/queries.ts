// Server state lives in TanStack Query. Keys mirror the dw-app commands, so an engine
// signal names exactly the queries to refetch (events.ts).

import { createContext, useContext } from "react";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import type { Backend } from "./backend";
import type { ShieldedSpendQuote } from "./generated/ShieldedSpendQuote";
import type { ShieldedStatus } from "./generated/ShieldedStatus";
import type { ShieldQuote } from "./generated/ShieldQuote";
import { creditsToDuffs } from "./pending";
import { feedSync, rawSync, readTicket, useShownSync } from "./syncDisplay";
import type { ShieldedRequest } from "../shielded/flow";
import type { DashPayStatus } from "./generated/DashPayStatus";
import type { HistoryRequest } from "./generated/HistoryRequest";
import type { IdentitySummary } from "./generated/IdentitySummary";
import type { LockOutcome } from "./generated/LockOutcome";
import type { LockProgress } from "./generated/LockProgress";
import { reconcileLockReport } from "./lockReport";
import { isChangingUsernameInRequests } from "./username";

/** A history query without its paging fields. */
export type HistoryFilter = Omit<HistoryRequest, "cursor" | "limit">;

export const BackendContext = createContext<Backend | null>(null);

export function useBackend(): Backend {
  const b = useContext(BackendContext);
  if (!b) throw new Error("BackendContext missing");
  return b;
}

export const keys = {
  network: ["network"] as const,
  lockStatus: ["lockStatus"] as const,
  lastLockReport: ["lastLockReport"] as const,
  /** Reads `leases`, `dispatchStatus` and `lockStatus` (lockReport.ts). */
  lockReconcile: ["lockReconcile"] as const,
  /** A lock's drain in progress, set by the event bridge (no engine query). */
  lockDrain: ["lockDrain"] as const,
  wallets: ["wallets"] as const,
  balance: (walletId: string) => ["balance", walletId] as const,
  // Under "dashpay" like the Platform keys: `platform{domain:"shielded"}` re-reads them all.
  shieldedStatus: (walletId: string) => ["dashpay", "shielded", "status", walletId] as const,
  shieldedHistory: (walletId: string) => ["dashpay", "shielded", "history", walletId] as const,
  shieldedAddress: (walletId: string) => ["dashpay", "shielded", "address", walletId] as const,
  shieldedQuote: (walletId: string, request: string) => ["shieldedQuote", walletId, request] as const,
  // Under "dashpay": a `platform{domain:"platformAddresses"}` event is a domain events.ts does not
  // name, which it answers by invalidating everything under that prefix.
  platformAddresses: (walletId: string) => ["dashpay", "platformAddresses", walletId] as const,
  platformActivity: (walletId: string) => ["dashpay", "platformActivity", walletId] as const,
  // Not under "dashpay": a quote is a proven network read, which a signal must not repeat while Confirm is open.
  platformMoveQuote: (walletId: string) => ["platformMoveQuote", walletId] as const,
  networkChoice: ["networkChoice"] as const,
  sync: ["sync"] as const,
  historyAll: (walletId: string) => ["history", walletId] as const,
  history: (walletId: string, filter: HistoryFilter, limit: number) => ["history", walletId, filter, limit] as const,
  txDetailAll: (walletId: string) => ["txDetail", walletId] as const,
  txDetail: (walletId: string, txid: string) => ["txDetail", walletId, txid] as const,
  /** Under the detail's key, so `historyChanged` refreshes the actions with the facts. */
  txDetailExtras: (walletId: string, txid: string) => ["txDetail", walletId, txid, "extras"] as const,
  /** Every receive read of a wallet: a payment uses the current address up. */
  receiveAll: (walletId: string) => ["receive", walletId] as const,
  receiveAddress: (walletId: string) => ["receive", walletId, "address"] as const,
  receiveRequests: (walletId: string) => ["receive", walletId, "requests"] as const,
  receiveUsername: (walletId: string) => ["receive", walletId, "username"] as const,
  usernameInRequests: (walletId: string) => ["receive", walletId, "usernameInRequests"] as const,
  /** The coin control list of a wallet: history and balance changes move coins. */
  coinsAll: (walletId: string) => ["coins", walletId] as const,
  utxos: (walletId: string) => ["coins", walletId, "utxos"] as const,
  feePolicy: ["feePolicy"] as const,
  // Settings (U-04). Automatic backups and the policy share a prefix: a policy change trims them.
  backupsAll: ["backups"] as const,
  automaticBackups: ["backups", "automatic"] as const,
  backupPolicy: ["backups", "policy"] as const,
  backupDirectory: ["backups", "directory"] as const,
  dust: ["dust"] as const,
  nodeInfo: ["nodeInfo"] as const,
  paymentAuth: ["paymentAuth"] as const,
  /** Every CoinJoin read (U-06): a settings write re-reads them all. */
  coinjoinAll: ["coinjoin"] as const,
  coinjoinStatus: (walletId: string) => ["coinjoin", "status", walletId] as const,
  coinjoinSettings: ["coinjoin", "settings"] as const,
  /** Every governance read (the `governance_*` commands, BL-232): a `governance` event makes them all stale. */
  governanceAll: ["governance"] as const,
  /** Every DashPay read below: a `platform` or `dashPayNews` signal makes them all stale. */
  dashpayAll: ["dashpay"] as const,
  /** Every wallet's DashPay status: sync moves `noIdentity`'s `waitingForSync` (BL-153 D9). */
  dashpayStatusAll: ["dashpay", "status"] as const,
  dashpayStatus: (walletId: string) => ["dashpay", "status", walletId] as const,
  identities: (walletId: string) => ["dashpay", "identities", walletId] as const,
  unread: (walletId: string, identity: string) => ["dashpay", "unread", walletId, identity] as const,
  requests: (walletId: string, identity: string) => ["dashpay", "requests", walletId, identity] as const,
  /** Every Contacts list of a wallet (DP2-07): a mined payment moves `lastActivityAt` too. */
  contactsAll: (walletId: string) => ["dashpay", "contacts", walletId] as const,
  /** The credit costs (DP2-09 D10): slow to change, read again with every DashPay signal. */
  costTable: (walletId: string) => ["dashpay", "costTable", walletId] as const,
  /** An identity's event feed (DP2-09 D11). */
  events: (walletId: string, identity: string) => ["dashpay", "events", walletId, identity] as const,
};

/** Opens the network; its lock status seeds `lockStatus`, so the shell never renders
 * one frame unlocked before the lock state is known. Resolves to the lock status. */
export function useOpenNetwork() {
  const b = useBackend();
  const client = useQueryClient();
  return useQuery({
    queryKey: keys.network,
    queryFn: async () => {
      const { lock } = await b.openNetwork();
      client.setQueryData(keys.lockStatus, lock);
      return lock;
    },
  });
}

export function useLockStatus() {
  const b = useBackend();
  return useQuery({ queryKey: keys.lockStatus, queryFn: () => b.lockStatus() });
}

export function useWallets() {
  const b = useBackend();
  return useQuery({ queryKey: keys.wallets, queryFn: () => b.listWallets() });
}

export function useBalance(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.balance(walletId), queryFn: () => b.balance(walletId) });
}

/** The wallet's Platform addresses; `null` while dw-app has no such command (api/pending.ts). */
export function usePlatformAddresses(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.platformAddresses(walletId), queryFn: () => b.platformAddresses(walletId), retry: false });
}

const PENDING_MOVE_REFETCH_MS = 30_000;

/** The Platform-address activity rows, newest first; `null` while dw-app has no such command. */
export function usePlatformActivity(walletId: string) {
  const b = useBackend();
  return useQuery({
    queryKey: keys.platformActivity(walletId),
    queryFn: () => b.platformActivity(walletId),
    retry: false,
    // A move Platform has not confirmed settles in the engine within minutes: look again.
    refetchInterval: (q) => (q.state.data?.some((a) => a.kind === "moveToCore" && a.state === "pending") ? PENDING_MOVE_REFETCH_MS : false),
  });
}

/** What "Move to Dash wallet" would take and pay out, read while its sheet asks to confirm. */
export function usePlatformMoveQuote(walletId: string, enabled: boolean) {
  const b = useBackend();
  return useQuery({
    queryKey: keys.platformMoveQuote(walletId),
    queryFn: () => b.platformMoveToCoreQuote(walletId, { kind: "all" }),
    enabled,
    retry: false,
    gcTime: 0,
  });
}

/** The shielded account's status; `null` while dw-app has no such command (api/pending.ts). */
export function useShieldedStatus(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.shieldedStatus(walletId), queryFn: () => b.shieldedStatus(walletId), retry: false });
}

/** The shielded activity rows, pending first; `null` while dw-app has no such command. */
export function useShieldedHistory(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.shieldedHistory(walletId), queryFn: () => b.shieldedHistory(walletId, 0, 200), retry: false });
}

/** A quote of either kind, with the grant it asks for. */
export type ShieldedQuoteView = { kind: "add"; quote: ShieldQuote } | { kind: "spend"; quote: ShieldedSpendQuote };

/** The pool's balance in duffs (a credit is a thousandth of one); `null` while it is no reading:
 * the command is absent, the network is not active or the account has not been scanned yet. */
export function shieldedDuffs(status: ShieldedStatus | null): number | null {
  return status?.active && status.ready ? creditsToDuffs(status.balance) : null;
}

/** What `request` costs and buys, read while the sheet asks to confirm it. */
export function useShieldedQuote(walletId: string, request: ShieldedRequest | null) {
  const b = useBackend();
  return useQuery({
    queryKey: keys.shieldedQuote(walletId, JSON.stringify(request)),
    queryFn: async (): Promise<ShieldedQuoteView> => {
      switch (request?.kind) {
        case "add": {
          return { kind: "add", quote: await b.shieldQuote(walletId, request.duffs) };
        }
        case "send": {
          return { kind: "spend", quote: await b.shieldedSendQuote(walletId, request.to, request.credits) };
        }
        case "out": {
          return { kind: "spend", quote: await b.unshieldQuote(walletId, request.credits, null) };
        }
        default:
          throw new Error("nothing to quote");
      }
    },
    enabled: request !== null && request.kind !== "resume",
    retry: false,
    gcTime: 0,
  });
}

/** The shielded and Platform balances in duffs; `null` while dw-app has no command for one
 * (api/pending.ts) or it failed, `undefined` while it is being read. */
export function usePoolBalances(walletId: string) {
  const shielded = useShieldedStatus(walletId);
  const platform = usePlatformAddresses(walletId);
  return {
    shielded: shielded.isError ? null : shielded.data === undefined ? undefined : shieldedDuffs(shielded.data),
    // What a move of everything would take (`movableCredits`): Home counts that, not a remainder too small to move.
    platform: platform.isError ? null : platform.data === undefined ? undefined : (platform.data?.duffs ?? null),
    /** Credits on the Platform addresses that are too small to move (`credits - movableCredits`). */
    platformDust: platform.data ? String(BigInt(platform.data.credits) - BigInt(platform.data.movableCredits)) : "0",
  };
}

/** The saved network choice and how this run relates to it; `null` while dw-app has no such command. */
export function useNetworkChoice() {
  const b = useBackend();
  return useQuery({ queryKey: keys.networkChoice, queryFn: () => b.networkChoice(), retry: false });
}

/** The sync as the status bar and the screens show it: the engine's reading, with a new block's
 * one-block flap smoothed away (api/syncDisplay.ts). The query itself holds the raw reading. */
export function useSyncStatus() {
  const b = useBackend();
  const query = useQuery({
    queryKey: keys.sync,
    queryFn: async ({ signal }) => {
      const ticket = readTicket(b);
      const raw = await b.syncStatus();
      // A reply that was superseded or cancelled is not a reading: the query keeps the newest one.
      if (signal.aborted || !feedSync(b, raw, Date.now(), ticket)) return rawSync(b) ?? raw;
      return raw;
    },
  });
  const shown = useShownSync(b);
  // The query's own state, with `data` the shown status once there is one.
  return query.data === undefined ? query : { ...query, data: shown ?? query.data };
}

/** Pages of `limit` records; `fetchNextPage` follows the engine's keyset cursor. */
export function useHistory(walletId: string, filter: HistoryFilter, limit: number) {
  const b = useBackend();
  return useInfiniteQuery({
    queryKey: keys.history(walletId, filter, limit),
    queryFn: ({ pageParam }) => b.historyPage(walletId, { ...filter, cursor: pageParam, limit }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
  });
}

export function useTxDetail(walletId: string, txid: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.txDetail(walletId, txid), queryFn: () => b.txDetail(walletId, txid) });
}

export function useTxDetailExtras(walletId: string, txid: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.txDetailExtras(walletId, txid), queryFn: () => b.txDetailExtras(walletId, txid) });
}

export function useReceiveAddress(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.receiveAddress(walletId), queryFn: () => b.currentReceiveAddress(walletId) });
}

/** The DashPay username a Receive QR carries (DP3-03); `null` without one. */
export function useReceiveUsername(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.receiveUsername(walletId), queryFn: () => b.receiveUsername(walletId) });
}

/** "Include my username in payment requests" for the wallet (DP3-03b); on by default. Off while a change of it is under way (D22). */
export function useUsernameInRequests(walletId: string) {
  const b = useBackend();
  return useQuery({
    queryKey: keys.usernameInRequests(walletId),
    queryFn: async () => (await b.usernameInRequests(walletId)) && !isChangingUsernameInRequests(walletId),
  });
}

/**
 * Whether the wallet's request URIs may show its username now (DP3-03b D22): only once the
 * setting has read on. A failed read is off; `undefined` until it first answers.
 */
export function useUsernameAllowed(walletId: string): boolean | undefined {
  const setting = useUsernameInRequests(walletId);
  return setting.isError ? false : setting.data;
}

export function useReceiveRequests(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.receiveRequests(walletId), queryFn: () => b.receiveRequests(walletId) });
}

/** The QR modules of `text` (dw-uri; level L). */
export function useQrMatrix(text: string | undefined) {
  const b = useBackend();
  return useQuery({ queryKey: ["qr", text ?? ""] as const, queryFn: () => b.qrMatrix(text!), enabled: !!text, staleTime: Infinity });
}

/** Every coin of the wallet, the locked and reserved ones too (coin control, U-03 D7). */
export function useUtxos(walletId: string, enabled: boolean) {
  const b = useBackend();
  return useQuery({
    queryKey: keys.utxos(walletId),
    queryFn: () => b.utxos(walletId, { includeLocked: true, fullyMixedOnly: false, minConfirmations: null }),
    enabled,
  });
}

/** The network's fee rules (constant on SPV). */
export function useFeePolicy() {
  const b = useBackend();
  return useQuery({ queryKey: keys.feePolicy, queryFn: () => b.feePolicy(), staleTime: Infinity });
}

/** "Require authentication for every payment"; read again each time the confirm sheet opens. */
export function usePaymentAuthentication() {
  const b = useBackend();
  return useQuery({ queryKey: keys.paymentAuth, queryFn: () => b.paymentAuthentication(), staleTime: 0, gcTime: 0, retry: false });
}

/** The network's last lock report (E0-13 D3); `null` until something locks the vault. */
export function useLastLockReport() {
  const b = useBackend();
  return useQuery({ queryKey: keys.lastLockReport, queryFn: () => b.lastLockReport() });
}

/** What the engine knows now about what `report` left pending or left out. */
export function useLockReconcile(report: LockOutcome, enabled: boolean) {
  const b = useBackend();
  return useQuery({ queryKey: [...keys.lockReconcile, report] as const, queryFn: () => reconcileLockReport(report, b), enabled });
}

export type LockDrain = Extract<LockProgress, { kind: "draining" }>;

/** The drain of a lock in progress (`lockProgress` `draining` until `done`). Until then
 * `lastLockReport` is still the previous lock's. A refetch of every query (an overflow) resets
 * it, so a dropped `done` cannot hide reports for good. */
export function useLockDrain() {
  return useQuery<LockDrain | null>({ queryKey: keys.lockDrain, queryFn: () => null, initialData: null });
}

/** The main identity a status names: a contested username's too (BL-153 D11). */
export function mainIdentityOf(status: DashPayStatus | undefined): string | null {
  switch (status?.kind) {
    case "ready":
      return status.main;
    case "contestPending":
      return status.identity;
    default:
      return null;
  }
}

/** `main`'s summary in a read of the identities; `undefined` while unknown or when the read
 * failed, a refetch included: the answer cached before it may be wrong now (DP1-08 D24). */
export function mainSummaryOf(
  identities: { data?: IdentitySummary[]; isError: boolean },
  main: string | null,
): IdentitySummary | undefined {
  return identities.isError ? undefined : identities.data?.find((i) => i.identity === main);
}

/** The identity chip's, the sidebar's and the bell's DashPay state (DASHPAY §4
 * `DashPayStatusModel`): the status, the main identity's summary, its unread events and the
 * contact requests waiting. Each read is `undefined` while unknown or when it fails, a refetch
 * that failed included: the answer cached before it may be wrong now (DP1-08 D24). A contested
 * username's identity is the main one too (BL-153 D11). */
export function useDashPayShell(walletId: string | undefined) {
  const b = useBackend();
  const status = useQuery({
    queryKey: keys.dashpayStatus(walletId ?? ""),
    queryFn: () => b.dashpayStatus(walletId!),
    enabled: !!walletId,
  });
  const main = mainIdentityOf(status.data);
  const identities = useQuery({
    queryKey: keys.identities(walletId ?? ""),
    queryFn: () => b.identities(walletId!),
    enabled: !!walletId && main !== null,
  });
  const unread = useQuery({
    queryKey: keys.unread(walletId ?? "", main ?? ""),
    queryFn: () => b.dashpayUnreadCount(walletId!, main!),
    enabled: !!walletId && main !== null,
  });
  const requests = useQuery({
    queryKey: keys.requests(walletId ?? "", main ?? ""),
    queryFn: async () => {
      const list = await b.dashpayContacts(walletId!, main!, { sections: ["requests"], sort: "dateAdded", text: null });
      return list.sections.reduce((n, s) => n + (s.section === "requests" ? s.contacts.length : 0), 0);
    },
    enabled: !!walletId && main !== null,
  });
  return {
    status: status.isError ? undefined : status.data,
    main: mainSummaryOf(identities, main),
    unread: unread.data ?? null,
    requests: requests.data ?? 0,
  };
}

// Settings (U-04): read when the tab shows, kept until the change that moves them.
export function useAutomaticBackups() {
  const b = useBackend();
  // The engine writes an automatic backup on its own after an import or a restore and says nothing
  // when it is done, so the list is read again whenever the tab shows (D28).
  return useQuery({ queryKey: keys.automaticBackups, queryFn: () => b.automaticBackups(null), refetchOnMount: "always" });
}

export function useBackupPolicy() {
  const b = useBackend();
  return useQuery({ queryKey: keys.backupPolicy, queryFn: () => b.backupPolicy() });
}

export function useBackupDirectory() {
  const b = useBackend();
  return useQuery({ queryKey: keys.backupDirectory, queryFn: () => b.backupDirectory() });
}

export function useDustProtection() {
  const b = useBackend();
  return useQuery({ queryKey: keys.dust, queryFn: () => b.dustProtection() });
}

/** Node facts (the data directory); read again each time a tab that shows them opens. */
export function useNodeInfo() {
  const b = useBackend();
  return useQuery({ queryKey: keys.nodeInfo, queryFn: () => b.nodeInfo(), staleTime: 0 });
}
