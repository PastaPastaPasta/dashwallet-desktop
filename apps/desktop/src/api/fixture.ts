// A sample wallet in TypeScript: the data of docs/design/ux/home-*.png plus a month of
// history. Deterministic (seeded PRNG, fixed clock), so screenshots and element boxes are
// comparable across engines and runs. Passphrase: "demo".

import { COIN } from "../format/amount";
import type { Backend } from "./backend";
import type { AppError, AppEvent, ErrorCode } from "./generated/commands";
import type { Balances } from "./generated/Balances";
import type { HistoryRequest } from "./generated/HistoryRequest";
import type { HistoryRows } from "./generated/HistoryRows";
import type { LaunchInfo } from "./generated/LaunchInfo";
import type { LockStatus } from "./generated/LockStatus";
import type { OpenedNetwork } from "./generated/OpenedNetwork";
import type { SyncStatus } from "./generated/SyncStatus";
import type { TxCategoryName } from "./generated/TxCategoryName";
import type { TxDetails } from "./generated/TxDetails";
import type { TxRow } from "./generated/TxRow";
import type { TxStatusInfo } from "./generated/TxStatusInfo";
import type { TxTypeName } from "./generated/TxTypeName";
import type { WalletSummary } from "./generated/WalletSummary";

/** `default`: the reference wallet; `xss`: hostile labels and names (U6); `empty`: no history;
 * `large`: 10,000 transactions (paging and render cost). */
export type FixtureVariant = "default" | "xss" | "empty" | "large";

export const FIXTURE_PASSPHRASE = "demo";
export const FIXTURE_WALLET_ID = "a1".repeat(32);
/** Fixed "now": Tuesday 6 October 2026, 12:00 local time. */
export const FIXTURE_NOW = new Date(2026, 9, 6, 12, 0, 0);

export const XSS_PAYLOADS = {
  label: `<img src=x onerror="window.__xss='label'">`,
  walletName: `<script>window.__xss='name'</script>Main`,
  contact: `"><svg onload="window.__xss='contact'">`,
};

const TIP = 1_352_000;

function prng(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

function makeIds(rand: () => number) {
  const hex = (n: number) => Array.from({ length: n }, () => "0123456789abcdef"[Math.floor(rand() * 16)]).join("");
  const address = () => "y" + Array.from({ length: 33 }, () => BASE58[Math.floor(rand() * 58)]).join("");
  return { hex, address };
}

const secs = (d: Date) => Math.floor(d.getTime() / 1000);
const at = (daysAgo: number, h: number, m: number) =>
  new Date(FIXTURE_NOW.getFullYear(), FIXTURE_NOW.getMonth(), FIXTURE_NOW.getDate() - daysAgo, h, m);

const settled = (confirmations: number): TxStatusInfo => ({
  kind: confirmations >= 6 ? "confirmed" : "confirming",
  confirmations,
  instantLocked: true,
  chainLocked: true,
  maturesIn: null,
});

interface Spec {
  when: Date;
  type: TxTypeName;
  category: TxCategoryName;
  amount: number;
  status: TxStatusInfo;
  label?: string | null;
  fee?: number | null;
}

function buildRows(variant: FixtureVariant): TxRow[] {
  if (variant === "empty") return [];
  const rand = prng(variant === "large" ? 1010 : 20261006);
  const { hex, address } = makeIds(rand);
  const specs: Spec[] = [
    {
      when: at(0, 1, 40),
      type: "recvWithAddress",
      category: "received",
      amount: 20_000_000,
      status: { kind: "unconfirmed", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
    },
    {
      when: at(0, 1, 10),
      type: "recvWithAddress",
      category: "received",
      amount: 125_000_000,
      label: "Coffee refund",
      status: { kind: "unconfirmed", confirmations: 0, instantLocked: true, chainLocked: false, maturesIn: null },
    },
    { when: at(1, 5, 46), type: "sendToAddress", category: "sent", amount: -50_000_226, fee: 226, label: "Alice", status: settled(150) },
  ];
  const count = variant === "large" ? 10_000 : 40;
  const kinds: [TxTypeName, TxCategoryName, number][] = [
    ["recvWithAddress", "received", 1],
    ["sendToAddress", "sent", -1],
    ["coinJoinMixing", "coinJoin", 0],
    ["sendToSelf", "internalTransfer", 0],
    ["recvWithAddress", "reward", 1],
  ];
  const labels = [null, null, null, "Rent", "Groceries", "Payroll", null, "Bob", null, "Exchange withdrawal"];
  for (let i = 0; i < count; i++) {
    const [type, category, sign] = kinds[Math.floor(rand() * kinds.length)];
    const daysAgo = 2 + Math.floor((i * (variant === "large" ? 900 : 30)) / count);
    const value = Math.floor(rand() * 4 * COIN) + Math.floor(rand() * 100_000);
    const fee = sign <= 0 ? 226 + Math.floor(rand() * 400) : null;
    specs.push({
      when: at(daysAgo, Math.floor(rand() * 24), Math.floor(rand() * 60)),
      type,
      category,
      amount: sign === 0 ? -(fee ?? 0) : sign * value,
      fee,
      label: type === "coinJoinMixing" || type === "sendToSelf" ? null : labels[Math.floor(rand() * labels.length)],
      status: settled(100 + i * 6),
    });
  }
  if (variant !== "large") {
    specs.push({
      when: at(3, 9, 15),
      type: "generated",
      category: "reward",
      amount: 288_000_000,
      status: { kind: "immature", confirmations: 40, instantLocked: false, chainLocked: true, maturesIn: 61 },
    });
    specs.push({
      when: at(4, 18, 2),
      type: "sendToAddress",
      category: "sent",
      amount: -12_345_678,
      fee: 374,
      status: { kind: "abandoned", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
    });
  }
  if (variant === "xss") {
    specs[0].label = XSS_PAYLOADS.label;
    specs[2].label = XSS_PAYLOADS.contact;
  }
  const rows = specs.map((s) => {
    const confirmations = s.status.confirmations;
    return {
      txid: hex(64),
      recordIndex: 0,
      txType: s.type,
      category: s.category,
      status: s.status,
      timestamp: secs(s.when),
      blockHeight: confirmations > 0 ? TIP - confirmations + 1 : null,
      amount: s.amount,
      fee: s.fee ?? null,
      address: address(),
      label: s.label ?? null,
      countsTowardBalance: s.status.kind !== "abandoned",
      involvesWatchOnly: false,
    } satisfies TxRow;
  });
  return rows.sort((a, b) => (b.timestamp ?? 0) - (a.timestamp ?? 0));
}

const BALANCES: Balances = {
  confirmed: 3_019_698_287,
  unconfirmed: 20_000_000,
  immature: 0,
  locked: 0,
  total: 3_039_698_287,
  coinjoin: 0,
};

function fail(code: ErrorCode, message: string): AppError {
  return { code, message, data: null };
}

export class FixtureBackend implements Backend {
  readonly kind = "fixture";
  readonly locale = "en-US";
  private readonly rows: TxRow[];
  private readonly listeners = new Set<(e: AppEvent) => void>();
  private locked: boolean;
  private failedAttempts = 0;

  constructor(
    private readonly launch: LaunchInfo,
    private readonly variant: FixtureVariant = "default",
    { locked = false }: { locked?: boolean } = {},
  ) {
    this.rows = buildRows(variant);
    this.locked = locked;
  }

  /** The launch info of a browser run (no Tauri): testnet, like the reference images. */
  static browserLaunch(): LaunchInfo {
    return {
      fixture: true,
      selftest: false,
      selftestPauseMs: 0,
      theme: null,
      network: { kind: "testnet", dirName: "testnet", unitName: "tDASH" },
      appVersion: "0.1.0",
      engineVersion: "fixture",
    };
  }

  private status(): LockStatus {
    return {
      state: this.locked ? "locked" : "unlocked",
      encrypted: true,
      quickUnlockEnrolled: false,
      failedAttempts: this.failedAttempts,
      retryAfterSecs: null,
      walletsWithSecrets: [FIXTURE_WALLET_ID],
    };
  }

  private emit(event: AppEvent) {
    for (const l of this.listeners) l(event);
  }

  private wallet(id: string) {
    if (id !== FIXTURE_WALLET_ID) throw fail("wallet_not_found", `wallet not found: ${id}`);
  }

  openNetwork = async (): Promise<OpenedNetwork> => ({ network: this.launch.network, lock: this.status() });
  lockStatus = async () => this.status();

  listWallets = async (): Promise<WalletSummary[]> => [
    {
      walletId: FIXTURE_WALLET_ID,
      name: this.variant === "xss" ? XSS_PAYLOADS.walletName : "Main",
      watchOnly: false,
      hasMnemonic: true,
      hd: true,
      birthHeight: 1_200_000,
      createdAt: null,
      balances: BALANCES,
    },
  ];

  unlock = async (passphrase: string) => {
    if (passphrase !== FIXTURE_PASSPHRASE) {
      this.failedAttempts += 1;
      throw {
        ...fail("vault.wrong_passphrase", `wrong passphrase (${this.failedAttempts} failed)`),
        data: { kind: "wrongPassphrase", failedAttempts: this.failedAttempts, retryAfterSecs: null },
      } satisfies AppError;
    }
    this.failedAttempts = 0;
    this.locked = false;
    this.emit({ kind: "vaultLockState", network: this.launch.network.dirName, state: "unlocked" });
    return this.status();
  };

  lock = async () => {
    this.locked = true;
    this.emit({ kind: "vaultLockState", network: this.launch.network.dirName, state: "locked" });
    return this.status();
  };

  balance = async (walletId: string) => {
    this.wallet(walletId);
    return BALANCES;
  };

  syncStatus = async (): Promise<SyncStatus> => ({
    running: true,
    phases: (["headers", "filterHeaders", "filters", "masternodes"] as const).map((phase) => ({
      phase,
      currentHeight: TIP,
      targetHeight: TIP,
      done: true,
    })),
    activePhase: null,
    tipHeight: TIP,
    tipTime: secs(FIXTURE_NOW) - 90,
    chainlockHeight: TIP,
    connectedPeers: 8,
    caughtUp: true,
    secondsSinceProgress: null,
  });

  historyPage = async (walletId: string, request: HistoryRequest): Promise<HistoryRows> => {
    this.wallet(walletId);
    const text = request.text?.trim().toLowerCase();
    let rows = this.rows.filter(
      (r) =>
        (request.categories.length === 0 || request.categories.includes(r.category)) &&
        (!text || [r.label, r.address, r.txid].some((v) => v?.toLowerCase().includes(text))),
    );
    if (request.sort === "oldestFirst") rows = [...rows].reverse();
    if (request.sort === "amountDescending") rows = [...rows].sort((a, b) => b.amount - a.amount);
    if (request.sort === "amountAscending") rows = [...rows].sort((a, b) => a.amount - b.amount);
    const start = request.cursor ? Number(request.cursor) : 0;
    const end = start + request.limit;
    return {
      rows: rows.slice(start, end),
      nextCursor: end < rows.length ? String(end) : null,
      totalMatching: rows.length,
    };
  };

  txDetail = async (walletId: string, txid: string): Promise<TxDetails> => {
    this.wallet(walletId);
    const row = this.rows.find((r) => r.txid === txid);
    if (!row) throw fail("tx_not_found", `transaction not found: ${txid}`);
    const rand = prng(parseInt(txid.slice(0, 8), 16));
    const { hex, address } = makeIds(rand);
    const incoming = row.amount >= 0;
    const fee = row.fee ?? 0;
    const value = Math.abs(row.amount);
    const change = incoming ? 0 : Math.floor(rand() * COIN);
    const details = {
      txid,
      rows: [row],
      status: row.status,
      timestamp: row.timestamp,
      blockHeight: row.blockHeight,
      blockHash: row.blockHeight ? "000000" + hex(58) : null,
      fee: row.fee,
      sizeBytes: 192 + Math.floor(rand() * 200),
      inputs: [
        {
          previousTxid: hex(64),
          previousVout: Math.floor(rand() * 3),
          address: incoming ? address() : row.address,
          // A send row's amount already includes the fee: inputs = outputs + fee.
          amount: incoming ? null : value + change,
          isMine: !incoming,
        },
      ],
      outputs: [
        { vout: 0, address: row.address, amount: incoming ? value : value - fee, isMine: incoming, isChange: false, dataHex: null },
        ...(change > 0 ? [{ vout: 1, address: address(), amount: change, isMine: true, isChange: true, dataHex: null }] : []),
      ],
      message: null,
      label: row.label,
    };
    // Placeholder bytes of the stated size (drawn after the rest, so the ids stay as they were).
    return { ...details, rawHex: "00".repeat(details.sizeBytes) };
  };

  onEvent = async (listener: (e: AppEvent) => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };

  now = () => FIXTURE_NOW;
}
