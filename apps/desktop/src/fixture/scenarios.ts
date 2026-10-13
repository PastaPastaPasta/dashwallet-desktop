// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// Named fixture scenarios. Each builds a fresh `World`: the state the fixture's command
// handlers read and change. Pick one with `?scenario=<name>` (dev server, Playwright) or
// `installFixture({ scenario })` (vitest).

import type { DispatchStateName } from "../api/generated/DispatchStateName";
import type { FlowLockReport } from "../api/generated/FlowLockReport";
import type { FlowOutcomeName } from "../api/generated/FlowOutcomeName";
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
import { buildRows, caughtUp, FUNDED_BALANCES, XSS_PAYLOADS, ZERO_BALANCES, type HistoryVariant } from "./wallet";

export const FIXTURE_PASSPHRASE = "demo";
export const FIXTURE_WALLET_ID = "a1".repeat(32);

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
  cut: Pick<LockOutcome, "flows" | "flowsOmitted">;
  notices: RecentNotice[];
}

/** dw-app's `MAX_REPORT_WALLETS`: a lock report lists this many wallets and counts the rest. */
export const MAX_REPORT_WALLETS = 16;

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
      selftestUnlock: false,
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
    cut: { flows: [], flowsOmitted: 0 },
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

/** The engine's outcome order, weakest first (E0-04 §8.4): a flow's is the strongest of its
 * artifacts'. And dw-app's listing order, unresolved first (`keep_unresolved_first`). */
const STRENGTH: FlowOutcomeName[] = ["cancelled", "maybeSent", "willBeSent", "sent"];
const LISTING: FlowOutcomeName[] = ["maybeSent", "willBeSent", "sent", "cancelled"];
const listed = <T extends { outcome: FlowOutcomeName }>(items: T[]) => items.toSorted((a, b) => LISTING.indexOf(a.outcome) - LISTING.indexOf(b.outcome));

/** A 32-byte artifact id (a txid or a state-transition hash): plain lower-case hex. */
const artifactId = (seed: string) => seed.repeat(64 / seed.length);
/** A step id, as a running call or a funding step is named. */
const step = (flow: string, draft: string, part: string) => `${flow}/${draft}/${part}`;

/** A cut flow as the bridge reports it: the facade's lease id (`lease-` and 32 hex digits, the
 * first 8 being `Lease.id`), the outcome derived from the artifacts, those listed unresolved first. */
const flow = (lease: string, kind: FlowLockReport["flow"], artifacts: [string, FlowOutcomeName][], omitted = 0): FlowLockReport => ({
  lease: `lease-${lease.repeat(4)}`,
  walletId: FIXTURE_WALLET_ID,
  flow: kind,
  outcome: artifacts.reduce<FlowOutcomeName>((o, [, a]) => (STRENGTH.indexOf(a) > STRENGTH.indexOf(o) ? a : o), "cancelled"),
  artifacts: listed(artifacts.map(([artifact, outcome]) => ({ artifact, outcome }))),
  artifactsOmitted: omitted,
});

/** Locks `w` with these flows cut, listed as the bridge lists them; the lock revokes each flow's
 * lease. The dispatch state of every listed artifact starts at its outcome. With `walletsOmitted`
 * above 0 the vault holds that many wallets with secrets beyond the 16 a report lists. */
function cutting(w: World, flows: FlowLockReport[], flowsOmitted = 0, walletsOmitted = 0): World {
  if (walletsOmitted > 0) {
    const others = Array.from({ length: MAX_REPORT_WALLETS - 1 + walletsOmitted }, (_, i) => hex(i + 1).repeat(32));
    w.lock = { ...w.lock, walletsWithSecrets: [...w.lock.walletsWithSecrets, ...others].sort() };
  }
  w.dashpay = registered();
  w.leases = flows.map((f) => lease(f.lease.slice("lease-".length, "lease-".length + 8), f.flow, f.flow === "registration" || f.flow === "topUp" || f.flow === "acceptAndPay"));
  w.dispatch = Object.fromEntries(flows.flatMap((f) => f.artifacts.map((a) => [a.artifact, a.outcome === "cancelled" ? "notSent" : a.outcome])));
  w.cut = { flows: listed(flows), flowsOmitted };
  return w;
}

const hex = (i: number) => i.toString(16).padStart(2, "0");
/** A contact request cut by the lock: its state transition, with this outcome. */
const request = (i: number, outcome: FlowOutcomeName) =>
  flow(`5eed00${hex(i)}`, "contactRequest", outcome === "cancelled" ? [] : [[artifactId(`5eed00${hex(i)}`), outcome]]);

/** A registration whose funding was sent and whose identity call may have gone out: the engine
 * lists the funding step too, so it reads `sent` (E0-04 §8.4). */
const registrationCut = () =>
  flow("3f9a1c27", "registration", [
    [step("registration", "3f9a1c27", "funding"), "sent"],
    [step("registration", "3f9a1c27", "identity"), "maybeSent"],
  ]);

/** The cut flows of a full report (E0-13 D3: 16 flows of 8 artifacts): a registration whose
 * identity call was still running, a top-up with these artifact outcomes, and 14 requests. */
function sixteen(requests: FlowOutcomeName, topUp: FlowOutcomeName[], topUpOmitted: number): FlowLockReport[] {
  return [
    registrationCut(),
    flow("8be04d11", "topUp", topUp.map((o, i) => [artifactId(`8be04d${hex(i)}`), o]), topUpOmitted),
    ...Array.from({ length: 14 }, (_, i) => request(i, requests)),
  ];
}

/** Every listed flow and artifact is pending and more were cut: the omitted ones may be pending
 * too, and no query names them. Unresolved for good. */
const omittedPending = (w: World) => cutting(w, sixteen("willBeSent", Array(8).fill("willBeSent"), 7), 3, 2);

/** Counts above 0, but the order proves nothing omitted is pending: the listed flows end with
 * cancelled ones and the top-up's listed artifacts with sent ones (the top-up itself reads
 * `sent`, its strongest). Unresolved until the listed pending artifacts settle and the
 * registration's call, still running, ends. */
function omittedSettled(w: World): World {
  cutting(w, sixteen("cancelled", ["willBeSent", ...Array<FlowOutcomeName>(7).fill("sent")], 4), 2, 2);
  w.leases[0] = { ...w.leases[0], inFlight: 1, callRunning: true };
  return w;
}

/** Flows in flight when the user locks, all listed: a registration whose funding was sent while
 * its identity call may have gone out (E0-04 §8.4: it reads `sent`, yet something of it is
 * pending), a top-up handed off, a sent payment and a cancelled request. */
const listedFlows = (w: World) =>
  cutting(w, [
    registrationCut(),
    flow("8be04d11", "topUp", [[artifactId("8be04d11"), "willBeSent"]]),
    flow("c0ffee01", "acceptAndPay", [[artifactId("c0ffee01"), "sent"]]),
    request(0, "cancelled"),
  ]);

/** An accept-and-pay whose payment was sent while its `accept_request` call may have gone out.
 * A call is reported under its label, which `dispatchStatus` cannot name: unresolved for good. */
const unnamedCall = (w: World) =>
  cutting(w, [
    flow("c0ffee01", "acceptAndPay", [
      [artifactId("c0ffee01"), "sent"],
      ["accept_request", "maybeSent"],
    ]),
  ]);

/** Everything resolved: the payment was sent and the request cancelled. */
function resolvedFlows(w: World): World {
  return cutting(w, [flow("c0ffee01", "acceptAndPay", [[artifactId("c0ffee01"), "sent"]]), request(0, "cancelled")]);
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
  /** Locking cuts more flows than the report lists, some pending (E0-13 D3): unresolved for good. */
  "lock-report-omitted": (t) => omittedPending(base(t)),
  /** The report omits flows, artifacts and wallets that its order shows are not pending. */
  "lock-report-omitted-settles": (t) => omittedSettled(base(t)),
  /** Locking cuts flows; the report lists them all, some still pending. */
  "lock-report-pending": (t) => listedFlows(base(t)),
  /** Locking cuts flows that all ended: sent or cancelled. */
  "lock-report-resolved": (t) => resolvedFlows(base(t)),
  /** A cut call reported under its label (no id to check): unresolved for good. */
  "lock-report-call": (t) => unnamedCall(base(t)),
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
