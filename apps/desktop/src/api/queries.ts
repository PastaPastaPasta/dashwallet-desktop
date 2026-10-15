// Server state lives in TanStack Query. Keys mirror the dw-app commands, so an engine
// signal names exactly the queries to refetch (events.ts).

import { createContext, useContext } from "react";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import type { Backend } from "./backend";
import type { DashPayStatus } from "./generated/DashPayStatus";
import type { HistoryRequest } from "./generated/HistoryRequest";
import type { LockOutcome } from "./generated/LockOutcome";
import type { LockProgress } from "./generated/LockProgress";
import { reconcileLockReport } from "./lockReport";

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

export function useSyncStatus() {
  const b = useBackend();
  return useQuery({ queryKey: keys.sync, queryFn: () => b.syncStatus() });
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
function mainIdentityOf(status: DashPayStatus | undefined): string | null {
  switch (status?.kind) {
    case "ready":
      return status.main;
    case "contestPending":
      return status.identity;
    default:
      return null;
  }
}

/** The identity chip's, the sidebar's and the bell's DashPay state (DASHPAY §4
 * `DashPayStatusModel`): the status, the main identity's summary, its unread events and the
 * contact requests waiting. Each read is `undefined` while unknown or when it fails. A contested
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
    status: status.data,
    main: identities.data?.find((i) => i.identity === main),
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
