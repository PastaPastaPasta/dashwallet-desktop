// Named fixture scenarios. Each builds a fresh `World`: the state the fixture's command
// handlers read and change. Pick one with `?scenario=<name>` (dev server, Playwright) or
// `installFixture({ scenario })` (vitest).

import type { DispatchStateName } from "../api/generated/DispatchStateName";
import type { FlowLockReport } from "../api/generated/FlowLockReport";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { Lease } from "../api/generated/Lease";
import type { LockOutcome } from "../api/generated/LockOutcome";
import type { LockStatus } from "../api/generated/LockStatus";
import type { RecentNotice } from "../api/generated/RecentNotice";
import type { SyncStatus } from "../api/generated/SyncStatus";
import type { ThemeName } from "../api/generated/ThemeName";
import type { TxRow } from "../api/generated/TxRow";
import type { WalletSummary } from "../api/generated/WalletSummary";
import { noIdentity, registered, registering, type DashPayWorld } from "./dashpay";
import { buildRows, caughtUp, FUNDED_BALANCES, ZERO_BALANCES, type HistoryVariant } from "./wallet";

export const FIXTURE_PASSPHRASE = "demo";
export const FIXTURE_WALLET_ID = "a1".repeat(32);

export const XSS_PAYLOADS = {
  label: `<img src=x onerror="window.__xss='label'">`,
  walletName: `<script>window.__xss='name'</script>Main`,
  contact: `"><svg onload="window.__xss='contact'">`,
};

export interface FixtureWallet {
  summary: WalletSummary;
  rows: TxRow[];
}

/** What the fixture's engine knows. Handlers mutate it, like the engine's own state. */
export interface World {
  launch: LaunchInfo;
  passphrase: string;
  lock: LockStatus;
  wallets: FixtureWallet[];
  sync: SyncStatus;
  /** The first wallet's DashPay state. */
  dashpay: DashPayWorld;
  leases: Lease[];
  /** `dispatch_status` by artifact. */
  dispatch: Record<string, DispatchStateName>;
  lastLockReport: LockOutcome | null;
  /** What the next `lock` cuts and reports (E0-13 D3's bounded report). */
  cut: Pick<LockOutcome, "flows" | "flowsOmitted" | "walletsWithSecretsOmitted">;
  notices: RecentNotice[];
}

const unlocked = (walletIds: string[]): LockStatus => ({
  state: "unlocked",
  encrypted: true,
  quickUnlockEnrolled: false,
  failedAttempts: 0,
  retryAfterSecs: null,
  walletsWithSecrets: walletIds,
});

function wallet(variant: HistoryVariant): FixtureWallet {
  return {
    summary: {
      walletId: FIXTURE_WALLET_ID,
      name: variant === "xss" ? XSS_PAYLOADS.walletName : "Main",
      watchOnly: false,
      hasMnemonic: true,
      hd: true,
      birthHeight: 1_200_000,
      createdAt: null,
      balances: variant === "empty" ? ZERO_BALANCES : FUNDED_BALANCES,
    },
    rows: buildRows(variant),
  };
}

function base(theme: ThemeName | null, variant: HistoryVariant = "default"): World {
  return {
    launch: {
      selftest: false,
      selftestPauseMs: 0,
      theme,
      // Testnet, like the reference images.
      network: { kind: "testnet", dirName: "testnet", unitName: "tDASH" },
      appVersion: "0.1.0",
      engineVersion: "fixture",
    },
    passphrase: FIXTURE_PASSPHRASE,
    lock: unlocked([FIXTURE_WALLET_ID]),
    wallets: [wallet(variant)],
    sync: caughtUp(),
    dashpay: noIdentity(),
    leases: [],
    dispatch: {},
    lastLockReport: null,
    cut: { flows: [], flowsOmitted: 0, walletsWithSecretsOmitted: 0 },
    notices: [],
  };
}

const lease = (id: string, flow: Lease["flow"], fundsCommitted: boolean, inFlight = 0): Lease => ({
  id,
  walletId: FIXTURE_WALLET_ID,
  flow,
  state: { kind: "active" },
  ownKey: true,
  keyExpiresInSecs: 240,
  fundsCommitted,
  budgets: [{ purpose: "funding", ceiling: "150000000", spent: fundsCommitted ? "120000000" : "0" }],
  inFlight,
  callRunning: inFlight > 0,
});

const flow = (lease: string, kind: FlowLockReport["flow"], outcome: FlowLockReport["outcome"], artifacts: string[], omitted = 0): FlowLockReport => ({
  lease,
  walletId: FIXTURE_WALLET_ID,
  flow: kind,
  outcome,
  artifacts: artifacts.map((artifact) => ({ artifact, outcome })),
  artifactsOmitted: omitted,
});

/** Flows in flight when the user locks: a registration whose funding may have been sent, a
 * top-up handed off, a sent payment and a cancelled request. */
function withFlows(w: World, omitted: boolean): World {
  w.dashpay = registered();
  w.leases = [lease("3f9a1c27", "registration", true, 1), lease("8be04d11", "topUp", true), lease("c0ffee01", "acceptAndPay", true), lease("5eed5eed", "contactRequest", false)];
  w.dispatch = { "tx:3f9a1c27": "maybeSent", "st:8be04d11": "willBeSent", "tx:c0ffee01": "sent" };
  w.cut = {
    flows: [
      flow("3f9a1c27", "registration", "maybeSent", ["tx:3f9a1c27"]),
      flow("8be04d11", "topUp", "willBeSent", ["st:8be04d11"], omitted ? 7 : 0),
      flow("c0ffee01", "acceptAndPay", "sent", ["tx:c0ffee01"]),
      flow("5eed5eed", "contactRequest", "cancelled", []),
    ],
    // The engine cut the rest: 16 flows listed at most (E0-13 D3), here 4 + 3 more.
    flowsOmitted: omitted ? 3 : 0,
    walletsWithSecretsOmitted: omitted ? 2 : 0,
  };
  return w;
}

/** Everything resolved: the payment was sent and the request cancelled. */
function resolvedFlows(w: World): World {
  w.dashpay = registered();
  w.leases = [lease("c0ffee01", "acceptAndPay", true), lease("5eed5eed", "contactRequest", false)];
  w.dispatch = { "tx:c0ffee01": "sent" };
  w.cut = {
    flows: [flow("c0ffee01", "acceptAndPay", "sent", ["tx:c0ffee01"]), flow("5eed5eed", "contactRequest", "cancelled", [])],
    flowsOmitted: 0,
    walletsWithSecretsOmitted: 0,
  };
  return w;
}

const SCENARIOS = {
  /** The reference wallet: balance, a month of history, caught up, unlocked. */
  funded: (t) => base(t),
  /** A new wallet: no history, zero balance. */
  empty: (t) => base(t, "empty"),
  /** No wallet on the network yet, and no vault. */
  "no-wallet": (t) => {
    const w = base(t);
    w.wallets = [];
    w.lock = { ...unlocked([]), state: "noVault", encrypted: false };
    return w;
  },
  /** The funded wallet behind the lock screen (passphrase `demo`). */
  locked: (t) => {
    const w = base(t);
    w.lock = { ...w.lock, state: "locked" };
    return w;
  },
  /** 10,003 transactions: paging and render cost. */
  large: (t) => base(t, "large"),
  /** Hostile labels and names (U6): they must render as text. */
  xss: (t) => base(t, "xss"),
  "dashpay-no-identity": (t) => base(t),
  /** A username registration funded and waiting for its InstantSend lock. */
  "dashpay-registering": (t) => {
    const w = base(t);
    w.dashpay = registering();
    w.leases = [lease("9a0b1c2d", "registration", true)];
    return w;
  },
  /** "alice", with contacts in every relation and channel state and two requests waiting. */
  "dashpay-registered": (t) => {
    const w = base(t);
    w.dashpay = registered();
    return w;
  },
  /** Locking cuts flows and the bounded report omits some (E0-13 D3): unresolved. */
  "lock-report-omitted": (t) => withFlows(base(t), true),
  /** Locking cuts flows; the report lists them all, some still pending. */
  "lock-report-pending": (t) => withFlows(base(t), false),
  /** Locking cuts flows that all ended: sent or cancelled. */
  "lock-report-resolved": (t) => resolvedFlows(base(t)),
} satisfies Record<string, (theme: ThemeName | null) => World>;

export type ScenarioName = keyof typeof SCENARIOS;
export const SCENARIO_NAMES = Object.keys(SCENARIOS) as ScenarioName[];
export const DEFAULT_SCENARIO: ScenarioName = "funded";

export function isScenario(s: string): s is ScenarioName {
  return Object.hasOwn(SCENARIOS, s);
}

export function buildWorld(name: ScenarioName, theme: ThemeName | null = null): World {
  return SCENARIOS[name](theme);
}
